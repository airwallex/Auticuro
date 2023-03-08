#![allow(warnings)]

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

use crate::errors::{self, Error};
use crate::keys;
use crate::metrics::GRPC_MSG_HISTOGRAM_STATIC;
use crate::msg::Callback;
use crate::peer_storage::init_apply_state;
use crate::rocks_engine::CF_RAFT;
use crate::storage::Storage;
use crate::store::RaftRouter;
use futures::channel::oneshot;
use futures::TryFutureExt;
use kvproto::kvrpcpb::{Context, MessageQueueRequest};
use kvproto::metapb::Region;
use kvproto::raft_cmdpb::{
    CmdType, RaftCmdRequest, RaftRequestHeader, StatusCmdType, StatusRequest,
};
use kvproto::raft_serverpb::RegionLocalState;
use prost;
use protobuf;
use raft::eraftpb::Entry;
use std::cmp::min;
use std::io::Cursor;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TryRecvError, RecvError};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tikv_util::future::paired_future_callback;
use tikv_util::time::{duration_to_sec, duration_to_ms, Instant};
use tikv_util::{debug, info, warn, trace};
use std::ops::{Deref, DerefMut};

fn parse_data_to_proto<T: protobuf::Message + Default>(data: &[u8]) -> T {
    let mut result = T::default();
    result.merge_from_bytes(data).unwrap();
    result
}

fn parse_data_to_prost<T: prost::Message + Default>(data: &[u8]) -> T {
    T::decode(&mut Cursor::new(data)).unwrap()
}

pub fn parse_log_entry_to_proto<T>(entry: &Entry) -> (u64, Vec<T>)
where
    T: protobuf::Message + Default,
{
    let index = entry.get_index();
    let data = entry.get_data();

    if data.is_empty() {
        return (index, vec![]);
    }

    let messages: Vec<_> = parse_data_to_proto::<RaftCmdRequest>(data)
        .get_requests()
        .into_iter()
        .map(|req| {
            assert_eq!(req.get_cmd_type(), CmdType::MessageQueue);
            parse_data_to_proto::<T>(req.get_message_queue().get_payload())
        })
        .collect();

    (index, messages)
}

pub fn parse_log_entry_to_prost<T>(entry: &Entry) -> (u64, Vec<T>)
where
    T: prost::Message + Default,
{
    let index = entry.get_index();
    let data = entry.get_data();

    if data.is_empty() {
        return (index, vec![]);
    }

    let messages: Vec<_> = parse_data_to_proto::<RaftCmdRequest>(data)
        .get_requests()
        .into_iter()
        .map(|req| {
            assert_eq!(req.get_cmd_type(), CmdType::MessageQueue);
            parse_data_to_prost::<T>(req.get_message_queue().get_payload())
        })
        .collect();

    (index, messages)
}

#[derive(Clone)]
pub struct MessageQueueAdapter {
    storage: Storage,
    router: RaftRouter,
    store_id: u64,
    region: Region,
}

impl MessageQueueAdapter {
    #[allow(dead_code)]
    pub fn new(storage: Storage, router: RaftRouter, region_id: u64, store_id: u64) -> Self {
        let local_state: RegionLocalState = storage
            .get_engine()
            .engines()
            .kv
            .get_msg_cf(CF_RAFT, &keys::region_state_key(region_id))
            .unwrap();
        let region = local_state.get_region().clone();

        MessageQueueAdapter {
            storage,
            router,
            store_id,
            region,
        }
    }

    // Return: Vec of entries (index, Vec of payloads)
    //         Error, return (log_start_index, log_end_index)
    #[allow(dead_code)]
    pub fn get_range<F, T>(
        &self,
        start: u64,
        end: u64,
        log_entry_parser: F,
    ) -> Result<Vec<(u64, Vec<T>)>, (u64, u64)>
    where
        F: Fn(&Entry) -> (u64, Vec<T>),
    {
        let (log_start_index, log_end_index) = self.get_start_index_and_applied_index();
        if start < log_start_index || end > log_end_index {
            return Err((log_start_index, log_end_index));
        }

        let mut buf = vec![];
        self.storage.get_engine().engines().raft.fetch_entries_to(
            self.region.get_id(),
            start,
            end + 1,
            None,
            &mut buf,
        );
        Ok(buf.iter().map(log_entry_parser).collect())
    }

    #[allow(dead_code)]
    pub fn get_start_index_and_applied_index(&self) -> (u64, u64) {
        let apply_state = init_apply_state(&self.storage.get_engine().engines(), &self.region);
        (
            apply_state.get_truncated_state().get_index() + 1,
            apply_state.get_applied_index(),
        )
    }

    #[allow(dead_code)]
    pub fn is_leader(&self) -> bool {
        let region_id = self.region.get_id();
        let store_id = self.store_id;
        let router = self.router.clone();

        let task = async move {
            let mut header = RaftRequestHeader::default();
            header.set_region_id(region_id);
            header.mut_peer().set_store_id(store_id);

            let mut status_request = StatusRequest::default();
            status_request.set_cmd_type(StatusCmdType::RegionLeader);

            let mut raft_cmd_req = RaftCmdRequest::default();
            raft_cmd_req.set_header(header);
            raft_cmd_req.set_status_request(status_request);

            let (tx, rx) = oneshot::channel();
            let cb = Callback::Read(Box::new(|resp| tx.send(resp).unwrap()));
            router.send_command(raft_cmd_req, cb)?;

            let leader_id = rx
                .map_err(|e| Error::Other(Box::new(e)))
                .await?
                .response
                .get_status_response()
                .get_region_leader()
                .get_leader()
                .get_id();

            errors::Result::Ok(leader_id == store_id)
        };

        match futures_executor::block_on(task) {
            Ok(result) => result,
            Err(e) => {
                warn!("isLeader check failed"; "err" => ?e);
                false
            }
        }
    }

    #[allow(dead_code)]
    pub async fn send_payload(&self, payload: Vec<u8>) -> Result<(u64, u64), String> {
        let mut context = Context::default();
        context.set_region_id(self.region.get_id());
        context.mut_peer().set_id(self.store_id);
        context.mut_peer().set_store_id(self.store_id);

        let mut message_queue_req = MessageQueueRequest::default();
        message_queue_req.set_context(context);
        message_queue_req.set_payload(payload);

        let begin_instant = Instant::now_coarse();

        let (cb, f) = paired_future_callback();
        self.storage
            .raw_message_queue(message_queue_req, cb);

        let v = f.await.map_err(|e| format!("{:?}", e))?;

        GRPC_MSG_HISTOGRAM_STATIC
            .message_queue
            .observe(duration_to_sec(begin_instant.elapsed()));

        let resp = v.map_err(|e| format!("{:?}", e))?;
        Ok((resp.get_index(), resp.get_offset()))
    }
}

pub struct MetaInfo {
    pub send_time: Instant,
    pub recv_time: Option<Instant>,
}

pub struct LogEntry<T> {
    pub meta: MetaInfo,
    pub index: u64,
    pub messages: Vec<T>
}

impl<T> LogEntry<T> {
    pub fn new(index: u64, messages: Vec<T>) -> Self {
        let meta = MetaInfo {
            send_time: Instant::now_coarse(),
            recv_time: None
        };

        LogEntry {
            meta,
            index,
            messages
        }
    }
}

pub struct LogEntryReceiver<T>(Receiver<LogEntry<T>>);

impl<T> Deref for LogEntryReceiver<T> {
    type Target = Receiver<LogEntry<T>>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for LogEntryReceiver<T> {
    fn deref_mut(&mut self) -> &mut Receiver<LogEntry<T>> {
        &mut self.0
    }
}

impl<T> LogEntryReceiver<T> {
    pub fn try_recv(&self) -> Result<LogEntry<T>, TryRecvError> {

        self.0.try_recv().map(|mut entry| {
            Self::enrich_meta_info(entry)
        })
    }

    pub fn recv(&self) -> Result<LogEntry<T>, RecvError> {
        self.0.recv().map(|mut entry| {
            Self::enrich_meta_info(entry)
        })
    }

    fn enrich_meta_info(mut entry: LogEntry<T>) -> LogEntry<T> {
        let recv_time = Instant::now_coarse();
        recv_time.duration_since(entry.meta.send_time);

        GRPC_MSG_HISTOGRAM_STATIC
            .log_consumer_queueing
            .observe(duration_to_sec(recv_time.duration_since(entry.meta.send_time)));

        entry.meta.recv_time = Some(recv_time);
        entry
    }

    pub fn batch_recv(&self, max_count: usize) -> Vec<LogEntry<T>> {
        let start = Instant::now();
        let mut buffer = vec![];
        while buffer.len() < max_count {
            match self.try_recv() {
                Ok(entry) => {
                    buffer.push(entry);
                }
                Err(TryRecvError::Empty) => {
                    break;
                }
                Err(TryRecvError::Disconnected) => {
                    unreachable!()
                }
            }
        }
        if buffer.len() > 0 {
            trace!(
            "LogEntryReceiver::batch_recv received {} entries of type {} in {:?}ms",
            buffer.len(),
            std::any::type_name::<LogEntry<T>>(),
            duration_to_ms(start.elapsed()));
        }
        buffer
    }
}

pub struct LogConsumer<F, T> {
    message_queue: Arc<MessageQueueAdapter>,
    log_index: u64,
    log_entry_parser: F,

    sender: SyncSender<LogEntry<T>>,
}

impl<F, T> LogConsumer<F, T>
where
    F: Fn(&Entry) -> (u64, Vec<T>),
    F: Send + Clone + 'static,
    T: Send + 'static,
{
    pub fn run(
        message_queue: Arc<MessageQueueAdapter>,
        log_index: u64,
        log_entry_parser: F,
    ) -> LogEntryReceiver<T> {
        let log_consumer_buffer_size = std::env::var("log_consumer_buffer_size")
            .unwrap_or("102400".to_string())
            .parse::<usize>()
            .unwrap();

        let (sender, receiver) = sync_channel(log_consumer_buffer_size);
        let mut log_consumer = LogConsumer {
            message_queue,
            log_index,
            log_entry_parser,
            sender,
        };

        thread::Builder::new()
            .name(format!("log_consumer_from_{}", log_index))
            .spawn(move || log_consumer.start_consuming())
            .unwrap();

        info!("LogConsumer started with log_index={} and log_consumer_buffer_size={}", log_index, log_consumer_buffer_size);
        LogEntryReceiver(receiver)
    }

    fn start_consuming(&mut self) {
        loop {
            let (start_index, applied_index) =
                self.message_queue.get_start_index_and_applied_index();

            if self.log_index < start_index {
                panic!(
                    "There is gap between log_index {} and start_index {}",
                    self.log_index, start_index
                );
            }

            if self.log_index > applied_index {
                thread::sleep(Duration::from_micros(500));
                continue;
            }

            let end = min(self.log_index + 1000, applied_index);
            debug!("consume range [{}, {}]", self.log_index, end);

            let entries = self
                .message_queue
                .get_range(self.log_index, end, self.log_entry_parser.clone())
                .unwrap();

            for (index, messages) in entries {
                let log_entry = LogEntry::new(index, messages);
                self.sender.send(log_entry).unwrap();
            }

            self.log_index = end + 1;
        }
    }
}
