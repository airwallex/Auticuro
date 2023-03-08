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

use crate::errors::{Error, Result};
use crate::keys;
use crate::metrics::*;
use crate::msg::Callback;
use crate::peer_storage::init_apply_state;
use crate::rocks_engine::CF_RAFT;
use crate::storage::Storage;
use crate::store::RaftRouter;
use crossbeam::channel;
use futures::channel::oneshot;
use futures::future::{self, Future, FutureExt, TryFutureExt};
use futures::sink::SinkExt;
use futures::stream::{StreamExt, TryStreamExt};
use grpcio::{
    ClientStreamingSink, DuplexSink, Error as GrpcError, RequestStream, Result as GrpcResult,
    RpcContext, RpcStatus, RpcStatusCode, ServerStreamingSink, UnarySink, WriteFlags,
};
use kvproto::kvrpcpb::*;
use kvproto::mqpb::*;
use kvproto::raft_cmdpb::*;
use kvproto::raft_serverpb::*;
use kvproto::tikvpb::*;
use std::sync::Arc;
use tikv_util::future::paired_future_callback;
use tikv_util::time::{duration_to_sec, Instant};
use tikv_util::{box_err, debug, error, info, trace, warn};

/// Service handles the RPC messages for the `Tikv` service.
pub struct Service {
    store_id: u64,
    // For handling KV requests.
    storage: Storage,

    // sender: Arc<channel::Sender<RaftMessage>>,
    router: RaftRouter,
}

impl Service {
    /// Constructs a new `Service` which provides the `Tikv` service.
    pub fn new(store_id: u64, storage: Storage, router: RaftRouter) -> Self {
        Service {
            store_id,
            storage,
            router,
        }
    }
}

impl Clone for Service {
    fn clone(&self) -> Self {
        Service {
            store_id: self.store_id,
            storage: self.storage.clone(),
            // sender: self.sender.clone(),
            router: self.router.clone(),
        }
    }
}

macro_rules! handle_request {
($fn_name: ident, $future_name: ident, $req_ty: ident, $resp_ty: ident) => {
    fn $fn_name(&mut self, ctx: RpcContext<'_>, req: $req_ty, sink: UnarySink<$resp_ty>) {
        // forward_unary!(self.proxy, $fn_name, ctx, req, sink);
        let begin_instant = Instant::now_coarse();

        let resp = $future_name(&self.storage, req);
        let task = async move {
            let resp = resp.await?;
            sink.success(resp).await?;
            GRPC_MSG_HISTOGRAM_STATIC
                .$fn_name
                .observe(duration_to_sec(begin_instant.elapsed()));
            Result::Ok(())
        }
        .map_err(|e| {
            debug!("kv rpc failed";
                "request" => stringify!($fn_name),
                "err" => ?e
            );
            // GRPC_MSG_FAIL_COUNTER.$fn_name.inc();
        })
        .map(|_|());

        ctx.spawn(task);
    }
}
}

impl Tikv for Service {
    fn raft(
        &mut self,
        ctx: RpcContext<'_>,
        stream: RequestStream<RaftMessage>,
        sink: ClientStreamingSink<Done>,
    ) {
        info!("Tikv.Raft() is called.");
        let store_id = self.store_id;
        // let sender = self.sender.clone();
        let router = self.router.clone();
        ctx.spawn(async move {
            let res = stream.map_err(Error::from).try_for_each(move |msg| {
                let to_store_id = msg.get_to_peer().get_store_id();
                if to_store_id != store_id {
                    future::err(box_err!(
                        "to_store_id != store_id, to_store_id is {}, store_id is {}",
                        to_store_id,
                        store_id
                    ))
                } else {
                    trace!("RaftMessage is received {:?}", msg); // debug
                    let ret = router.send_raft_message(msg).map_err(Error::from);
                    future::ready(ret)
                }
            });
            let status = match res.await {
                Err(e) => {
                    let msg = format!("{:?}", e);
                    error!("dispatch raft msg from gRPC to raftstore fail"; "err" => %msg);
                    RpcStatus::with_message(RpcStatusCode::UNKNOWN, msg)
                }
                Ok(_) => RpcStatus::new(RpcStatusCode::UNKNOWN),
            };
            let _ = sink
                .fail(status)
                .map_err(|e| error!("KvService::raft send response fail"; "err" => ?e))
                .await;
        });
    }

    fn probe(&mut self, ctx: RpcContext<'_>, req: ProbeRequest, sink: UnarySink<ProbeResponse>) {
        let store_id = self.store_id;
        let region_id = 1; // region_id is hard code to 1 for now as it is a single raft group.
        let router = self.router.clone();

        let task = async move {
            let mut header = RaftRequestHeader::default();
            header.set_region_id(region_id);
            header.mut_peer().set_store_id(store_id);

            let mut status_request = StatusRequest::default();
            status_request.set_cmd_type(StatusCmdType::RegionDetail);

            let mut raft_cmd_req = RaftCmdRequest::default();
            raft_cmd_req.set_header(header);
            raft_cmd_req.set_status_request(status_request);

            let (tx, rx) = oneshot::channel();
            let cb = Callback::Read(Box::new(|resp| tx.send(resp).unwrap()));
            router.send_command(raft_cmd_req, cb)?;

            let mut r = rx.map_err(|e| Error::Other(Box::new(e))).await?;

            if r.response.get_header().has_error() {
                let e = r.response.get_header().get_error();
                return Err(Error::Other(e.message.clone().into()));
            }

            let mut detail = r.response.take_status_response().take_region_detail();

            let mut resp = ProbeResponse::default();
            resp.set_region(detail.take_region());
            resp.set_leader(detail.take_leader());

            sink.success(resp).await?;
            Ok(())
        }
        .map_err(|e| {
            warn!("probe rpc failed"; "err" => ?e);
        })
        .map(|_| ());

        ctx.spawn(task);
    }

    handle_request!(
        query_raft_log_entries,
        future_query_raft_log_entries,
        QueryRaftLogEntriesRequest,
        QueryRaftLogEntriesResponse
    );
}

fn future_query_raft_log_entries(
    storage: &Storage,
    mut req: QueryRaftLogEntriesRequest,
) -> impl Future<Output = Result<QueryRaftLogEntriesResponse>> {
    let t = Instant::now_coarse();

    let engines = storage.get_engine().engines();

    let region_id = 1;
    let local_state: RegionLocalState = engines
        .kv
        .get_msg_cf(CF_RAFT, &keys::region_state_key(region_id))
        .unwrap();
    let region = local_state.get_region();

    let apply_state = init_apply_state(&engines, &region);

    let first_log_index = apply_state.get_truncated_state().get_index() + 1;
    let start_index = std::cmp::max(
        req.get_start(),
        first_log_index,
    );

    let last_log_index = apply_state.get_applied_index();
    let end_index = std::cmp::min(req.get_end(), last_log_index);

    let mut resp = QueryRaftLogEntriesResponse::default();
    resp.set_first_log_index(first_log_index);
    resp.set_last_log_index(last_log_index);
    if start_index > end_index {
        resp.set_error(format!("Illegal Request. Max(Req's start index({}), first_log_index({}))=={} is greater than min(Req's end index({}), last_log_index({}))=={}", req.get_start(), first_log_index, start_index, req.get_end(), last_log_index, end_index));
        return future::ready(Ok(resp));
    }

    let mut entries = vec![];
    engines
        .raft
        .fetch_entries_to(region_id, start_index, end_index + 1, None, &mut entries);

    resp.set_entries(entries.into());

    info!(
        "Tikv.QueryRaftLogEntries() is called, request is {:?}, shrink to [{}, {}), elapsed {:?}",
        req,
        start_index,
        end_index,
        t.elapsed()
    );
    future::ready(Ok(resp))
}

#[derive(Clone)]
pub struct MQService {
    // For handling KV requests.
    storage: Storage,
}

impl MQService {
    /// Constructs a new `Service` which provides the `Tikv` service.
    pub fn new(storage: Storage) -> Self {
        MQService { storage }
    }
}

impl Mq for MQService {
    handle_request!(
        message_queue,
        future_raw_message_queue,
        MessageQueueRequest,
        MessageQueueResponse
    );
}

fn future_raw_message_queue(
    storage: &Storage,
    mut req: MessageQueueRequest,
) -> impl Future<Output = Result<MessageQueueResponse>> {
    let t = Instant::now_coarse();

    let (cb, f) = paired_future_callback();
    let res = storage.raw_message_queue(req, cb);

    async move {
        let v = match res {
            Err(e) => Err(e),
            Ok(_) => f.await?, // not Canceled
        };

        let resp = match v {
            Ok(resp) => resp,
            Err(e) => {
                let mut resp = MessageQueueResponse::default();
                resp.set_error(format!("{:?}", e));
                resp
            }
        };

        trace!(
            "Tikv.MessageQueue() is called, elapsed: {:?}, resp: {:?}",
            t.elapsed(),
            resp
        );
        Ok(resp)
    }
}
