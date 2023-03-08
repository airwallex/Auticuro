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

use std::{
    fmt::{self, Debug, Display, Formatter},
    mem, result,
    sync::Arc,
    time::Duration,
};

use kvproto::{
    errorpb,
    kvrpcpb::Context,
    metapb,
    raft_cmdpb::{CmdType, RaftCmdRequest, RaftCmdResponse, RaftRequestHeader, Request, Response},
};

use crate::rocks_engine::{Engines, RocksEngine};
use crate::store::RaftRouter;

use crate::errors::{Error, Result};
use crate::msg::{Callback as StoreCallback, WriteResponse};
use crate::storage::{Modify, WriteData};
use tikv_util::box_err;
use tikv_util::time::Instant;

pub type Callback<T> = Box<dyn FnOnce((CbContext, Result<T>)) + Send>;
pub type ExtCallback = Box<dyn FnOnce() + Send>;

#[derive(Debug)]
pub struct CbContext {
    pub term: Option<u64>,
}

impl CbContext {
    pub fn new() -> CbContext {
        CbContext { term: None }
    }
}

/// `RaftKv` is a storage engine base on `RaftStore`.
#[derive(Clone)]
pub struct RaftKv {
    router: RaftRouter,
    engines: Engines,
}

pub enum CmdRes {
    Resp(Vec<Response>),
    // Snap(RegionSnapshot<S>),
}

fn new_ctx(resp: &RaftCmdResponse) -> CbContext {
    let mut cb_ctx = CbContext::new();
    cb_ctx.term = Some(resp.get_header().get_current_term());
    cb_ctx
}

fn check_raft_cmd_response(resp: &mut RaftCmdResponse, req_cnt: usize) -> Result<()> {
    if resp.get_header().has_error() {
        return Err(Error::RequestFailed(resp.take_header().take_error()));
    }
    if req_cnt != resp.get_responses().len() {
        // return Err(Error::InvalidResponse(format!(
        //     "responses count {} is not equal to requests count {}",
        //     resp.get_responses().len(),
        //     req_cnt
        // )));
        return Err(box_err!(format!(
            "responses count {} is not equal to requests count {}",
            resp.get_responses().len(),
            req_cnt
        )));
    }

    Ok(())
}

fn on_write_result(mut write_resp: WriteResponse, req_cnt: usize) -> (CbContext, Result<CmdRes>) {
    let cb_ctx = new_ctx(&write_resp.response);
    if let Err(e) = check_raft_cmd_response(&mut write_resp.response, req_cnt) {
        return (cb_ctx, Err(e));
    }
    let resps = write_resp.response.take_responses();
    (cb_ctx, Ok(CmdRes::Resp(resps.into())))
}

impl RaftKv {
    /// Create a RaftKv using specified configuration.
    pub fn new(router: RaftRouter, engines: Engines) -> RaftKv {
        RaftKv { router, engines }
    }

    fn new_request_header(&self, ctx: &Context) -> RaftRequestHeader {
        let mut header = RaftRequestHeader::default();
        header.set_region_id(ctx.get_region_id());
        header.set_peer(ctx.get_peer().clone());

        if ctx.get_term() != 0 {
            header.set_term(ctx.get_term());
        }

        header
    }

    pub fn exec_write_requests(
        &self,
        ctx: &Context,
        batch: WriteData,
        write_cb: Callback<CmdRes>,
        proposed_cb: Option<ExtCallback>,
        committed_cb: Option<ExtCallback>,
    ) -> Result<()> {
        let reqs = modifies_to_requests(batch.modifies);
        let len = reqs.len();
        let mut header = self.new_request_header(ctx);

        let mut cmd = RaftCmdRequest::default();
        cmd.set_header(header);
        cmd.set_requests(reqs.into());

        let cb = StoreCallback::write_ext(
            Box::new(move |resp| {
                let (cb_ctx, res) = on_write_result(resp, len);
                write_cb((cb_ctx, res.map_err(Error::into)));
            }),
            proposed_cb,
            committed_cb,
        );

        self.router.send_command(cmd, cb)?;
        Ok(())
    }
}

impl Display for RaftKv {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "RaftKv")
    }
}

impl Debug for RaftKv {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "RaftKv")
    }
}

impl RaftKv {
    pub fn kv_engine(&self) -> RocksEngine {
        self.engines.kv.clone()
    }

    pub fn engines(&self) -> Engines {
        self.engines.clone()
    }
}

pub fn modifies_to_requests(modifies: Vec<Modify>) -> Vec<Request> {
    let mut reqs = Vec::with_capacity(modifies.len());
    for m in modifies {
        let mut req = Request::default();
        match m {
            Modify::MessageQueue(message_queue) => {
                req.set_cmd_type(CmdType::MessageQueue);
                req.set_message_queue(message_queue);
            }
        }
        reqs.push(req);
    }
    reqs
}
