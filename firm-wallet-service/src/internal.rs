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
use crate::errors::common_error::Error as OtherError;
use crate::errors::general_error::GeneralResult;
use hologram_protos::firm_walletpb::account_management_servicepb::*;
use hologram_protos::firm_walletpb::account_management_servicepb::{
    AccountConfigUpdatedEvent, AccountCreatedEvent, AccountDeletedEvent, AccountLockedEvent,
    AccountUnlockedEvent, CreateAccountRequest, DeleteAccountRequest, LockAccountRequest,
    UnlockAccountRequest, UpdateAccountConfigRequest,
};
use hologram_protos::firm_walletpb::accountpb::Balance;
use hologram_protos::firm_walletpb::balance_operation_servicepb::*;
use hologram_protos::firm_walletpb::balance_operation_servicepb::{
    BatchBalanceOperationEvent, BatchBalanceOperationRequest, ReleaseEvent, ReleaseRequest,
    ReserveEvent, ReserveRequest, TransferEvent, TransferRequest,
};
use hologram_protos::firm_walletpb::commonpb::ResponseHeader;
use hologram_protos::firm_walletpb::internal_servicepb::CommandPayload_oneof_request::*;
use hologram_protos::firm_walletpb::internal_servicepb::EventPayload_oneof_event::*;
use hologram_protos::firm_walletpb::internal_servicepb::{
    Command as CommandPb, CommandHeader, CommandPayload as CommandPayloadPb,
    CommandPayload_oneof_request, Event as EventPb, EventHeader, EventPayload as EventPayloadPb,
};
use std::marker::PhantomData;
use std::time::{SystemTime, UNIX_EPOCH};
use tikv_util::error;

#[derive(Debug)]
pub struct Command(CommandPb);

impl Command {
    pub fn payload(&self) -> &CommandPayload_oneof_request {
        self.0.get_payload().request.as_ref().unwrap()
    }

    pub fn command_id(&self) -> &str {
        self.0.get_header().get_command_id()
    }

    pub fn header(&self) -> &CommandHeader {
        self.0.get_header()
    }

    pub fn from_proto(command_pb: CommandPb) -> Self {
        Command(command_pb)
    }

    pub fn to_proto(&self) -> CommandPb {
        self.0.clone()
    }

    pub fn from_payload(payload: CommandPayload) -> Self {
        let header = Command::build_header(payload.command_id());
        let mut command = CommandPb::default();
        command.set_header(header);
        command.set_payload(payload.into_payload());

        Command(command)
    }

    fn build_header(command_id: &str) -> CommandHeader {
        let mut header = CommandHeader::default();
        header.set_command_id(command_id.to_string());
        header.set_process_time_in_sec(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        );
        header
    }
}

pub enum CommandPayload {
    TransferRequest(TransferRequest),
    ReserveRequest(ReserveRequest),
    ReleaseRequest(ReleaseRequest),
    BatchBalanceOperationRequest(BatchBalanceOperationRequest),
    CreateAccountRequest(CreateAccountRequest),
    DeleteAccountRequest(DeleteAccountRequest),
    LockAccountRequest(LockAccountRequest),
    UnlockAccountRequest(UnlockAccountRequest),
    UpdateAccountConfigRequest(UpdateAccountConfigRequest),
}

impl CommandPayload {
    fn command_id(&self) -> &str {
        match self {
            CommandPayload::TransferRequest(request) => request.get_dedup_id(),
            CommandPayload::ReserveRequest(request) => request.get_dedup_id(),
            CommandPayload::ReleaseRequest(request) => request.get_dedup_id(),
            CommandPayload::BatchBalanceOperationRequest(request) => request.get_dedup_id(),
            CommandPayload::CreateAccountRequest(request) => request.get_header().get_dedup_id(),
            CommandPayload::DeleteAccountRequest(request) => request.get_header().get_dedup_id(),
            CommandPayload::LockAccountRequest(request) => request.get_header().get_dedup_id(),
            CommandPayload::UnlockAccountRequest(request) => request.get_header().get_dedup_id(),
            CommandPayload::UpdateAccountConfigRequest(request) => {
                request.get_header().get_dedup_id()
            }
        }
    }

    fn into_payload(self) -> CommandPayloadPb {
        let mut payload = CommandPayloadPb::default();
        match self {
            CommandPayload::TransferRequest(request) => payload.set_transfer_request(request),
            CommandPayload::ReserveRequest(request) => payload.set_reserve_request(request),
            CommandPayload::ReleaseRequest(request) => payload.set_release_request(request),
            CommandPayload::BatchBalanceOperationRequest(request) => {
                payload.set_batch_balance_operation_request(request)
            }
            CommandPayload::CreateAccountRequest(request) => {
                payload.set_create_account_request(request)
            }
            CommandPayload::DeleteAccountRequest(request) => {
                payload.set_delete_account_request(request)
            }
            CommandPayload::LockAccountRequest(request) => {
                payload.set_lock_account_request(request)
            }
            CommandPayload::UnlockAccountRequest(request) => {
                payload.set_unlock_account_request(request)
            }
            CommandPayload::UpdateAccountConfigRequest(request) => {
                payload.set_update_account_config_request(request)
            }
        }
        payload
    }
}

#[derive(Debug)]
pub struct Event(EventPb);

impl Event {
    pub fn to_proto(&self) -> EventPb {
        self.0.clone()
    }

    pub fn set_header(
        &mut self,
        command_header: CommandHeader,
        log_index: u64,
        offset: u64,
        seq_num: u64,
    ) {
        let mut event_header = EventHeader::default();
        event_header.set_command_id(command_header.get_command_id().to_string());
        event_header.set_process_time_in_sec(command_header.get_process_time_in_sec());
        event_header.set_log_index(log_index);
        event_header.set_offset(offset);
        event_header.set_seq_num(seq_num);
        self.0.set_header(event_header);
    }
}

pub struct Response<Resp> {
    pd: PhantomData<Resp>,
}

impl<Resp> Response<Resp> {
    pub fn build_header(seq_num: u64, log_index: u64, offset: u64) -> ResponseHeader {
        let mut response_header = ResponseHeader::default();
        response_header.set_seq_num(seq_num);
        response_header.set_log_index(log_index);
        response_header.set_offset(offset);
        response_header
    }
}

impl Response<TransferResponse> {
    pub fn build_event(
        seq_num: u64,
        log_index: u64,
        offset: u64,
        request: TransferRequest,
        from: BalanceChange,
        to: BalanceChange,
    ) -> Event {
        let head = Self::build_header(seq_num, log_index, offset);

        let mut transfer_response = TransferResponse::default();
        transfer_response.set_header(head);
        transfer_response.set_request(request);
        transfer_response.set_from(from);
        transfer_response.set_to(to);

        let mut event_pb = TransferEvent::default();
        event_pb.set_response(transfer_response);

        let mut payload = EventPayloadPb::default();
        payload.set_transfer_event(event_pb);

        let mut event = EventPb::default();
        event.set_payload(payload);
        Event(event)
    }
}

impl Response<BatchBalanceOperationResponse> {
    pub fn build_event(
        seq_num: u64,
        log_index: u64,
        offset: u64,
        request: BatchBalanceOperationRequest,
        balance_changes: Vec<BalanceChange>,
    ) -> Event {
        let header = Self::build_header(seq_num, log_index, offset);
        let mut response = BatchBalanceOperationResponse::default();
        response.set_header(header);
        response.set_request(request);
        response.set_balance_changes(balance_changes.into());

        let mut event_pb = BatchBalanceOperationEvent::default();
        event_pb.set_response(response);

        let mut payload = EventPayloadPb::default();
        payload.set_batch_balance_operation_event(event_pb);

        let mut event = EventPb::default();
        event.set_payload(payload);
        Event(event)
    }
}

impl Response<ReserveResponse> {
    pub fn build_event(
        seq_num: u64,
        log_index: u64,
        offset: u64,
        request: ReserveRequest,
        prev_balance: Balance,
        curr_balance: Balance,
    ) -> Event {
        let header = Self::build_header(seq_num, log_index, offset);
        let mut response = ReserveResponse::default();
        response.set_header(header);
        response.set_request(request);
        response.set_prev_balance(prev_balance);
        response.set_curr_balance(curr_balance);

        let mut event_pb = ReserveEvent::default();
        event_pb.set_response(response);

        let mut payload = EventPayloadPb::default();
        payload.set_reserve_event(event_pb);

        let mut event = EventPb::default();
        event.set_payload(payload);
        Event(event)
    }
}

impl Response<ReleaseResponse> {
    pub fn build_event(
        seq_num: u64,
        log_index: u64,
        offset: u64,
        request: ReleaseRequest,
        prev_balance: Balance,
        curr_balance: Balance,
    ) -> Event {
        let header = Self::build_header(seq_num, log_index, offset);
        let mut response = ReleaseResponse::default();
        response.set_header(header);
        response.set_request(request);
        response.set_prev_balance(prev_balance);
        response.set_curr_balance(curr_balance);

        let mut event_pb = ReleaseEvent::default();
        event_pb.set_response(response);

        let mut payload = EventPayloadPb::default();
        payload.set_release_event(event_pb);

        let mut event = EventPb::default();
        event.set_payload(payload);
        Event(event)
    }
}

impl Response<CreateAccountResponse> {
    pub fn build_event(
        seq_num: u64,
        log_index: u64,
        offset: u64,
        request: CreateAccountRequest,
        account: Account,
    ) -> Event {
        let header = Self::build_header(seq_num, log_index, offset);
        let mut response = CreateAccountResponse::default();
        response.set_header(header);
        response.set_request(request);
        response.set_account(account.to_proto());

        let mut event_pb = AccountCreatedEvent::default();
        event_pb.set_response(response);

        let mut payload = EventPayloadPb::default();
        payload.set_account_created_event(event_pb);

        let mut event = EventPb::default();
        event.set_payload(payload);
        Event(event)
    }
}

impl Response<LockAccountResponse> {
    pub fn build_event(
        seq_num: u64,
        log_index: u64,
        offset: u64,
        request: LockAccountRequest,
        account: Account,
    ) -> Event {
        let header = Self::build_header(seq_num, log_index, offset);
        let mut response = LockAccountResponse::default();
        response.set_header(header);
        response.set_request(request);
        response.set_account(account.to_proto());

        let mut event_pb = AccountLockedEvent::default();
        event_pb.set_response(response);

        let mut payload = EventPayloadPb::default();
        payload.set_account_locked_event(event_pb);

        let mut event = EventPb::default();
        event.set_payload(payload);
        Event(event)
    }
}

impl Response<UnlockAccountResponse> {
    pub fn build_event(
        seq_num: u64,
        log_index: u64,
        offset: u64,
        request: UnlockAccountRequest,
        account: Account,
    ) -> Event {
        let header = Self::build_header(seq_num, log_index, offset);
        let mut response = UnlockAccountResponse::default();
        response.set_header(header);
        response.set_request(request);
        response.set_account(account.to_proto());

        let mut event_pb = AccountUnlockedEvent::default();
        event_pb.set_response(response);

        let mut payload = EventPayloadPb::default();
        payload.set_account_unlock_event(event_pb);

        let mut event = EventPb::default();
        event.set_payload(payload);
        Event(event)
    }
}

impl Response<DeleteAccountResponse> {
    pub fn build_event(
        seq_num: u64,
        log_index: u64,
        offset: u64,
        request: DeleteAccountRequest,
        account: Account,
    ) -> Event {
        let header = Self::build_header(seq_num, log_index, offset);
        let mut response = DeleteAccountResponse::default();
        response.set_header(header);
        response.set_request(request);
        response.set_account(account.to_proto());

        let mut event_pb = AccountDeletedEvent::default();
        event_pb.set_response(response);

        let mut payload = EventPayloadPb::default();
        payload.set_account_deleted_event(event_pb);

        let mut event = EventPb::default();
        event.set_payload(payload);
        Event(event)
    }
}

impl Response<UpdateAccountConfigResponse> {
    pub fn build_event(
        seq_num: u64,
        log_index: u64,
        offset: u64,
        request: UpdateAccountConfigRequest,
        account: Account,
    ) -> Event {
        let header = Self::build_header(seq_num, log_index, offset);
        let mut response = UpdateAccountConfigResponse::default();
        response.set_header(header);
        response.set_request(request);
        response.set_account(account.to_proto());

        let mut event_pb = AccountConfigUpdatedEvent::default();
        event_pb.set_response(response);

        let mut payload = EventPayloadPb::default();
        payload.set_account_config_update_event(event_pb);

        let mut event = EventPb::default();
        event.set_payload(payload);
        Event(event)
    }
}

pub fn assert_command_and_event_match(command: &Command, event: EventPb) -> GeneralResult<EventPb> {
    let command_type = command.payload();
    let event_type = event.get_payload().event.as_ref().unwrap();

    match (command_type, event_type) {
        (transfer_request(_), transfer_event(_))
        | (batch_balance_operation_request(_), batch_balance_operation_event(_))
        | (reserve_request(_), reserve_event(_))
        | (release_request(_), release_event(_))
        | (create_account_request(_), account_created_event(_))
        | (lock_account_request(_), account_locked_event(_))
        | (unlock_account_request(_), account_unlock_event(_))
        | (delete_account_request(_), account_deleted_event(_))
        | (update_account_config_request(_), account_config_update_event(_)) => Ok(event),
        (_, _) => {
            error!(
                "Command {:?} has duplicate command_id with event {:?}, \
                but with conflict command type!",
                command, event
            );
            Err(OtherError::ConflictDedupId(command.command_id().to_string()).into())
        }
    }
}
