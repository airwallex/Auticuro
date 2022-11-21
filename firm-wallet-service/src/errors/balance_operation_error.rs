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
use bigdecimal::ParseBigDecimalError;
use hologram_protos::firm_walletpb::accountpb::*;
use hologram_protos::firm_walletpb::errorpb::Error as ErrorPb;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("BalanceLimitExceeded BalanceLimit={:?}", .0)]
    BalanceLimitExceeded(BalanceLimit),

    #[error("InvalidAccountState AccountState={:?}", .0)]
    InvalidAccountState(AccountState),

    #[error("AccountNotExist account={}", .0)]
    AccountNotExist(String),

    #[error("IncompatibleAssetClass from={:?}, to={:?}", .0, .1)]
    IncompatibleAssetClass(AssetClass, AssetClass),

    #[error("ParseError for number: {}, errors details: {:?}", .0, .1)]
    ParseError(String, ParseBigDecimalError),

    #[error("ReservationIdAlreadyExist, reservation_id={}", .0)]
    ReservationIdAlreadyExist(String),

    #[error("ReservationIdNotFound, reservation_id={}", .0)]
    ReservationIdNotFound(String),

    #[error("InvalidAmount, amount={}", amount)]
    InvalidAmount { amount: String },
}

impl Error {
    pub fn into_proto(self) -> ErrorPb {
        let mut error_pb = ErrorPb::default();
        error_pb.mut_other_error().set_reason(format!("{:?}", self));
        error_pb
    }
}
