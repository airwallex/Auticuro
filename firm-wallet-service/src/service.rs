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
use crate::config::WalletServiceConfig;
use crate::errors::account_management_error::Error as AccountManagementError;
use crate::errors::balance_operation_error::Error as BalanceOperationError;
use crate::errors::common_error::Error as CommonError;
use crate::errors::general_error::GeneralResult;
use crate::errors::internal_error::{Error as InternalError, Result as InternalResult};
use crate::internal::{Command, CommandPayload};
use crate::message_broker::{InflightRequest, Position};
use crate::metrics::*;
use crate::rocksdb_state::{
    RocksDBState, SequenceNumber, PERSISTED_FIRST_SEQ_NUM, PERSISTED_LAST_SEQ_NUM,
};
use crate::state_machine::StateMachine;
use futures::channel::oneshot;
use futures::TryFutureExt;
use grpcio::{RpcContext, UnarySink};
use hologram_kv::message_queue_adapter::MessageQueueAdapter;
use hologram_protos::firm_walletpb::account_management_servicepb::*;
use hologram_protos::firm_walletpb::account_management_servicepb_grpc::AccountManagementService;
use hologram_protos::firm_walletpb::balance_operation_servicepb::*;
use hologram_protos::firm_walletpb::internal_servicepb::InternalService;
use hologram_protos::firm_walletpb::internal_servicepb::{
    Event, QueryEventsRequest, QueryEventsResponse,
};
use protobuf::Message;
use std::cmp::min;
use std::str;
use std::sync::{mpsc, Arc, RwLock};
use tikv_util::time::{duration_to_sec, Instant};
use tikv_util::{debug, error, info, trace};

/**
 * Grpc Service
 */
/// Service handles the RPC messages for the `Tikv` service.
#[derive(Clone)]
pub struct WalletService {
    message_queue: Arc<MessageQueueAdapter>,
    state_machine: Arc<RwLock<StateMachine>>,
    config: WalletServiceConfig,
    inflight_request_tx: mpsc::Sender<InflightRequest>,
}

impl WalletService {
    /// Constructs a new `Service` which provides the `Tikv` service.
    pub fn new(
        message_queue: Arc<MessageQueueAdapter>,
        state_machine: Arc<RwLock<StateMachine>>,
        config: WalletServiceConfig,
        inflight_request_tx: mpsc::Sender<InflightRequest>,
    ) -> Self {
        WalletService {
            message_queue,
            state_machine,
            config,
            inflight_request_tx,
        }
    }

    async fn wait_for_response(
        log_index: u64,
        offset: u64,
        inflight_request_tx: mpsc::Sender<InflightRequest>,
        command_id: &str,
    ) -> GeneralResult<Event> {
        let (oneshot_tx, oneshot_rx) = oneshot::channel();
        let send_time = Instant::now();
        let inflight_request = InflightRequest {
            position: Position { log_index, offset },
            resp_to: oneshot_tx,
            send_time,
        };

        trace!(
            "registering position for log_index={}, offset={}",
            log_index,
            offset
        );

        if let Err(e) = inflight_request_tx.send(inflight_request) {
            panic!(
                "WalletService::wait_for_response() encountered error: {} when trying to send \
            request at Position(log_index={}, offset={}) to message broker",
                e, log_index, offset
            );
        }

        let inflight_event = oneshot_rx.await.unwrap();
        trace!("GRPC recv inflight_event: {:?}", inflight_event);
        if let Ok(event) = &inflight_event.event {
            if event.get_header().get_command_id() != command_id {
                error!(
                    "Found inconsistent req(command_id = {}) and resp event: {:?}",
                    command_id, event
                );
                std::process::exit(1);
            }
        }
        inflight_event.event
    }
}

impl InternalService for WalletService {
    fn query_events(
        &mut self,
        ctx: RpcContext<'_>,
        request: QueryEventsRequest,
        sink: UnarySink<QueryEventsResponse>,
    ) {
        let t = Instant::now_coarse();
        let rocksdb_state;
        {
            // RocksDB is thread-safe for concurrent read/write.
            let guard = self.state_machine.read().unwrap();
            rocksdb_state = guard.rocksdb_state.clone();
        }

        let first_seq_num = request.get_first_seq_num();
        let last_seq_num = request.get_last_seq_num();
        let persisted_last_seq_num = rocksdb_state.read_hard_state_from_db(PERSISTED_LAST_SEQ_NUM);
        let persisted_first_seq_num =
            rocksdb_state.read_hard_state_from_db(PERSISTED_FIRST_SEQ_NUM);

        let mut response = QueryEventsResponse::default();
        response.set_persisted_first_seq_num(persisted_first_seq_num);
        response.set_persisted_last_seq_num(persisted_last_seq_num);

        match WalletService::query_events_impl(
            rocksdb_state,
            first_seq_num,
            last_seq_num,
            persisted_first_seq_num,
            persisted_last_seq_num,
            self.config.query_events_max_count,
        ) {
            Err(err) => {
                response.set_error(err.into_proto());
            }
            Ok((first_seq_num, last_seq_num, events)) => {
                response.set_first_seq_num(first_seq_num);
                response.set_last_seq_num(last_seq_num);
                response.set_events(events.into());
            }
        }

        info!(
            "Wallet.QueryEvents() is called, elapsed: {:?}, events range: [{},{}], events num {},\
                 persisted events range: [{},{}]",
            t.elapsed(),
            response.get_first_seq_num(),
            response.get_last_seq_num(),
            response.get_events().len(),
            response.get_persisted_first_seq_num(),
            response.get_persisted_last_seq_num()
        );

        ctx.spawn(
            sink.success(response)
                .unwrap_or_else(|e| error!("Wallet.QueryEvents() is failed, errors {:?}", e)),
        );
    }
}

impl WalletService {
    fn query_events_impl(
        rocksdb_state: RocksDBState,
        first_seq_num: u64,
        last_seq_num: u64,
        persisted_first_seq_num: u64,
        persisted_last_seq_num: u64,
        query_events_max_count: u64,
    ) -> InternalResult<(u64, u64, Vec<Event>)> {
        let (adjusted_first_seq_num, adjusted_last_seq_num) =
            WalletService::maybe_adjust_query_range(
                first_seq_num,
                last_seq_num,
                persisted_first_seq_num,
                persisted_last_seq_num,
                query_events_max_count,
            )?;

        let mut events = vec![];
        let mut seq_num = adjusted_first_seq_num;

        while seq_num <= adjusted_last_seq_num {
            match rocksdb_state.read_event_from_db(SequenceNumber(seq_num)) {
                None => {
                    return Err(InternalError::LogTruncated);
                }
                Some(event) => {
                    assert_eq!(seq_num, event.get_header().get_seq_num());
                    events.push(event);
                    seq_num += 1;
                }
            }
        }

        assert_eq!(seq_num - 1, adjusted_last_seq_num);
        Ok((adjusted_first_seq_num, adjusted_last_seq_num, events))
    }

    fn maybe_adjust_query_range(
        first_seq_num: u64,
        last_seq_num: u64,
        persisted_first_seq_num: u64,
        persisted_last_seq_num: u64,
        query_events_max_count: u64,
    ) -> InternalResult<(u64, u64)> {
        if first_seq_num != 0
            && first_seq_num <= last_seq_num
            && first_seq_num >= persisted_first_seq_num
            && first_seq_num <= persisted_last_seq_num
        {
            // trim the query range to be within count limitation and persisted last seq num
            let last_available_seq_num = min(
                min(last_seq_num, first_seq_num + query_events_max_count - 1),
                persisted_last_seq_num,
            );

            return Ok((first_seq_num, last_available_seq_num));
        }

        let reason = {
            if persisted_first_seq_num == 0 && persisted_last_seq_num == 0 {
                format!("No available events on server.")
            } else {
                format!(
                    "Request's first_seq_num {0} should be within server's available \
                        range of [{2}-{3}], and less equal to last_seq_num {1}.",
                    first_seq_num, last_seq_num, persisted_first_seq_num, persisted_last_seq_num
                )
            }
        };

        return Err(InternalError::OtherError { reason });
    }
}

impl AccountManagementService for WalletService {
    fn create_account(
        &mut self,
        ctx: RpcContext<'_>,
        request: CreateAccountRequest,
        sink: UnarySink<CreateAccountResponse>,
    ) {
        let begin_instant = Instant::now_coarse();
        let message_queue = self.message_queue.clone();
        let inflight_request_tx = self.inflight_request_tx.clone();
        let task = async move {
            let command = Command::from_payload(CommandPayload::CreateAccountRequest(request));
            let result = message_queue
                .send_payload(command.to_proto().write_to_bytes().unwrap())
                .await;
            debug!(
                "Handling create_account RPC, send_payload replied with {:?}",
                result
            );

            let mut response = CreateAccountResponse::default();
            match result {
                Ok((log_index, offset)) => {
                    let resp = WalletService::wait_for_response(
                        log_index,
                        offset,
                        inflight_request_tx,
                        command.command_id(),
                    )
                    .await;

                    match resp {
                        Ok(event) => {
                            response = event
                                .get_payload()
                                .get_account_created_event()
                                .get_response()
                                .clone();
                        }
                        Err(err) => {
                            response.set_error(err.into_proto());
                        }
                    }
                }
                Err(e) => {
                    response.set_error(CommonError::MessageQueueError(e).into_proto());
                }
            }

            info!(
                "Wallet.create_account() is called, elapsed: {:?}, response: {:?}",
                begin_instant.elapsed(),
                response
            );
            sink.success(response).await.unwrap()
        };

        ctx.spawn(task)
    }

    fn lock_account(
        &mut self,
        ctx: RpcContext<'_>,
        request: LockAccountRequest,
        sink: UnarySink<LockAccountResponse>,
    ) {
        let begin_instant = Instant::now_coarse();
        let message_queue = self.message_queue.clone();
        let inflight_request_tx = self.inflight_request_tx.clone();
        let task = async move {
            let command = Command::from_payload(CommandPayload::LockAccountRequest(request));
            let result = message_queue
                .send_payload(command.to_proto().write_to_bytes().unwrap())
                .await;
            debug!(
                "Handling lock_account RPC, send_payload replied with {:?}",
                result
            );

            let mut response = LockAccountResponse::default();
            match result {
                Ok((log_index, offset)) => {
                    let resp = WalletService::wait_for_response(
                        log_index,
                        offset,
                        inflight_request_tx,
                        command.command_id(),
                    )
                    .await;

                    match resp {
                        Ok(event) => {
                            response = event
                                .get_payload()
                                .get_account_locked_event()
                                .get_response()
                                .clone();
                        }
                        Err(err) => {
                            response.set_error(err.into_proto());
                        }
                    }
                }
                Err(e) => {
                    response.set_error(CommonError::MessageQueueError(e).into_proto());
                }
            }

            info!(
                "Wallet.lock_account() is called, elapsed: {:?}, response: {:?}",
                begin_instant.elapsed(),
                response
            );
            sink.success(response).await.unwrap()
        };

        ctx.spawn(task)
    }

    fn unlock_account(
        &mut self,
        ctx: RpcContext<'_>,
        request: UnlockAccountRequest,
        sink: UnarySink<UnlockAccountResponse>,
    ) {
        let begin_instant = Instant::now_coarse();
        let message_queue = self.message_queue.clone();
        let inflight_request_tx = self.inflight_request_tx.clone();
        let task = async move {
            let command = Command::from_payload(CommandPayload::UnlockAccountRequest(request));
            let result = message_queue
                .send_payload(command.to_proto().write_to_bytes().unwrap())
                .await;
            debug!(
                "Handling unlock_account RPC,send_payload replied with {:?}",
                result
            );

            let mut response = UnlockAccountResponse::default();
            match result {
                Ok((log_index, offset)) => {
                    let resp = WalletService::wait_for_response(
                        log_index,
                        offset,
                        inflight_request_tx,
                        command.command_id(),
                    )
                    .await;

                    match resp {
                        Ok(event) => {
                            response = event
                                .get_payload()
                                .get_account_unlock_event()
                                .get_response()
                                .clone();
                        }
                        Err(err) => {
                            response.set_error(err.into_proto());
                        }
                    }
                }
                Err(e) => {
                    response.set_error(CommonError::MessageQueueError(e).into_proto());
                }
            }

            info!(
                "Wallet.unlock_account() is called, elapsed: {:?}, response: {:?}",
                begin_instant.elapsed(),
                response
            );
            sink.success(response).await.unwrap()
        };

        ctx.spawn(task)
    }

    fn delete_account(
        &mut self,
        ctx: RpcContext<'_>,
        request: DeleteAccountRequest,
        sink: UnarySink<DeleteAccountResponse>,
    ) {
        let begin_instant = Instant::now_coarse();
        let message_queue = self.message_queue.clone();
        let inflight_request_tx = self.inflight_request_tx.clone();
        let task = async move {
            let command = Command::from_payload(CommandPayload::DeleteAccountRequest(request));
            let result = message_queue
                .send_payload(command.to_proto().write_to_bytes().unwrap())
                .await;
            debug!(
                "Handling delete_account RPC, send_payload replied with {:?}",
                result
            );

            let mut response = DeleteAccountResponse::default();
            match result {
                Ok((log_index, offset)) => {
                    let resp = WalletService::wait_for_response(
                        log_index,
                        offset,
                        inflight_request_tx,
                        command.command_id(),
                    )
                    .await;

                    match resp {
                        Ok(event) => {
                            response = event
                                .get_payload()
                                .get_account_deleted_event()
                                .get_response()
                                .clone();
                        }
                        Err(err) => {
                            response.set_error(err.into_proto());
                        }
                    }
                }
                Err(e) => {
                    response.set_error(CommonError::MessageQueueError(e).into_proto());
                }
            }

            info!(
                "Wallet.delete_account() is called, elapsed: {:?}, response: {:?}",
                begin_instant.elapsed(),
                response
            );
            sink.success(response).await.unwrap()
        };

        ctx.spawn(task)
    }

    fn update_account_config(
        &mut self,
        ctx: RpcContext,
        request: UpdateAccountConfigRequest,
        sink: UnarySink<UpdateAccountConfigResponse>,
    ) {
        let begin_instant = Instant::now_coarse();
        let message_queue = self.message_queue.clone();
        let inflight_request_tx = self.inflight_request_tx.clone();
        let task = async move {
            let command =
                Command::from_payload(CommandPayload::UpdateAccountConfigRequest(request));
            let result = message_queue
                .send_payload(command.to_proto().write_to_bytes().unwrap())
                .await;
            debug!(
                "Handling delete_account RPC, send_payload replied with {:?}",
                result
            );

            let mut response = UpdateAccountConfigResponse::default();
            match result {
                Ok((log_index, offset)) => {
                    let resp = WalletService::wait_for_response(
                        log_index,
                        offset,
                        inflight_request_tx,
                        command.command_id(),
                    )
                    .await;

                    match resp {
                        Ok(event) => {
                            response = event
                                .get_payload()
                                .get_account_config_update_event()
                                .get_response()
                                .clone();
                        }
                        Err(err) => {
                            response.set_error(err.into_proto());
                        }
                    }
                }
                Err(e) => {
                    response.set_error(CommonError::MessageQueueError(e).into_proto());
                }
            }

            info!(
                "Wallet.update_account_config() is called, elapsed: {:?}, response: {:?}",
                begin_instant.elapsed(),
                response
            );
            sink.success(response).await.unwrap()
        };

        ctx.spawn(task)
    }

    fn get_account(
        &mut self,
        ctx: RpcContext<'_>,
        request: GetAccountRequest,
        sink: UnarySink<GetAccountResponse>,
    ) {
        let t = Instant::now_coarse();

        let mut response = GetAccountResponse::default();

        {
            let guard = self.state_machine.read().unwrap();
            if request.get_seq_num() > guard.last_seq_num {
                let error = CommonError::DataNotReady {
                    requested_seq_num: request.get_seq_num(),
                    available_seq_num: guard.last_seq_num,
                };

                response.set_error(error.into_proto());
            } else {
                response.set_seq_num(guard.last_seq_num);

                if let Some(account) = guard.accounts.get(request.get_account_id()) {
                    response.set_account(account.to_proto());
                } else {
                    let error = AccountManagementError::AccountNotExist(
                        request.get_account_id().to_string(),
                    );
                    response.set_error(error.into_proto());
                }
            }
        }

        debug!(
            "Wallet.QueryBalance() is called, elapsed: {:?}, response: {:?}",
            t.elapsed(),
            response
        );

        ctx.spawn(
            sink.success(response)
                .unwrap_or_else(|e| error!("Wallet.QueryBalance() is failed, errors {:?}", e)),
        );
    }
}

impl BalanceOperationService for WalletService {
    fn reserve(&mut self, ctx: RpcContext, req: ReserveRequest, sink: UnarySink<ReserveResponse>) {
        let begin_instant = Instant::now_coarse();
        let message_queue = self.message_queue.clone();
        let inflight_request_tx = self.inflight_request_tx.clone();
        let task = async move {
            let command = Command::from_payload(CommandPayload::ReserveRequest(req));
            let result = message_queue
                .send_payload(command.to_proto().write_to_bytes().unwrap())
                .await;
            debug!(
                "BalanceOperationService on reserve RPC,send_payload replied with {:?}",
                result
            );

            let mut response = ReserveResponse::default();
            match result {
                Ok((log_index, offset)) => {
                    let resp = WalletService::wait_for_response(
                        log_index,
                        offset,
                        inflight_request_tx,
                        command.command_id(),
                    )
                    .await;

                    LATENCY_HISTOGRAM_STATIC
                        .reserve
                        .observe(duration_to_sec(begin_instant.elapsed()));

                    match resp {
                        Ok(event) => {
                            response = event
                                .get_payload()
                                .get_reserve_event()
                                .get_response()
                                .clone();
                        }
                        Err(err) => {
                            response.set_error(err.into_proto());
                        }
                    }
                }
                Err(e) => {
                    LATENCY_HISTOGRAM_STATIC
                        .reserve
                        .observe(duration_to_sec(begin_instant.elapsed()));

                    response.set_error(CommonError::MessageQueueError(e).into_proto());
                }
            }

            info!(
                "Wallet.reserve() is called, elapsed: {:?}, response: {:?}",
                begin_instant.elapsed(),
                response
            );
            sink.success(response).await.unwrap();
        };

        ctx.spawn(task)
    }

    fn release(&mut self, ctx: RpcContext, req: ReleaseRequest, sink: UnarySink<ReleaseResponse>) {
        let begin_instant = Instant::now_coarse();
        let message_queue = self.message_queue.clone();
        let inflight_request_tx = self.inflight_request_tx.clone();
        let task = async move {
            let command = Command::from_payload(CommandPayload::ReleaseRequest(req));
            let result = message_queue
                .send_payload(command.to_proto().write_to_bytes().unwrap())
                .await;
            debug!(
                "BalanceOperationService on release RPC,send_payload replied with {:?}",
                result
            );

            let mut response = ReleaseResponse::default();
            match result {
                Ok((log_index, offset)) => {
                    let resp = WalletService::wait_for_response(
                        log_index,
                        offset,
                        inflight_request_tx,
                        command.command_id(),
                    )
                    .await;

                    LATENCY_HISTOGRAM_STATIC
                        .release
                        .observe(duration_to_sec(begin_instant.elapsed()));

                    match resp {
                        Ok(event) => {
                            response = event
                                .get_payload()
                                .get_release_event()
                                .get_response()
                                .clone();
                        }
                        Err(err) => {
                            response.set_error(err.into_proto());
                        }
                    }
                }
                Err(e) => {
                    LATENCY_HISTOGRAM_STATIC
                        .release
                        .observe(duration_to_sec(begin_instant.elapsed()));

                    response.set_error(CommonError::MessageQueueError(e).into_proto());
                }
            }

            info!(
                "Wallet.release() is called, elapsed: {:?}, response: {:?}",
                begin_instant.elapsed(),
                response
            );
            sink.success(response).await.unwrap();
        };

        ctx.spawn(task);
    }

    fn batch_balance_operation(
        &mut self,
        ctx: RpcContext,
        req: BatchBalanceOperationRequest,
        sink: UnarySink<BatchBalanceOperationResponse>,
    ) {
        let begin_instant = Instant::now_coarse();
        let message_queue = self.message_queue.clone();
        let inflight_request_tx = self.inflight_request_tx.clone();
        let task = async move {
            let command = Command::from_payload(CommandPayload::BatchBalanceOperationRequest(req));
            let result = message_queue
                .send_payload(command.to_proto().write_to_bytes().unwrap())
                .await;
            debug!("BalanceOperationService on batch_balance_operation RPC,send_payload replied with {:?}", result);

            let mut response = BatchBalanceOperationResponse::default();
            match result {
                Ok((log_index, offset)) => {
                    let resp = WalletService::wait_for_response(
                        log_index,
                        offset,
                        inflight_request_tx,
                        command.command_id(),
                    )
                    .await;

                    LATENCY_HISTOGRAM_STATIC
                        .batch_balance_operation
                        .observe(duration_to_sec(begin_instant.elapsed()));

                    match resp {
                        Ok(event) => {
                            response = event
                                .get_payload()
                                .get_batch_balance_operation_event()
                                .get_response()
                                .clone();
                        }
                        Err(err) => {
                            response.set_error(err.into_proto());
                        }
                    }
                }
                Err(e) => {
                    LATENCY_HISTOGRAM_STATIC
                        .batch_balance_operation
                        .observe(duration_to_sec(begin_instant.elapsed()));

                    response.set_error(CommonError::MessageQueueError(e).into_proto());
                }
            }

            info!(
                "Wallet.batch_balance_operation() is called, elapsed: {:?}, response: {:?}",
                begin_instant.elapsed(),
                response
            );
            sink.success(response).await.unwrap()
        };

        ctx.spawn(task);
    }

    fn transfer(
        &mut self,
        ctx: RpcContext<'_>,
        req: TransferRequest,
        sink: UnarySink<TransferResponse>,
    ) {
        let begin_instant = Instant::now_coarse();
        let message_queue = self.message_queue.clone();
        let inflight_request_tx = self.inflight_request_tx.clone();
        let task = async move {
            let command = Command::from_payload(CommandPayload::TransferRequest(req));
            let result = message_queue
                .send_payload(command.to_proto().write_to_bytes().unwrap())
                .await;
            debug!(
                "BalanceOperationService on transfer RPC,send_payload replied with {:?}",
                result
            );

            let mut response = TransferResponse::default();
            match result {
                Ok((log_index, offset)) => {
                    let resp = WalletService::wait_for_response(
                        log_index,
                        offset,
                        inflight_request_tx,
                        command.command_id(),
                    )
                    .await;

                    LATENCY_HISTOGRAM_STATIC
                        .transfer
                        .observe(duration_to_sec(begin_instant.elapsed()));

                    match resp {
                        Ok(event) => {
                            response = event
                                .get_payload()
                                .get_transfer_event()
                                .get_response()
                                .clone();
                        }
                        Err(err) => {
                            response.set_error(err.into_proto());
                        }
                    }
                }
                Err(e) => {
                    LATENCY_HISTOGRAM_STATIC
                        .transfer
                        .observe(duration_to_sec(begin_instant.elapsed()));

                    let error = CommonError::MessageQueueError(e);
                    response.set_error(error.into_proto());
                }
            }

            info!(
                "Wallet.transfer() is called, elapsed: {:?}, response: {:?}",
                begin_instant.elapsed(),
                response
            );
            sink.success(response).await.unwrap();
        };

        ctx.spawn(task);
    }

    fn query_balance(
        &mut self,
        ctx: RpcContext<'_>,
        request: QueryBalanceRequest,
        sink: UnarySink<QueryBalanceResponse>,
    ) {
        let t = Instant::now_coarse();

        let mut response = QueryBalanceResponse::default();
        response.set_account_id(request.get_account_id().to_string());

        {
            let guard = self.state_machine.read().unwrap();
            if request.get_seq_num() > guard.last_seq_num {
                let error = CommonError::DataNotReady {
                    requested_seq_num: request.get_seq_num(),
                    available_seq_num: guard.last_seq_num,
                };
                response.set_error(error.into_proto());
            } else {
                response.set_seq_num(guard.last_seq_num);

                if let Some(account) = guard.accounts.get(request.get_account_id()) {
                    response.set_balance(account.0.get_balance().clone());
                } else {
                    let error = BalanceOperationError::AccountNotExist(
                        request.get_account_id().to_string(),
                    );
                    response.set_error(error.into_proto());
                }
            }
        }

        info!(
            "Wallet.QueryBalance() is called, elapsed: {:?}, response: {:?}",
            t.elapsed(),
            response
        );

        ctx.spawn(
            sink.success(response)
                .unwrap_or_else(|e| error!("Wallet.QueryBalance() is failed, errors {:?}", e)),
        );
    }
}
