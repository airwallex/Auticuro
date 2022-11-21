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

#[cfg(test)]
mod test {
    use crate::balance_calculator::BalanceCalculator;
    use crate::config::{
        ClusterConfig, Config, EventLogGCConfig, EventLogGCSetting, WalletServiceConfig,
    };
    use crate::start_wallet;
    use grpcio::{self, *};
    use hologram_protos::firm_walletpb::account_management_servicepb::AccountManagementServiceClient;
    use hologram_protos::firm_walletpb::account_management_servicepb::*;
    use hologram_protos::firm_walletpb::accountpb::AccountState;
    use hologram_protos::firm_walletpb::balance_operation_servicepb::BalanceOperationServiceClient;
    use hologram_protos::firm_walletpb::balance_operation_servicepb::TransferRequest;
    use hologram_protos::firm_walletpb::balance_operation_servicepb::*;
    use hologram_protos::firm_walletpb::internal_servicepb::InternalServiceClient;
    use hologram_protos::firm_walletpb::internal_servicepb::*;
    use rand::{distributions::Alphanumeric, Rng};
    use std::env;
    use std::fs;
    use std::sync::Arc;
    use std::thread;
    use std::thread::sleep;
    use std::time::Duration;
    use tikv_util::{debug, info};

    #[test]
    fn test_balance_operations() {
        let peer_id = 1;
        let test_db_dir = "./test_db_dir";
        // Pre-start: CLean up the rocks db dir for sure
        clean_up_storage(test_db_dir);

        // 1. Setup the env variable since the wallet service depend on them to start
        setup_env_vars(peer_id, test_db_dir.to_string());
        let gc_limit = 1000;
        setup_gc_env_vars(gc_limit);

        // 2. Start the single node wallet service in a new thread, it will be shutdown when the
        // test main thread exits
        let cluster_config = ClusterConfig {
            cluster_id: 1,
            cluster_size: 1,
            store_id: 1,
        };
        let wallet_service_config = WalletServiceConfig::default();
        let config = Config {
            wallet_service_config,
            cluster_config,
            event_log_gc_setting: EventLogGCSetting::Enabled(EventLogGCConfig::default()),
        };

        let _ = thread::spawn(|| start_wallet(config));

        // 3. Build a gRPC client and call the interface in main thread
        // Todo Wait the wallet start up, ideally we should introduce a notify channel in the
        // Wallet Service for easy testing
        info!("Sleep for 3 sec to wait the wallet service start up...");
        thread::sleep(Duration::from_secs(3));
        let balance_operation_service_client = create_balance_operation_service_client(peer_id);
        let internal_service_client = create_internal_service_client(peer_id);
        let account_management_service_client = create_account_management_service_client(peer_id);

        let dummy_from_account: &str = "dummy_from_account";

        // 4 Verify balance operations
        // 4.1 Transfer
        verify_transfer(&balance_operation_service_client, &internal_service_client);

        // 4.2 Batch balance operation
        verify_batch_balance_operation_req(&balance_operation_service_client);

        verify_reserve_and_release(
            &balance_operation_service_client,
            &account_management_service_client,
        );

        verify_account_operations(&account_management_service_client);

        // 5. Test GC
        verify_gc(
            &balance_operation_service_client,
            &internal_service_client,
            gc_limit,
        );

        // 6. Test QueryBalance with illegal index
        let mut query_balance_req_with_illegal_seq_num =
            build_query_balance_req(dummy_from_account);
        query_balance_req_with_illegal_seq_num.set_seq_num(gc_limit * 2 + 1);
        let query_balance_resp_with_illegal_seq_num = balance_operation_service_client
            .query_balance(&query_balance_req_with_illegal_seq_num)
            .unwrap();
        assert!(query_balance_resp_with_illegal_seq_num.has_error());

        info!(
            "query_balance_resp_with_illegal_seq_num: {:?}",
            query_balance_resp_with_illegal_seq_num
        );

        // 6.2 Test QueryBalance with illegal account
        let query_balance_req_with_illegal_account = build_query_balance_req("NotExistAccount");
        let query_balance_resp_with_illegal_account = balance_operation_service_client
            .query_balance(&query_balance_req_with_illegal_account)
            .unwrap();

        assert!(query_balance_resp_with_illegal_account.has_error());
        info!(
            "query_balance_resp_with_illegal_account: {:?}",
            query_balance_resp_with_illegal_account
        );

        // 7. Clean up the rocks db dir
        clean_up_storage(test_db_dir);
    }

    fn build_channel(peer_id: u32) -> Channel {
        let addr = format!("127.0.0.1:2016{}", peer_id);
        let env = Arc::new(EnvBuilder::new().build());
        ChannelBuilder::new(env).connect(&addr)
    }

    fn create_balance_operation_service_client(peer_id: u32) -> BalanceOperationServiceClient {
        let channel = build_channel(peer_id);
        BalanceOperationServiceClient::new(channel)
    }

    fn create_internal_service_client(peer_id: u32) -> InternalServiceClient {
        let channel = build_channel(peer_id);
        InternalServiceClient::new(channel)
    }

    fn create_account_management_service_client(peer_id: u32) -> AccountManagementServiceClient {
        let channel = build_channel(peer_id);
        AccountManagementServiceClient::new(channel)
    }

    // Todo Ideally, we should remove the env dependency
    fn setup_env_vars(peer_id: u32, db_dir: String) {
        env::set_var(
            format!("peer_{}", peer_id),
            format!("0.0.0.0:2016{}", peer_id),
        );
        env::set_var(
            format!("status_address_{}", peer_id),
            format!("0.0.0.0:2021{}", peer_id),
        );
        env::set_var(
            "wallet_db_path",
            format!("{}/wallet_db_{}", db_dir, peer_id),
        );
        env::set_var("raft_db_path", format!("{}/raft_db_{}", db_dir, peer_id));
        env::set_var("db_path", format!("{}/db_{}", db_dir, peer_id));
    }

    fn setup_gc_env_vars(gc_limit: u64) {
        env::set_var("event_log_gc_count_limit", gc_limit.to_string());
        env::set_var("event_log_gc_batch_size", "10");
    }

    fn clean_up_storage(dir: &str) {
        if let Err(e) = fs::remove_dir_all(dir) {
            debug!("Encounter errors {} when trying to remove dir: {}", e, dir);
        }
    }

    fn build_transfer_req(
        amount: &str,
        to_account: &str,
        from_account: &str,
        dedup_id: &str,
    ) -> TransferRequest {
        let mut transfer_request_req = TransferRequest::default();
        let mut transfer_spec = TransferRequest_TransferSpec::default();
        transfer_spec.set_amount(amount.to_string());
        transfer_spec.set_from_account_id(from_account.clone().to_string());
        transfer_spec.set_to_account_id(to_account.clone().to_string());
        transfer_request_req.set_dedup_id(dedup_id.to_string());
        transfer_request_req.set_transfer_spec(transfer_spec);
        transfer_request_req
    }

    fn verify_transfer(
        balance_operation_service_client: &BalanceOperationServiceClient,
        internal_service_client: &InternalServiceClient,
    ) {
        let amount = "20.1";
        let neg_amount = "-20.1";
        let dummy_to_account: &str = "dummy_to_account";
        let dummy_from_account: &str = "dummy_from_account";
        let dummy_dedup_id: &str = "dummy_dedup_id";

        let transfer_req =
            build_transfer_req(amount, dummy_to_account, dummy_from_account, dummy_dedup_id);
        let transfer_resp = balance_operation_service_client
            .transfer(&transfer_req)
            .unwrap();
        info!("Got transfer_resp: {:?}", transfer_resp);

        // Todo Wait for the async handling of the posted journal entry, we should use a notify
        // channel in the future
        info!("Sleep for 1 sec to make sure transfer request has been handled");
        thread::sleep(Duration::from_secs(1));
        let query_to_account_req = build_query_balance_req(dummy_to_account);
        let query_to_account_resp = balance_operation_service_client
            .query_balance(&query_to_account_req)
            .unwrap();

        let query_from_account_req = build_query_balance_req(dummy_from_account);
        let query_from_account_resp = balance_operation_service_client
            .query_balance(&query_from_account_req)
            .unwrap();

        let query_events_req = build_query_events_req();
        let query_events_resp = internal_service_client
            .query_events(&query_events_req)
            .unwrap();

        info!("Got query_credit_balance_resp: {:?}", query_to_account_resp);
        info!(
            "Got query_debit_balance_resp: {:?}",
            query_from_account_resp
        );
        info!("Got query_events_resp: {:?}", query_events_resp);
        assert_eq!(
            query_to_account_resp.get_balance().get_available(),
            amount.to_string()
        );

        let to_balance = query_to_account_resp
            .get_balance()
            .get_available()
            .to_string();
        let from_balance = query_from_account_resp
            .get_balance()
            .get_available()
            .to_string();

        assert_eq!(from_balance, neg_amount);
        assert_eq!(to_balance, amount);

        let last_seq_num = query_to_account_resp.get_seq_num();
        assert_eq!(
            query_to_account_resp.get_seq_num(),
            query_from_account_resp.get_seq_num()
        );
        let events = query_events_resp.get_events();
        assert_eq!(events.len(), 1);
        let event = &events[0];

        // The latest event would have the latest view for certain wallet
        let latest_response = event.get_payload().get_transfer_event().get_response();
        assert_eq!(
            latest_response.get_to().get_curr_balance().get_available(),
            to_balance
        );
        assert_eq!(
            latest_response
                .get_from()
                .get_curr_balance()
                .get_available(),
            from_balance
        );

        // 4.2 Post the second Transfer Request(duplicated req will be ignored)
        let second_transfer_req = transfer_req;
        let _ = balance_operation_service_client
            .transfer(&second_transfer_req)
            .unwrap();

        // Todo Wait for the async handling of the posted journal entry, we should use a notify
        // channel in the future
        info!("Sleep for 1 sec to make sure transfer request has been handled");
        thread::sleep(Duration::from_secs(1));

        let second_query_from_balance_req = build_query_balance_req(dummy_from_account);
        let second_query_from_balance_resp = balance_operation_service_client
            .query_balance(&second_query_from_balance_req)
            .unwrap();

        // Although the transfer_req is duplicate, the wallet will not create a new event, but
        // the raft log will be viewed as applied
        assert_eq!(second_query_from_balance_resp.get_seq_num(), last_seq_num);

        let second_query_events_req = build_query_events_req();
        let second_query_events_resp = internal_service_client
            .query_events(&second_query_events_req)
            .unwrap();

        // No events will be generated for duplicated transfer_req
        assert_eq!(second_query_events_resp.get_events().len(), 1);
    }

    fn build_batch_balance_operation_req(
        dedup_id: &str,
        transfer_specs: Vec<BatchBalanceOperationRequest_BalanceOperationSpec>,
    ) -> BatchBalanceOperationRequest {
        let mut batch_balance_operation_req = BatchBalanceOperationRequest::default();
        batch_balance_operation_req.set_dedup_id(dedup_id.to_string());
        for transfer_spec in transfer_specs {
            batch_balance_operation_req
                .mut_balance_operation_spec()
                .push(transfer_spec);
        }
        batch_balance_operation_req
    }

    fn verify_batch_balance_operation_req(
        balance_operation_service_client: &BalanceOperationServiceClient,
    ) {
        let amount = "20.1";
        let neg_amount = "-20.1";
        let account_a: &str = "account_a";
        let account_b: &str = "account_b";

        let mut transfer_spec1 = BatchBalanceOperationRequest_BalanceOperationSpec::default();
        transfer_spec1.set_amount(amount.to_string());
        transfer_spec1.set_account_id(account_a.to_string());

        let mut transfer_spec2 = BatchBalanceOperationRequest_BalanceOperationSpec::default();
        transfer_spec2.set_amount(neg_amount.to_string());
        transfer_spec2.set_account_id(account_b.to_string());

        let transfer_specs = vec![transfer_spec1.clone(), transfer_spec2.clone()];

        let batch_balance_operation_req = build_batch_balance_operation_req(amount, transfer_specs);
        let batch_balance_operation_resp = balance_operation_service_client
            .batch_balance_operation(&batch_balance_operation_req)
            .unwrap();
        info!(
            "Got batch_balance_operation_resp: {:?}",
            batch_balance_operation_resp
        );

        let zero = "0.0";
        for balance_change in batch_balance_operation_resp.balance_changes {
            if balance_change.account_id == transfer_spec1.account_id {
                assert_eq!(balance_change.get_prev_balance().available, zero);
                assert_eq!(
                    balance_change.get_curr_balance().available,
                    transfer_spec1.amount
                );
            }
            if balance_change.account_id == transfer_spec2.account_id {
                assert_eq!(balance_change.get_prev_balance().available, zero);
                assert_eq!(
                    balance_change.get_curr_balance().available,
                    transfer_spec2.amount
                )
            }
        }
    }

    fn build_release_req(
        amount: &str,
        account_id: &str,
        dedup_id: &str,
        reservation_id: &str,
    ) -> ReleaseRequest {
        let mut req = ReleaseRequest::default();
        req.set_account_id(account_id.to_string());
        req.set_amount(amount.to_string());
        req.set_dedup_id(dedup_id.to_string());
        req.set_reservation_id(reservation_id.to_string());
        req
    }

    fn build_reserve_req(
        amount: &str,
        account_id: &str,
        dedup_id: &str,
        reservation_id: &str,
    ) -> ReserveRequest {
        let mut req = ReserveRequest::default();
        req.set_amount(amount.to_string());
        req.set_account_id(account_id.to_string());
        req.set_dedup_id(dedup_id.to_string());
        req.set_reservation_id(reservation_id.to_string());
        req
    }

    fn verify_reserve_and_release(
        balance_operation_service_client: &BalanceOperationServiceClient,
        account_management_service: &AccountManagementServiceClient,
    ) {
        let account_id: &str = &random_string();
        let initial_total_balance = "100";
        let reserve_amount = "30";
        let available_amount = "70";
        let reservation_id: &str = "reservation_id_1";

        let _ = create_account(account_management_service, account_id);
        increase_balance(
            balance_operation_service_client,
            account_id,
            initial_total_balance,
        );
        let reserve_req =
            build_reserve_req(reserve_amount, account_id, &random_string(), reservation_id);
        let reserve_resp = balance_operation_service_client
            .reserve(&reserve_req)
            .unwrap();
        info!("reserve_resp: {:?}", reserve_resp);
        assert_eq!(reserve_resp.get_curr_balance().available, available_amount);
        assert_eq!(
            reserve_resp
                .get_curr_balance()
                .reservations
                .get(reservation_id)
                .unwrap(),
            reserve_amount
        );

        // 1. Partial Release
        let first_release_amount: &str = "10";
        let remained_amount_after_first_release = BalanceCalculator
            .add(
                reserve_amount,
                &BalanceCalculator.neg(first_release_amount).unwrap(),
            )
            .unwrap();
        let available_amount_after_first_release = BalanceCalculator
            .add(available_amount, first_release_amount)
            .unwrap();
        let partial_release_req = build_release_req(
            first_release_amount,
            account_id,
            &random_string(),
            reservation_id,
        );
        let partial_release_resp = balance_operation_service_client
            .release(&partial_release_req)
            .unwrap();
        info!("partial_release_resp: {:?}", partial_release_resp);
        assert_eq!(
            partial_release_resp.get_curr_balance().available,
            available_amount_after_first_release
        );
        assert_eq!(
            partial_release_resp
                .get_curr_balance()
                .reservations
                .get(reservation_id)
                .unwrap(),
            &remained_amount_after_first_release
        );

        // 2. Invalid release(Request's Release Amount > Reserved Amount)
        let invalid_release_amount = "20.0000000001";
        let invalid_release_req = build_release_req(
            invalid_release_amount,
            account_id,
            &random_string(),
            reservation_id,
        );
        let invalid_release_resp = balance_operation_service_client
            .release(&invalid_release_req)
            .unwrap();
        info!("invalid_release_resp: {:?}", invalid_release_resp);
        assert!(invalid_release_resp.has_error());

        // 3. Release Remaining
        let mut release_remaining_req = build_release_req(
            &remained_amount_after_first_release,
            account_id,
            &random_string(),
            reservation_id,
        );

        // ReleaseRequest without amount implies release_remaining
        release_remaining_req.clear_amount();

        let release_remaining_resp = balance_operation_service_client
            .release(&release_remaining_req)
            .unwrap();
        info!("release_remaining_resp: {:?}", release_remaining_resp);
        assert_eq!(
            release_remaining_resp.get_curr_balance().available,
            initial_total_balance
        );
        assert_eq!(
            release_remaining_resp.get_curr_balance().reservations.len(),
            0
        );
    }

    fn verify_account_operations(account_management_service: &AccountManagementServiceClient) {
        let account = &random_string();
        let create_account_resp = create_account(account_management_service, account);
        assert_eq!(
            create_account_resp.get_account().get_state(),
            AccountState::Normal
        );

        // Create account again would fail
        let duplicate_create_account_resp = create_account(account_management_service, account);
        assert!(duplicate_create_account_resp.has_error());

        let lock_account_resp = lock_account(account_management_service, account);
        assert_eq!(
            lock_account_resp.get_account().get_state(),
            AccountState::Locked
        );

        let invalid_account = &random_string();
        let lock_account_resp = lock_account(account_management_service, invalid_account);
        assert!(lock_account_resp.has_error());

        let unlock_account_resp = unlock_account(account_management_service, account);
        assert_eq!(
            unlock_account_resp.get_account().get_state(),
            AccountState::Normal
        );

        let delete_account_resp = delete_account(account_management_service, account);
        assert_eq!(
            delete_account_resp.get_account().get_state(),
            AccountState::Deleted
        );

        // Todo Update account will fail if account is not in correct state
        let lower_limit = "10";
        let update_account_resp = update_account(account_management_service, account, lower_limit);
        assert_eq!(
            update_account_resp
                .get_account()
                .get_config()
                .get_balance_limit()
                .get_lower(),
            lower_limit
        );

        // Get the account immediately after Deleting an account will find the account is in Deleted state
        let get_account_resp = get_account(account_management_service, account);
        assert_eq!(
            get_account_resp.get_account().get_state(),
            AccountState::Deleted
        );

        // sleep 100 ms to wait for the write batch flush, and the account is physically deleted from
        // rocksDB and mem
        sleep(Duration::from_millis(100));
        let get_account_resp = get_account(account_management_service, account);

        // The Deleted account has been removed from the wallet
        assert!(get_account_resp.has_error())
    }

    fn build_create_account_req(dedup_id: &str, account_id: &str) -> CreateAccountRequest {
        let mut req = CreateAccountRequest::default();
        req.mut_header().set_account_id(account_id.to_string());
        req.mut_header().set_dedup_id(dedup_id.to_string());
        req
    }

    fn build_lock_account_req(dedup_id: &str, account_id: &str) -> LockAccountRequest {
        let mut req = LockAccountRequest::default();
        req.mut_header().set_account_id(account_id.to_string());
        req.mut_header().set_dedup_id(dedup_id.to_string());
        req
    }

    fn build_unlock_account_req(dedup_id: &str, account_id: &str) -> UnlockAccountRequest {
        let mut req = UnlockAccountRequest::default();
        req.mut_header().set_account_id(account_id.to_string());
        req.mut_header().set_dedup_id(dedup_id.to_string());
        req
    }

    fn build_delete_account_req(dedup_id: &str, account_id: &str) -> DeleteAccountRequest {
        let mut req = DeleteAccountRequest::default();
        req.mut_header().set_account_id(account_id.to_string());
        req.mut_header().set_dedup_id(dedup_id.to_string());
        req
    }

    fn build_update_account_req(
        dedup_id: &str,
        account_id: &str,
        lower_limit: &str,
    ) -> UpdateAccountConfigRequest {
        let mut req = UpdateAccountConfigRequest::default();
        req.mut_header().set_account_id(account_id.to_string());
        req.mut_header().set_dedup_id(dedup_id.to_string());
        req.mut_balance_limit().set_lower(lower_limit.to_string());
        req
    }

    fn build_get_account_req(account_id: &str) -> GetAccountRequest {
        let mut req = GetAccountRequest::default();
        req.set_account_id(account_id.to_string());
        req
    }

    fn create_account(
        account_management_service: &AccountManagementServiceClient,
        account_id: &str,
    ) -> CreateAccountResponse {
        let create_account_req = build_create_account_req(&random_string(), account_id);
        account_management_service
            .create_account(&create_account_req)
            .unwrap()
    }

    fn lock_account(
        account_management_service: &AccountManagementServiceClient,
        account_id: &str,
    ) -> LockAccountResponse {
        let req = build_lock_account_req(&random_string(), account_id);
        account_management_service.lock_account(&req).unwrap()
    }

    fn unlock_account(
        account_management_service: &AccountManagementServiceClient,
        account_id: &str,
    ) -> UnlockAccountResponse {
        let req = build_unlock_account_req(&random_string(), account_id);
        account_management_service.unlock_account(&req).unwrap()
    }

    fn delete_account(
        account_management_service: &AccountManagementServiceClient,
        account_id: &str,
    ) -> DeleteAccountResponse {
        let req = build_delete_account_req(&random_string(), account_id);
        account_management_service.delete_account(&req).unwrap()
    }

    fn get_account(
        account_management_service: &AccountManagementServiceClient,
        account_id: &str,
    ) -> GetAccountResponse {
        let req = build_get_account_req(account_id);
        account_management_service.get_account(&req).unwrap()
    }

    fn update_account(
        account_management_service: &AccountManagementServiceClient,
        account_id: &str,
        lower_limit: &str,
    ) -> UpdateAccountConfigResponse {
        let req = build_update_account_req(&random_string(), account_id, lower_limit);
        account_management_service
            .update_account_config(&req)
            .unwrap()
    }

    fn increase_balance(
        balance_operation_client: &BalanceOperationServiceClient,
        account_id: &str,
        amount: &str,
    ) {
        let mut transfer_spec = BatchBalanceOperationRequest_BalanceOperationSpec::default();
        transfer_spec.set_amount(amount.to_string());
        transfer_spec.set_account_id(account_id.to_string());
        let batch_balance_operation_req =
            build_batch_balance_operation_req(&random_string(), vec![transfer_spec]);
        balance_operation_client
            .batch_balance_operation(&batch_balance_operation_req)
            .unwrap();
        info!("Increased {} for account_id: {}", amount, account_id);
    }

    fn build_query_balance_req(account: &str) -> QueryBalanceRequest {
        let mut query_balance_req = QueryBalanceRequest::default();
        query_balance_req.set_account_id(account.clone().to_string());
        query_balance_req
    }

    fn build_query_events_req() -> QueryEventsRequest {
        let mut query_events_req = QueryEventsRequest::default();
        query_events_req.first_seq_num = 1;
        query_events_req.last_seq_num = 10;
        query_events_req
    }

    fn verify_gc(
        balance_operation_service_client: &BalanceOperationServiceClient,
        internal_service_client: &InternalServiceClient,
        gc_limit: u64,
    ) {
        let amount = "20.1";
        let dummy_to_account: &str = "dummy_to_account";
        let dummy_from_account: &str = "dummy_from_account";

        for i in 1..gc_limit {
            let dedup_id = format!("dummy_dedup_id_{}", i);
            let transfer_req =
                build_transfer_req(amount, dummy_to_account, dummy_from_account, &dedup_id);
            let _ = balance_operation_service_client
                .transfer(&transfer_req)
                .unwrap();
        }

        info!("Sleep for 3 sec to wait the entries to be synced to db...");
        thread::sleep(Duration::from_secs(3));

        let last_query_events_req = build_query_events_req();
        let last_query_events_resp = internal_service_client
            .query_events(&last_query_events_req)
            .unwrap();
        info!("Got query_events_resp: {:?}", last_query_events_resp);

        assert!(last_query_events_resp.has_error());
        assert!(last_query_events_resp.persisted_first_seq_num > 0);
        info!(
            "GC has been happened, since the persisted_first_seq_num has been updated to {}",
            last_query_events_resp.persisted_first_seq_num
        );
    }

    fn random_string() -> String {
        let s: String = rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(7)
            .map(char::from)
            .collect();
        s
    }
}
