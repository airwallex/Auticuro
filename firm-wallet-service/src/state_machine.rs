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
use crate::account::Account;
use crate::balance_calculator::BalanceCalculator;
use crate::errors::account_management_error::Error as AccountManagementError;
use crate::errors::general_error::GeneralResult;
use crate::internal::{assert_command_and_event_match, Command, Event, Response};
use crate::message_broker::{InflightEvent, Position};
use crate::metrics::*;
use crate::rocksdb_state::{RocksDBState, SequenceNumber, APPLIED_INDEX, PERSISTED_LAST_SEQ_NUM};
use crate::utils;
use byteorder::{BigEndian, ByteOrder};
use hologram_kv::keys;
use hologram_kv::message_queue_adapter::{
    parse_log_entry_to_proto, LogConsumer, LogEntryReceiver, MessageQueueAdapter,
};
use hologram_kv::rocks_engine::RocksWriteBatch;
use hologram_protos::firm_walletpb::account_management_servicepb::*;
use hologram_protos::firm_walletpb::balance_operation_servicepb::*;
use hologram_protos::firm_walletpb::internal_servicepb::CommandPayload_oneof_request::{
    batch_balance_operation_request, create_account_request, delete_account_request,
    lock_account_request, release_request, reserve_request, transfer_request,
    unlock_account_request, update_account_config_request,
};
use hologram_protos::firm_walletpb::internal_servicepb::{Command as CommandPb, Event as EventPb};
use protobuf::Message;
use std::collections::HashMap;
use std::sync::{mpsc, Arc, RwLock};
use std::{mem, str};
use tikv_util::time::{duration_to_ms, duration_to_sec, Instant};
use tikv_util::{debug, trace, warn};

/**
 * State Machine
 */
pub struct StateMachine {
    // account_id -> balance
    // The whole accounts map, recovered from DB during startup.
    pub(crate) accounts: HashMap<String, Account>,

    // log_index of the latest applied entry.
    applied_index: u64,

    // last event entry's seq_num, used by downstream to verify data integrity of event stream.
    // last_seq_num == PERSISTED_LAST_SEQ_NUM + num_of_in_mem_events
    pub(crate) last_seq_num: u64,

    // Todo Need dedup_id -> seq_nums since Cmd: Event is 1:N
    // dedup_id -> seq_num
    seq_num_cache: HashMap<String, SequenceNumber>,

    // seq_num -> Event
    event_cache: HashMap<SequenceNumber, EventPb>,

    // wb: RocksWriteBatch,
    pub(crate) rocksdb_state: RocksDBState,

    balance_calculator: BalanceCalculator,
}

impl StateMachine {
    pub fn recover_from_db(
        rocksdb_state: RocksDBState,
        inflight_event_tx: mpsc::Sender<InflightEvent>,
    ) -> (Self, ApplyContext) {
        let accounts = rocksdb_state.read_accounts_from_db();
        let applied_index = rocksdb_state.read_hard_state_from_db(APPLIED_INDEX);
        let persisted_last_seq_num = rocksdb_state.read_hard_state_from_db(PERSISTED_LAST_SEQ_NUM);

        let context = ApplyContext::new(
            RocksWriteBatch::new(rocksdb_state.get_db().as_inner().clone()),
            applied_index,
            inflight_event_tx,
        );
        let state_machine = StateMachine {
            accounts,
            applied_index,
            last_seq_num: persisted_last_seq_num,
            seq_num_cache: HashMap::new(),
            event_cache: HashMap::new(),
            rocksdb_state,
            balance_calculator: BalanceCalculator {},
        };
        (state_machine, context)
    }

    pub fn get_event(&self, seq_num: SequenceNumber) -> Option<EventPb> {
        if let Some(event) = self.event_cache.get(&seq_num) {
            return Some(event.clone());
        }

        self.rocksdb_state.read_event_from_db(seq_num)
    }

    // search cache, if not found, search disk.
    pub fn get_seq_num(&self, command_id: &str) -> Option<SequenceNumber> {
        if let Some(&seq_num) = self.seq_num_cache.get(command_id) {
            return Some(seq_num);
        }

        self.rocksdb_state.read_seq_num_from_db(command_id)
    }

    pub fn apply_entry(&mut self, context: &mut ApplyContext, entry: (u64, Vec<CommandPb>)) {
        let (log_index, commands) = entry;
        assert_eq!(self.applied_index + 1, log_index);

        APPLY_COMMAND_COUNTER.inc_by(commands.len() as u64);
        for (offset, command_pb) in commands.into_iter().enumerate() {
            let begin_instant = Instant::now_coarse();

            let command = Command::from_proto(command_pb);

            self.handle_command(context, log_index, offset as u64, command);

            CRITICAL_EVENT_HISTOGRAM_STATIC
                .handle_command
                .observe(duration_to_sec(begin_instant.elapsed()));
        }

        // update applied_index after already applied the entry.
        self.applied_index = log_index;

        // update context
        let mut buf = [0; 8];
        BigEndian::write_u64(&mut buf, self.applied_index);
        context.wb.put(APPLIED_INDEX.key.as_bytes(), &buf);

        BigEndian::write_u64(&mut buf, self.last_seq_num);
        context.wb.put(PERSISTED_LAST_SEQ_NUM.key.as_bytes(), &buf);
    }

    pub fn maybe_flush(&mut self, context: &mut ApplyContext) {
        if self.should_flush(context) {
            self.flush(context);
        }
    }

    fn should_flush(&self, context: &ApplyContext) -> bool {
        if self.applied_index - context.flushed_applied_index >= context.batch_size {
            // too many updates cached in memory
            true
        } else if self.applied_index > context.flushed_applied_index
            && duration_to_ms(context.flushed_timestamp.elapsed()) > 100
        {
            // periodically flush in-memory state
            debug!(
                "periodically flush in-memory state, applied_index {}, \
                flushed_applied_index {}, idle time {:?}",
                self.applied_index,
                context.flushed_applied_index,
                context.flushed_timestamp.elapsed()
            );
            true
        } else {
            false
        }
    }

    fn flush(&mut self, context: &mut ApplyContext) {
        assert!(self.applied_index > context.flushed_applied_index);
        let begin_instant = Instant::now_coarse();

        context.wb.write_opt(false); // we don't need fsync write here.
        context.wb.clear();

        // The Deleted Accounts are just marked as Deleted before the WriteBatch
        // is flushed into RocksDB. This is where we finally remove these tombstones.
        for to_be_delete_account_id in mem::take(&mut context.to_be_deleted_account_ids) {
            self.accounts.remove(&to_be_delete_account_id);
        }

        // clear cache
        self.event_cache.clear();
        self.seq_num_cache.clear();

        debug!(
            "Flush modifications between [{}, {}], cost {:?}.",
            context.flushed_applied_index + 1,
            self.applied_index,
            begin_instant.elapsed()
        );

        context.flushed_applied_index = self.applied_index;
        context.flushed_timestamp = Instant::now_coarse();
    }

    fn handle_command(
        &mut self,
        context: &mut ApplyContext,
        log_index: u64,
        offset: u64,
        command: Command,
    ) {
        let position = Position { log_index, offset };
        if let Some(event) = self.maybe_handle_duplicate_command(&command) {
            warn!("Command(id={}) is duplicate", command.command_id());
            context.send_event(position, event);
            return;
        }

        // At least one clone() is needed because request ownership is needed by the Event proto
        let res = match command.payload().clone() {
            transfer_request(request) => self.handle_transfer(context, request, log_index, offset),
            reserve_request(request) => self.handle_reserve(context, request, log_index, offset),
            release_request(request) => self.handle_release(context, request, log_index, offset),
            batch_balance_operation_request(request) => {
                self.handle_batch_balance_operation(context, request, log_index, offset)
            }

            create_account_request(request) => {
                self.handle_create_account(context, request, log_index, offset)
            }
            lock_account_request(request) => {
                self.handle_lock_account(context, request, log_index, offset)
            }
            unlock_account_request(request) => {
                self.handle_unlock_account(context, request, log_index, offset)
            }
            delete_account_request(request) => {
                self.handle_delete_account(context, request, log_index, offset)
            }
            update_account_config_request(request) => {
                self.handle_update_account_config(context, request, log_index, offset)
            }
        };

        match res {
            Err(e) => {
                context.send_event(position, Err(e));
            }
            Ok(mut event) => {
                event.set_header(
                    command.header().clone(),
                    log_index,
                    offset,
                    self.last_seq_num,
                );

                debug!(
                    "Command(id={}) has been handled, the generated event is {:?}",
                    command.command_id(),
                    event
                );

                let event_pb = event.to_proto();
                self.update_seq_num_and_event(context, command.command_id(), event_pb.clone());

                context.send_event(position, Ok(event_pb));
            }
        }
    }

    fn maybe_handle_duplicate_command(&self, command: &Command) -> Option<GeneralResult<EventPb>> {
        self.get_seq_num(command.command_id()).map(|seq_num| {
            // command_id -> seq_num and seq_num -> EventPb is consistent with each other
            let event = self.get_event(seq_num).unwrap();
            assert_command_and_event_match(command, event)
        })
    }

    fn update_seq_num_and_event(
        &mut self,
        context: &mut ApplyContext,
        command_id: &str,
        event: EventPb,
    ) {
        let seq_num = SequenceNumber(self.last_seq_num);

        let bytes = seq_num.to_bytes();

        let dedup_key = keys::dedup_key(command_id.as_bytes());
        context.wb.put(&dedup_key, &bytes);

        let event_key = keys::event_key(&bytes);
        context.wb.put_msg(&event_key, &event);

        self.seq_num_cache.insert(command_id.to_string(), seq_num);
        self.event_cache.insert(seq_num, event);
    }

    fn handle_transfer(
        &mut self,
        context: &mut ApplyContext,
        request: TransferRequest,
        log_index: u64,
        offset: u64,
    ) -> GeneralResult<Event> {
        let transfer_spec = request.get_transfer_spec();
        let amount: &str = transfer_spec.get_amount();

        let from_account = self.get_or_create_account(transfer_spec.get_from_account_id());
        let to_account = self.get_or_create_account(transfer_spec.get_to_account_id());

        from_account.validate_asset_class_compatibility(&to_account)?;

        let (updated_to_account, to_balance_change) =
            to_account.increase_balance_by(amount, &self.balance_calculator)?;

        let (updated_from_account, from_balance_change) = from_account.increase_balance_by(
            &self.balance_calculator.neg(amount)?,
            &self.balance_calculator,
        )?;

        self.last_seq_num += 1;

        self.update_account(context, updated_to_account);
        self.update_account(context, updated_from_account);

        let event = Response::<TransferResponse>::build_event(
            self.last_seq_num,
            log_index,
            offset,
            request,
            from_balance_change,
            to_balance_change,
        );

        trace!("handle_transfer, the event generated is {:?}", event);

        Ok(event)
    }

    fn handle_batch_balance_operation(
        &mut self,
        context: &mut ApplyContext,
        request: BatchBalanceOperationRequest,
        log_index: u64,
        offset: u64,
    ) -> GeneralResult<Event> {
        let mut balance_changes = vec![];
        let mut updated_accounts = vec![];
        for spec in request.get_balance_operation_spec() {
            let account = self.get_or_create_account(spec.get_account_id());

            let (updated_account, balance_change) =
                account.increase_balance_by(spec.get_amount(), &self.balance_calculator)?;

            balance_changes.push(balance_change);
            updated_accounts.push(updated_account);
        }

        self.last_seq_num += 1;

        for updated_account in updated_accounts {
            self.update_account(context, updated_account);
        }

        let event = Response::<BatchBalanceOperationResponse>::build_event(
            self.last_seq_num,
            log_index,
            offset,
            request,
            balance_changes,
        );
        trace!(
            "handle_batch_balance_operation, the event generated is {:?}",
            &event
        );

        Ok(event)
    }

    fn handle_reserve(
        &mut self,
        context: &mut ApplyContext,
        request: ReserveRequest,
        log_index: u64,
        offset: u64,
    ) -> GeneralResult<Event> {
        let prev_account = self.assert_account_exists(request.get_account_id())?;
        let prev_balance = prev_account.get_balance().clone();
        let curr_account = prev_account.reserve(
            request.get_reservation_id(),
            request.get_amount(),
            &self.balance_calculator,
        )?;
        let curr_balance = curr_account.get_balance().clone();

        self.last_seq_num += 1;

        self.update_account(context, curr_account);

        let event = Response::<ReserveResponse>::build_event(
            self.last_seq_num,
            log_index,
            offset,
            request,
            prev_balance,
            curr_balance,
        );

        trace!("handle_reserve, the event generated is {:?}", event);

        Ok(event)
    }

    fn handle_release(
        &mut self,
        context: &mut ApplyContext,
        request: ReleaseRequest,
        log_index: u64,
        offset: u64,
    ) -> GeneralResult<Event> {
        let prev_account = self.assert_account_exists(request.get_account_id())?;
        let prev_balance = prev_account.get_balance().clone();

        let curr_account = if request.get_amount().is_empty() {
            prev_account.release_all(request.get_reservation_id(), &self.balance_calculator)?
        } else {
            prev_account.partial_release(
                request.get_reservation_id(),
                request.get_amount(),
                &self.balance_calculator,
            )?
        };
        let curr_balance = curr_account.get_balance().clone();

        self.last_seq_num += 1;

        self.update_account(context, curr_account);

        let event = Response::<ReleaseResponse>::build_event(
            self.last_seq_num,
            log_index,
            offset,
            request,
            prev_balance,
            curr_balance,
        );

        trace!("handle_release, the event generated is {:?}", event);

        Ok(event)
    }

    fn handle_create_account(
        &mut self,
        context: &mut ApplyContext,
        request: CreateAccountRequest,
        log_index: u64,
        offset: u64,
    ) -> GeneralResult<Event> {
        let account_id = request.get_header().get_account_id();
        if self.accounts.contains_key(account_id) {
            return Err(AccountManagementError::AccountAlreadyExist(account_id.to_string()).into());
        }
        let account = Account::new(account_id);

        self.last_seq_num += 1;

        self.update_account(context, account.clone());

        let event = Response::<CreateAccountResponse>::build_event(
            self.last_seq_num,
            log_index,
            offset,
            request,
            account,
        );

        trace!("handle_create_account, the event generated is {:?}", event);

        Ok(event)
    }

    fn handle_lock_account(
        &mut self,
        context: &mut ApplyContext,
        request: LockAccountRequest,
        log_index: u64,
        offset: u64,
    ) -> GeneralResult<Event> {
        let account = self.assert_account_exists(request.get_header().get_account_id())?;
        let updated_account = account.lock()?;

        self.last_seq_num += 1;

        self.update_account(context, updated_account.clone());

        let event = Response::<LockAccountResponse>::build_event(
            self.last_seq_num,
            log_index,
            offset,
            request,
            updated_account,
        );

        trace!("handle_lock_account, the event generated is {:?}", event);

        Ok(event)
    }

    fn handle_unlock_account(
        &mut self,
        context: &mut ApplyContext,
        request: UnlockAccountRequest,
        log_index: u64,
        offset: u64,
    ) -> GeneralResult<Event> {
        let account = self.assert_account_exists(request.get_header().get_account_id())?;
        let updated_account = account.unlock()?;

        self.last_seq_num += 1;

        self.update_account(context, updated_account.clone());

        let event = Response::<UnlockAccountResponse>::build_event(
            self.last_seq_num,
            log_index,
            offset,
            request,
            updated_account,
        );

        trace!("handle_unlock_account, the event generated is {:?}", event);

        Ok(event)
    }

    fn handle_delete_account(
        &mut self,
        context: &mut ApplyContext,
        request: DeleteAccountRequest,
        log_index: u64,
        offset: u64,
    ) -> GeneralResult<Event> {
        let account = self.assert_account_exists(request.get_header().get_account_id())?;
        let updated_account = account.delete(&self.balance_calculator)?;

        self.last_seq_num += 1;

        self.delete_account(context, updated_account.clone());

        let event = Response::<DeleteAccountResponse>::build_event(
            self.last_seq_num,
            log_index,
            offset,
            request,
            updated_account,
        );

        trace!("handle_unlock_account, the event generated is {:?}", event);

        Ok(event)
    }

    fn handle_update_account_config(
        &mut self,
        context: &mut ApplyContext,
        request: UpdateAccountConfigRequest,
        log_index: u64,
        offset: u64,
    ) -> GeneralResult<Event> {
        let account = self.assert_account_exists(request.get_header().get_account_id())?;

        let updated_account = account.set_balance_limit(request.get_balance_limit());

        self.last_seq_num += 1;

        self.update_account(context, updated_account.clone());

        let event = Response::<UpdateAccountConfigResponse>::build_event(
            self.last_seq_num,
            log_index,
            offset,
            request,
            updated_account,
        );

        trace!(
            "handle_update_account_config, the event generated is {:?}",
            event
        );

        Ok(event)
    }
}

impl StateMachine {
    fn delete_account(&mut self, context: &mut ApplyContext, account: Account) {
        let account_id = account.account_id().to_string();
        let data_key = keys::data_key(account_id.as_bytes());
        context.wb.delete(&data_key);
        context.to_be_deleted_account_ids.push(account_id.clone());

        // Put the "Deleted" account back in mem
        self.accounts.insert(account_id, account);
    }

    fn update_account(&mut self, context: &mut ApplyContext, account: Account) {
        let account_id = account.account_id().to_string();
        // update context
        let data_key = keys::data_key(account_id.as_bytes());
        let account_bytes = account.to_proto().write_to_bytes().unwrap();
        context.wb.put(&data_key, &account_bytes);

        // update in-mem
        self.accounts.insert(account_id, account);
    }

    // Todo Try to avoid copy if account exists
    fn get_or_create_account(&self, account_id: &str) -> Account {
        match self.accounts.get(account_id) {
            None => Account::new(account_id),
            Some(account) => account.clone(),
        }
    }

    fn assert_account_exists(&self, account_id: &str) -> GeneralResult<&Account> {
        match self.accounts.get(account_id) {
            Some(account) => Ok(account),
            None => Err(AccountManagementError::AccountNotExist(account_id.to_string()).into()),
        }
    }
}

/**
 * Apply Loop
 */
/// ApplyContext is not Sync, since DBWriteBatch is not Sync.
pub struct ApplyContext {
    batch_size: u64,
    wb: RocksWriteBatch,
    to_be_deleted_account_ids: Vec<String>,
    flushed_applied_index: u64,
    flushed_timestamp: Instant,
    inflight_event_tx: mpsc::Sender<InflightEvent>,
}

impl ApplyContext {
    pub fn new(
        wb: RocksWriteBatch,
        applied_index: u64,
        inflight_event_tx: mpsc::Sender<InflightEvent>,
    ) -> Self {
        ApplyContext {
            batch_size: std::env::var("batch_size")
                .unwrap_or("100".to_string())
                .parse::<u64>()
                .unwrap(),
            wb,
            to_be_deleted_account_ids: vec![],
            flushed_applied_index: applied_index,
            flushed_timestamp: Instant::now_coarse(),
            inflight_event_tx,
        }
    }

    pub fn send_event(&self, position: Position, event: GeneralResult<EventPb>) {
        let inflight_event = InflightEvent { position, event };
        self.inflight_event_tx.send(inflight_event).unwrap();
    }
}

pub struct ApplyLoop {
    statemachine: Arc<RwLock<StateMachine>>,
    context: ApplyContext,
    receiver: LogEntryReceiver<CommandPb>,
    applied_index: u64,
}

impl ApplyLoop {
    pub fn new(
        statemachine: Arc<RwLock<StateMachine>>,
        context: ApplyContext,
        message_queue: Arc<MessageQueueAdapter>,
    ) -> Self {
        let applied_index = context.flushed_applied_index;
        let receiver = LogConsumer::run(
            message_queue,
            applied_index + 1,
            parse_log_entry_to_proto::<CommandPb>,
        );

        ApplyLoop {
            statemachine,
            context,
            receiver,
            applied_index,
        }
    }

    pub fn run(&mut self) {
        loop {
            let start_time = Instant::now_coarse();
            let buf = self.receiver.batch_recv(1000);

            {
                let mut guard = self.statemachine.write().unwrap();
                for log_entry in buf {
                    assert_eq!(self.applied_index + 1, log_entry.index);
                    self.applied_index = log_entry.index;
                    guard.apply_entry(&mut self.context, (log_entry.index, log_entry.messages));
                }
                guard.maybe_flush(&mut self.context);
            }

            // yield CPU, avoid busy wait
            utils::yield_cpu(start_time, 50);
        }
    }
}
