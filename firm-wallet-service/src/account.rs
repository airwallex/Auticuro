/**
 * Copyright 2022 Airwallex (Hong Kong) Limited
 *
 * Licensed under the Apache License, Version 2.0 (the "License"); you may not use this file except
 * in compliance with the License.
 *
 * You may obtain a copy of the License at
 *   http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software distributed under the License
 * is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express
 * or implied.
 *
 * See the License for the specific language governing permissions and limitations under the License.
 */
use crate::balance_calculator::BalanceCalculator;
use crate::errors::account_management_error::Error as AccountManagementError;
use crate::errors::balance_operation_error::Error as BalanceOperationError;
use crate::errors::general_error::GeneralResult;
use hologram_protos::firm_walletpb::accountpb::{
    Account as AccountPb, AccountConfig as AccountConfigPb, AccountState as AccountStatePb,
    AccountState, AssetClass as AssetClassPb, AssetClass_Cash, Balance as BalancePb, BalanceLimit,
};
use hologram_protos::firm_walletpb::balance_operation_servicepb::BalanceChange;
use lazy_static::lazy_static;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use tikv_util::{info, trace};

lazy_static! {
    static ref LEGAL_TRANSITIONS: HashMap<AccountState, HashSet<AccountState>> = {
        let mut m = HashMap::new();
        m.insert(
            AccountStatePb::Normal,
            HashSet::from([AccountStatePb::Locked, AccountStatePb::Deleted]),
        );
        m.insert(
            AccountStatePb::Locked,
            HashSet::from([AccountStatePb::Normal]),
        );
        m
    };
}

/// The `struct Account` is an in-mem wrapper for the Account protobuf to simplify the account
/// manipulations. An account contains three parts: `Balance`, `AccountConfig` and `AccountState`
/// The `Account` struct provides basic query and updates APIs for these three parts.
#[derive(Clone)]
pub struct Account(pub AccountPb);

impl Account {
    /// Create an Account by account_id, with balance = 0.0, state = Normal,
    /// asset_class = Cash {currency = USD}
    pub fn new(account_id: &str) -> Self {
        info!(
            "Account created for {} with initial balance 0.0",
            account_id
        );
        let mut account_pb = AccountPb::default();
        account_pb.set_balance(Balance::default().into_proto());
        account_pb.set_config(AccountConfig::default().into_proto());
        account_pb.set_state(AccountStatePb::Normal);
        account_pb.set_account_id(account_id.to_string());
        Account(account_pb)
    }
}

/// All the account update methods follow the Copy-On-Write pattern, and the modifications are
/// applied on a cloned account. For balance operation methods, the account should be in the
/// Normal state, and the updated available balance must not violate the balance limitations. For
/// account state operation methods, the state transition will be validated.
impl Account {
    pub fn from_proto(account_pb: AccountPb) -> Self {
        Account(account_pb)
    }

    pub fn to_proto(&self) -> AccountPb {
        self.0.clone()
    }

    pub fn account_id(&self) -> &str {
        &self.0.account_id
    }

    /// If successfully executed, `available += amount`.
    pub fn increase_balance_by(
        &self,
        amount: &str,
        calculator: &BalanceCalculator,
    ) -> GeneralResult<(Self, BalanceChange)> {
        self.assert_state_is_normal()?;

        let mut new_account = self.clone();

        let new_balance = calculator.add(new_account.get_balance().get_available(), amount)?;
        new_account.validate_balance_limitation(&new_balance, calculator)?;
        new_account.0.mut_balance().set_available(new_balance);

        let mut balance_change = BalanceChange::default();
        balance_change.set_prev_balance(self.get_balance().clone());
        balance_change.set_curr_balance(new_account.get_balance().clone());
        balance_change.set_account_id(self.account_id().to_string());

        trace!(
            "Updated balance of account: {}, balance change: {:?}.",
            self.account_id(),
            balance_change
        );

        Ok((new_account, balance_change))
    }

    pub fn get_balance(&self) -> &BalancePb {
        self.0.get_balance()
    }

    /// Set the BalanceLimit of the current account, basically the `upper limit` and `lower limit`.
    pub fn set_balance_limit(&self, balance_limit: &BalanceLimit) -> Self {
        let mut updated_account = self.clone();
        updated_account
            .0
            .mut_config()
            .set_balance_limit(balance_limit.clone());
        updated_account
    }

    /// Reserve money with a `reservation_id`. The`amount` should >= 0 and the `reservation_id`
    /// should not exist in the `Balance.reservations`.
    /// 1. `available -= amount`
    /// 2. Insert an entry `<reservation_id, amount>` into `Balance.reservations`
    pub fn reserve(
        &self,
        reservation_id: &str,
        amount: &str,
        calculator: &BalanceCalculator,
    ) -> GeneralResult<Self> {
        self.assert_state_is_normal()?;
        Account::assert_amount_gt_zero(amount, calculator)?;

        if self
            .get_balance()
            .get_reservations()
            .contains_key(reservation_id)
        {
            return Err(BalanceOperationError::ReservationIdAlreadyExist(
                reservation_id.to_string(),
            )
            .into());
        }

        let (mut updated_account, _) =
            self.increase_balance_by(&calculator.neg(amount)?, calculator)?;
        updated_account
            .0
            .mut_balance()
            .mut_reservations()
            .insert(reservation_id.to_string(), amount.to_string());
        Ok(updated_account)
    }

    /// Release all money by `reservation_id`. The entry `reservation_id` should exist in
    /// `Balance.reservations`.
    /// 1. Remove the entry `<reservation_id, reservation_amount>` from `Balance.reservations`
    /// 2. `available += reservation_amount`
    pub fn release_all(
        &self,
        reservation_id: &str,
        calculator: &BalanceCalculator,
    ) -> GeneralResult<Self> {
        self.assert_state_is_normal()?;
        let reservation_amount = self.assert_reservation_exists(reservation_id)?;

        let (mut updated_account, _) = self.increase_balance_by(&reservation_amount, calculator)?;
        updated_account
            .0
            .mut_balance()
            .mut_reservations()
            .remove(reservation_id)
            .unwrap();
        Ok(updated_account)
    }

    /// Partially release money by `reservation_id`. The entry `reservation_id` should exist in
    /// `Balance.reservations` and the `amount` should <= `reservation_amount`.
    /// 1. If `reservation_amount == amount`, remove entry `<reservation_id, reservation_amount>`,
    ///    else, update the entry to `<reservation_id, reservation_amount - amount>`
    /// 2. `available += amount`
    pub fn partial_release(
        &self,
        reservation_id: &str,
        amount: &str,
        calculator: &BalanceCalculator,
    ) -> GeneralResult<Self> {
        self.assert_state_is_normal()?;
        Account::assert_amount_gt_zero(amount, calculator)?;

        let reservation_amount = self.assert_reservation_exists(reservation_id)?;

        let cmp_to_reservation_amount = calculator.cmp(amount, &reservation_amount)?;
        if cmp_to_reservation_amount == Ordering::Greater {
            Err(BalanceOperationError::InvalidAmount {
                amount: amount.to_string(),
            }
            .into())
        } else if cmp_to_reservation_amount == Ordering::Equal {
            self.release_all(reservation_id, calculator)
        } else {
            let (mut updated_account, _) = self.increase_balance_by(&amount, calculator)?;
            let remained_amount = calculator.add(&reservation_amount, &calculator.neg(amount)?)?;
            updated_account
                .0
                .mut_balance()
                .mut_reservations()
                .insert(reservation_id.to_string(), remained_amount);
            Ok(updated_account)
        }
    }

    /// Lock the account. If locked, the account is not allowed to do any operations until unlocked.
    pub fn lock(&self) -> GeneralResult<Self> {
        self.validate_state_transition(AccountState::Locked)?;
        let mut updated_account = self.clone();
        updated_account.0.set_state(AccountStatePb::Locked);
        Ok(updated_account)
    }

    /// Unlock the locked account
    pub fn unlock(&self) -> GeneralResult<Self> {
        self.validate_state_transition(AccountState::Normal)?;
        let mut updated_account = self.clone();
        updated_account.0.set_state(AccountStatePb::Normal);
        Ok(updated_account)
    }

    /// Delete the account. Prerequisites:
    /// 1. `available == 0`
    /// 2. No existence of reservations
    pub fn delete(&self, calculator: &BalanceCalculator) -> GeneralResult<Self> {
        self.validate_state_transition(AccountState::Deleted)?;

        self.assert_balance_is_zero(calculator)?;
        let mut updated_account = self.clone();
        updated_account.0.set_state(AccountStatePb::Deleted);
        Ok(updated_account)
    }
}

impl Account {
    /// Validate that the two accounts have the same `asset_class`.
    pub fn validate_asset_class_compatibility(&self, other: &Account) -> GeneralResult<()> {
        let this_asset_class = self.0.get_config().get_asset_class();
        let other_asset_class = other.0.get_config().get_asset_class();
        if this_asset_class != other_asset_class {
            Err(BalanceOperationError::IncompatibleAssetClass(
                this_asset_class.clone(),
                other_asset_class.clone(),
            )
            .into())
        } else {
            Ok(())
        }
    }

    fn validate_state_transition(&self, target_state: AccountState) -> GeneralResult<()> {
        if LEGAL_TRANSITIONS
            .get(&self.0.get_state())
            .filter(|legal_target_states| legal_target_states.contains(&target_state))
            .is_some()
        {
            Ok(())
        } else {
            Err(AccountManagementError::InvalidStateTransition {
                from: self.0.get_state(),
                to: target_state,
            }
            .into())
        }
    }

    fn validate_balance_limitation(
        &self,
        balance: &str,
        calculator: &BalanceCalculator,
    ) -> GeneralResult<()> {
        let account_pb = &self.0;
        if !account_pb.get_config().has_balance_limit() {
            return Ok(());
        }
        let balance_limit = account_pb.get_config().get_balance_limit();
        let cmp_to_upper = calculator.cmp(balance, balance_limit.get_upper())?;
        let cmp_to_lower = calculator.cmp(balance, balance_limit.get_lower())?;

        if cmp_to_lower == Ordering::Less || cmp_to_upper == Ordering::Greater {
            Err(BalanceOperationError::BalanceLimitExceeded(balance_limit.clone()).into())
        } else {
            Ok(())
        }
    }

    fn assert_state_is_normal(&self) -> GeneralResult<()> {
        let account_pb = &self.0;

        if account_pb.get_state() != AccountStatePb::Normal {
            Err(BalanceOperationError::InvalidAccountState(account_pb.get_state().clone()).into())
        } else {
            Ok(())
        }
    }

    fn assert_amount_gt_zero(amount: &str, calculator: &BalanceCalculator) -> GeneralResult<()> {
        let cmp_to_zero = calculator.cmp(amount, Balance::ZERO)?;

        if cmp_to_zero == Ordering::Less || cmp_to_zero == Ordering::Equal {
            return Err(BalanceOperationError::InvalidAmount {
                amount: amount.to_string(),
            }
            .into());
        }
        Ok(())
    }

    fn assert_balance_is_zero(&self, calculator: &BalanceCalculator) -> GeneralResult<()> {
        let balance = self.0.get_balance();
        if balance.reservations.is_empty()
            && calculator.cmp(&balance.available, Balance::ZERO)? == Ordering::Equal
        {
            Ok(())
        } else {
            Err(AccountManagementError::BalanceIsNotZero(balance.clone()).into())
        }
    }

    fn assert_reservation_exists(&self, reservation_id: &str) -> GeneralResult<String> {
        let reservation = self.0.get_balance().reservations.get(reservation_id);
        match reservation {
            Some(reservation_amount) => Ok(reservation_amount.to_string()),
            None => {
                Err(BalanceOperationError::ReservationIdNotFound(reservation_id.to_string()).into())
            }
        }
    }
}

pub struct Balance(BalancePb);

impl Default for Balance {
    fn default() -> Self {
        let mut balance_pb = BalancePb::default();
        balance_pb.set_available(Balance::ZERO.to_string());
        Balance(balance_pb)
    }
}

impl Balance {
    const ZERO: &'static str = "0.0";

    pub fn into_proto(self) -> BalancePb {
        self.0
    }
}

pub struct AccountConfig(AccountConfigPb);

impl Default for AccountConfig {
    fn default() -> Self {
        let mut account_config_pb = AccountConfigPb::default();
        let mut asset_class = AssetClassPb::default();
        let mut cash = AssetClass_Cash::default();

        // Todo Handle properly
        cash.set_currency("USD".to_string());
        asset_class.set_cash(cash);
        account_config_pb.set_asset_class(asset_class);
        AccountConfig(account_config_pb)
    }
}

impl AccountConfig {
    pub fn into_proto(self) -> AccountConfigPb {
        self.0
    }
}
