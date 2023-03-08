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

use std::collections::{HashMap, VecDeque};
use std::fmt::{self, Debug, Formatter};
use std::ops::{Deref, DerefMut, Range as StdRange};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
#[cfg(test)]
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::vec::Drain;
use std::{cmp, usize};
use std::{mem, thread};

use crossbeam::channel::{self, TryRecvError, TrySendError};
use kvproto::kvrpcpb::{MessageQueueRequest, MessageQueueResponse};
use kvproto::metapb::{PeerRole, Region, RegionEpoch};
use kvproto::raft_cmdpb::{
    AdminCmdType, AdminRequest, AdminResponse, CmdType, RaftCmdRequest, RaftCmdResponse, Request,
    Response,
};
use kvproto::raft_serverpb::{RaftApplyState, RaftTruncatedState};
use raft::eraftpb::{
    ConfChange, ConfChangeType, ConfChangeV2, Entry, EntryType, Snapshot as RaftSnapshot,
};
use tikv_util::time::{duration_to_sec, Instant};
use tikv_util::{box_err, debug, error, info, safe_panic, slow_log, thd_name, trace, warn};
use tikv_util::{Either, MustConsumeVec};
use time::Timespec;
use uuid::Builder as UuidBuilder;

use crate::msg::{Callback, PeerMsg};
use crate::peer::Peer;
use crate::peer_storage::{self, CachedEntries};
use crate::{cmd_resp, keys, util};

use crate::errors::{Error, Result};
use crate::keys::{DATA_MAX_KEY, DATA_MIN_KEY, DEDUP_MAX_KEY, DEDUP_MIN_KEY};
use crate::metrics::{APPLY_ENTRY_COUNTER, APPLY_EVENT_COUNTER};
use crate::rocks_engine::{RocksEngine, RocksWriteBatch, CF_RAFT};
use crate::store::RaftPollerBuilder;
use std::str::FromStr;

const DEFAULT_APPLY_WB_SIZE: usize = 4 * 1024;
const SHRINK_PENDING_CMD_QUEUE_CAP: usize = 64;

#[derive(Clone, Debug, Default)]
pub struct Cmd {
    pub index: u64,
    pub request: RaftCmdRequest,
    pub response: RaftCmdResponse,
}

impl Cmd {
    pub fn new(index: u64, request: RaftCmdRequest, response: RaftCmdResponse) -> Cmd {
        Cmd {
            index,
            request,
            response,
        }
    }
}

pub struct PendingCmd {
    pub index: u64,
    pub term: u64,
    pub cb: Option<Callback>,
}

impl PendingCmd {
    fn new(index: u64, term: u64, cb: Callback) -> PendingCmd {
        PendingCmd {
            index,
            term,
            cb: Some(cb),
        }
    }
}

impl Drop for PendingCmd {
    fn drop(&mut self) {
        if self.cb.is_some() {
            safe_panic!(
                "callback of pending command at [index: {}, term: {}] is leak",
                self.index,
                self.term
            );
        }
    }
}

impl Debug for PendingCmd {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "PendingCmd [index: {}, term: {}, has_cb: {}]",
            self.index,
            self.term,
            self.cb.is_some()
        )
    }
}

/// Commands waiting to be committed and applied.
#[derive(Debug)]
pub struct PendingCmdQueue {
    normals: VecDeque<PendingCmd>,
}

impl PendingCmdQueue {
    fn new() -> PendingCmdQueue {
        PendingCmdQueue {
            normals: VecDeque::new(),
        }
    }

    fn pop_normal(&mut self, index: u64, term: u64) -> Option<PendingCmd> {
        self.normals.pop_front().and_then(|cmd| {
            if self.normals.capacity() > SHRINK_PENDING_CMD_QUEUE_CAP
                && self.normals.len() < SHRINK_PENDING_CMD_QUEUE_CAP
            {
                self.normals.shrink_to_fit();
            }
            if (cmd.term, cmd.index) > (term, index) {
                self.normals.push_front(cmd);
                return None;
            }
            Some(cmd)
        })
    }

    fn append_normal(&mut self, cmd: PendingCmd) {
        self.normals.push_back(cmd);
    }
}

#[derive(Debug)]
pub enum ExecResult {
    CompactLog {
        state: RaftTruncatedState,
        first_index: u64,
    },
}

/// The possible returned value when applying logs.
pub enum ApplyResult {
    None,
    // Yield,
    /// Additional result that needs to be sent back to raftstore.
    Res(ExecResult),
    // /// It is unable to apply the `CommitMerge` until the source peer
    // /// has applied to the required position and sets the atomic boolean
    // /// to true.
    // WaitMergeSource(Arc<AtomicU64>),
}

struct ExecContext {
    apply_state: RaftApplyState,
    index: u64,
    term: u64,
}

impl ExecContext {
    pub fn new(apply_state: RaftApplyState, index: u64, term: u64) -> ExecContext {
        ExecContext {
            apply_state,
            index,
            term,
        }
    }
}

// The applied command and their callback
struct ApplyCallbackBatch {
    cb_batch: MustConsumeVec<(Callback, RaftCmdResponse)>,
}

impl ApplyCallbackBatch {
    fn new() -> ApplyCallbackBatch {
        ApplyCallbackBatch {
            cb_batch: MustConsumeVec::new("callback of apply callback batch"),
        }
    }

    fn push_cb(&mut self, cb: Callback, resp: RaftCmdResponse) {
        self.cb_batch.push((cb, resp));
    }

    fn push(&mut self, cb: Option<Callback>, cmd: Cmd) {
        if let Some(cb) = cb {
            self.cb_batch.push((cb, cmd.response.clone()));
        }
    }
}

pub trait Notifier: Send {
    fn notify(&self, apply_res: Vec<ApplyRes>);
    fn notify_one(&self, region_id: u64, msg: PeerMsg);
    fn clone_box(&self) -> Box<dyn Notifier>;
}

struct ApplyContext {
    tag: String,
    timer: Option<Instant>,

    router: ApplyRouter,
    notifier: Box<dyn Notifier>,
    engine: RocksEngine,
    applied_batch: ApplyCallbackBatch,
    apply_res: Vec<ApplyRes>,
    exec_ctx: Option<ExecContext>,

    kv_wb: RocksWriteBatch,
    committed_count: usize,

    // Whether synchronize WAL is preferred.
    sync_log_hint: bool,

    store_id: u64,
}

impl ApplyContext {
    pub fn new(
        tag: String,
        engine: RocksEngine,
        router: ApplyRouter,
        notifier: Box<dyn Notifier>,
        store_id: u64,
    ) -> ApplyContext {
        // If `enable_multi_batch_write` was set true, we create `RocksWriteBatchVec`.
        // Otherwise create `RocksWriteBatch`.
        // let kv_wb = W::with_capacity(&engine, DEFAULT_APPLY_WB_SIZE);
        let kv_wb =
            RocksWriteBatch::with_capacity(Arc::clone(engine.as_inner()), DEFAULT_APPLY_WB_SIZE);

        ApplyContext {
            tag,
            timer: None,
            engine: engine.clone(),
            router,
            notifier,
            kv_wb,
            applied_batch: ApplyCallbackBatch::new(),
            apply_res: vec![],
            committed_count: 0,
            sync_log_hint: false,
            exec_ctx: None,
            store_id,
        }
    }

    /// Prepares for applying entries for `delegate`.
    ///
    /// A general apply progress for a delegate is:
    /// `prepare_for` -> `commit` [-> `commit` ...] -> `finish_for`.
    /// After all delegates are handled, `write_to_db` method should be called.
    pub fn prepare_for(&mut self, _delegate: &mut ApplyDelegate) {}

    /// Commits all changes have done for delegate. `persistent` indicates whether
    /// write the changes into rocksdb.
    ///
    /// This call is valid only when it's between a `prepare_for` and `finish_for`.
    pub fn commit(&mut self, delegate: &mut ApplyDelegate) {
        if delegate.last_flush_applied_index < delegate.apply_state.get_applied_index() {
            delegate.write_apply_state(self.kv_wb_mut());
        }
        self.commit_opt(delegate, true);
    }

    fn commit_opt(&mut self, delegate: &mut ApplyDelegate, persistent: bool) {
        if persistent {
            self.write_to_db();
            self.prepare_for(delegate);
            delegate.last_flush_applied_index = delegate.apply_state.get_applied_index()
        }
    }

    /// Writes all the changes into RocksDB.
    /// If it returns true, all pending writes are persisted in engines.
    pub fn write_to_db(&mut self) -> bool {
        let need_sync = self.sync_log_hint;
        if !self.kv_wb_mut().is_empty() {
            self.kv_wb().write_opt(need_sync);
            self.sync_log_hint = false;
            // Clear data, reuse the WriteBatch, this can reduce memory allocations and deallocations.
            self.kv_wb_mut().clear();
        }

        // Take the applied commands and their callback
        let ApplyCallbackBatch { mut cb_batch } =
            mem::replace(&mut self.applied_batch, ApplyCallbackBatch::new());

        // Invoke callbacks
        for (cb, resp) in cb_batch.drain(..) {
            cb.invoke_with_response(resp)
        }

        need_sync
    }

    /// Finishes `Apply`s for the delegate.
    pub fn finish_for(&mut self, delegate: &mut ApplyDelegate, results: VecDeque<ExecResult>) {
        delegate.write_apply_state(self.kv_wb_mut());

        self.commit_opt(delegate, false);
        self.apply_res.push(ApplyRes {
            region_id: delegate.region_id(),
            apply_state: delegate.apply_state.clone(),
            exec_res: results,
            applied_index_term: delegate.applied_index_term,
        });
    }

    #[inline]
    pub fn kv_wb(&self) -> &RocksWriteBatch {
        &self.kv_wb
    }

    #[inline]
    pub fn kv_wb_mut(&mut self) -> &mut RocksWriteBatch {
        &mut self.kv_wb
    }

    /// Flush all pending writes to engines.
    /// If it returns true, all pending writes are persisted in engines.
    pub fn flush(&mut self) -> bool {
        // TODO: this check is too hacky, need to be more verbose and less buggy.
        let t = match self.timer.take() {
            Some(t) => t,
            None => return false,
        };

        let is_synced = self.write_to_db();

        if !self.apply_res.is_empty() {
            let apply_res = mem::take(&mut self.apply_res);
            self.notifier.notify(apply_res);
        }

        let elapsed = t.elapsed();

        info!(
            "elapsed {:?}, {} handle ready {} committed entries",
            elapsed, self.tag, self.committed_count
        );
        self.committed_count = 0;
        is_synced
    }
}

/// Calls the callback of `cmd` when it can not be processed further.
fn notify_stale_command(region_id: u64, peer_id: u64, term: u64, mut cmd: PendingCmd) {
    info!(
        "command is stale, skip";
        "region_id" => region_id,
        "peer_id" => peer_id,
        "index" => cmd.index,
        "term" => cmd.term
    );
    notify_stale_req(term, cmd.cb.take().unwrap());
}

pub fn notify_stale_req(term: u64, cb: Callback) {
    let resp = cmd_resp::err_resp(Error::StaleCommand, term);
    cb.invoke_with_response(resp);
}

/// Checks if a write is needed to be issued after handling the command.
fn should_sync_log(cmd: &RaftCmdRequest) -> bool {
    // if cmd.has_admin_request() {
    //     if cmd.get_admin_request().get_cmd_type() == AdminCmdType::CompactLog {
    //         // We do not need to sync WAL before compact log, because this request will send a msg to
    //         // raft_gc_log thread to delete the entries before this index instead of deleting them in
    //         // apply thread directly.
    //         return false;
    //     }
    //     return true;
    // }
    //
    // for req in cmd.get_requests() {
    //     // After ingest sst, sst files are deleted quickly. As a result,
    //     // ingest sst command can not be handled again and must be synced.
    //     // See more in Cleanup worker.
    //     if req.has_ingest_sst() {
    //         return true;
    //     }
    // }

    false
}

/// The apply delegate of a Region which is responsible for handling committed
/// raft log entries of a Region.
///
/// `Apply` is a term of Raft, which means executing the actual commands.
/// In Raft, once some log entries are committed, for every peer of the Raft
/// group will apply the logs one by one. For write commands, it does write or
/// delete to local engine; for admin commands, it does some meta change of the
/// Raft group.
///
/// `Delegate` is just a structure to congregate all apply related fields of a
/// Region. The apply worker receives all the apply tasks of different Regions
/// located at this store, and it will get the corresponding apply delegate to
/// handle the apply task to make the code logic more clear.
// #[derive(Derivative)]
// #[derivative(Debug)]
pub struct ApplyDelegate {
    /// The ID of the peer.
    id: u64,
    /// The term of the Region.
    term: u64,
    /// The Region information of the peer.
    region: Region,
    /// Peer_tag, "[region region_id] peer_id".
    tag: String,

    /// The start time of the current round to execute commands.
    handle_start: Option<Instant>,

    /// The commands waiting to be committed and applied
    pending_cmds: PendingCmdQueue,

    /// TiKV writes apply_state to KV RocksDB, in one write batch together with kv data.
    ///
    /// If we write it to Raft RocksDB, apply_state and kv data (Put, Delete) are in
    /// separate WAL file. When power failure, for current raft log, apply_index may synced
    /// to file, but KV data may not synced to file, so we will lose data.
    apply_state: RaftApplyState,
    /// The term of the raft log at applied index.
    applied_index_term: u64,
    /// The latest flushed applied index.
    last_flush_applied_index: u64,

    // /// To fetch Raft entries for applying if necessary.
    // #[derivative(Debug = "ignore")]
    // raft_engine: Box<dyn RaftEngineReadOnly>,

    // wallet_id -> balance.
    wallet_map: HashMap<String, String>,

    // dedup_id -> event_id,
    // dedup_map is used to implement idempotence: when receiving a duplicated request,
    // the previous result should be replied.
    dedup_map: HashMap<String, String>,

    engine: RocksEngine,
}

impl ApplyDelegate {
    fn from_registration(reg: Registration) -> ApplyDelegate {
        // recover wallet map and dedup map from kv engine.
        let mut wallet_map = HashMap::default();
        let mut dedup_map = HashMap::default();

        reg.engine
            .scan(DATA_MIN_KEY, DATA_MAX_KEY, false, |key, value| {
                wallet_map.insert(
                    String::from_utf8(keys::origin_data_key(key).to_owned()).unwrap(),
                    String::from_utf8(value.to_owned()).unwrap(),
                );
                Ok(true)
            })
            .unwrap();

        reg.engine
            .scan(DEDUP_MIN_KEY, DEDUP_MAX_KEY, false, |key, value| {
                dedup_map.insert(
                    String::from_utf8(keys::origin_dedup_key(key).to_owned()).unwrap(),
                    String::from_utf8(value.to_owned()).unwrap(),
                );
                Ok(true)
            })
            .unwrap();

        ApplyDelegate {
            id: reg.id,
            tag: format!("[region {}] {}", reg.region.get_id(), reg.id),
            region: reg.region,
            last_flush_applied_index: reg.apply_state.get_applied_index(),
            apply_state: reg.apply_state,
            applied_index_term: reg.applied_index_term,
            term: reg.term,
            handle_start: None,
            pending_cmds: PendingCmdQueue::new(),
            wallet_map,
            dedup_map,
            engine: reg.engine,
        }
    }

    pub fn region_id(&self) -> u64 {
        self.region.get_id()
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    /// Handles all the committed_entries, namely, applies the committed entries.
    fn handle_raft_committed_entries(
        &mut self,
        apply_ctx: &mut ApplyContext,
        mut committed_entries_drainer: Drain<Entry>,
    ) {
        if committed_entries_drainer.len() == 0 {
            return;
        }
        apply_ctx.prepare_for(self);
        // If we send multiple ConfChange commands, only first one will be proposed correctly,
        // others will be saved as a normal entry with no data, so we must re-propose these
        // commands again.
        apply_ctx.committed_count += committed_entries_drainer.len();
        let mut results = VecDeque::new();
        while let Some(entry) = committed_entries_drainer.next() {
            let expect_index = self.apply_state.get_applied_index() + 1;
            if expect_index != entry.get_index() {
                panic!(
                    "{} expect index {}, but got {}",
                    self.tag,
                    expect_index,
                    entry.get_index()
                );
            }

            // NOTE: before v5.0, `EntryType::EntryConfChangeV2` entry is handled by `unimplemented!()`,
            // which can break compatibility (i.e. old version tikv running on data written by new version tikv),
            // but PD will reject old version tikv join the cluster, so this should not happen.
            let res = match entry.get_entry_type() {
                EntryType::EntryNormal => self.handle_raft_entry_normal(apply_ctx, &entry),
                _ => unreachable!(),
            };

            match res {
                ApplyResult::None => {}
                ApplyResult::Res(res) => results.push_back(res),
            }
        }
        apply_ctx.finish_for(self, results);
    }

    fn write_apply_state(&self, wb: &mut RocksWriteBatch) {
        wb.put_msg_cf(
            CF_RAFT,
            &keys::apply_state_key(self.region.get_id()),
            &self.apply_state,
        )
    }

    fn handle_raft_entry_normal(
        &mut self,
        apply_ctx: &mut ApplyContext,
        entry: &Entry,
    ) -> ApplyResult {
        APPLY_ENTRY_COUNTER.inc();
        let index = entry.get_index();
        let term = entry.get_term();
        let data = entry.get_data();

        if !data.is_empty() {
            let cmd = util::parse_data_at(data, index, &self.tag);
            return self.process_raft_cmd(apply_ctx, index, term, cmd);
        }
        // TOOD(cdc): should we observe empty cmd, aka leader change?

        self.apply_state.set_applied_index(index);
        self.applied_index_term = term;
        assert!(term > 0);

        // 1. When a peer become leader, it will send an empty entry.
        // 2. When a leader tries to read index during transferring leader,
        //    it will also propose an empty entry. But that entry will not contain
        //    any associated callback. So no need to clear callback.
        while let Some(mut cmd) = self.pending_cmds.pop_normal(std::u64::MAX, term - 1) {
            if let Some(cb) = cmd.cb.take() {
                apply_ctx
                    .applied_batch
                    .push_cb(cb, cmd_resp::err_resp(Error::StaleCommand, term));
            }
        }
        ApplyResult::None
    }

    fn find_pending(&mut self, index: u64, term: u64) -> Option<Callback> {
        let (region_id, peer_id) = (self.region_id(), self.id());
        while let Some(mut head) = self.pending_cmds.pop_normal(index, term) {
            if head.term == term {
                if head.index == index {
                    return Some(head.cb.take().unwrap());
                } else {
                    panic!(
                        "{} unexpected callback at term {}, found index {}, expected {}",
                        self.tag, term, head.index, index
                    );
                }
            } else {
                // Because of the lack of original RaftCmdRequest, we skip calling
                // coprocessor here.
                notify_stale_command(region_id, peer_id, self.term, head);
            }
        }
        None
    }

    fn process_raft_cmd(
        &mut self,
        apply_ctx: &mut ApplyContext,
        index: u64,
        term: u64,
        cmd: RaftCmdRequest,
    ) -> ApplyResult {
        if index == 0 {
            panic!(
                "{} processing raft command needs a none zero index",
                self.tag
            );
        }

        // Set sync log hint if the cmd requires so.
        apply_ctx.sync_log_hint |= should_sync_log(&cmd);

        let (mut resp, exec_result) = self.apply_raft_cmd(apply_ctx, index, term, &cmd);

        debug!(
            "applied command";
            "region_id" => self.region_id(),
            "peer_id" => self.id(),
            "index" => index
        );

        // TODO: if we have exec_result, maybe we should return this callback too. Outer
        // store will call it after handing exec result.
        cmd_resp::bind_term(&mut resp, self.term);
        let cmd_cb = self.find_pending(index, term);
        let cmd = Cmd::new(index, cmd, resp);
        apply_ctx.applied_batch.push(cmd_cb, cmd);
        exec_result
    }

    /// Applies raft command.
    ///
    /// An apply operation can fail in the following situations:
    ///   1. it encounters an error that will occur on all stores, it can continue
    /// applying next entry safely, like epoch not match for example;
    ///   2. it encounters an error that may not occur on all stores, in this case
    /// we should try to apply the entry again or panic. Considering that this
    /// usually due to disk operation fail, which is rare, so just panic is ok.
    fn apply_raft_cmd(
        &mut self,
        ctx: &mut ApplyContext,
        index: u64,
        term: u64,
        req: &RaftCmdRequest,
    ) -> (RaftCmdResponse, ApplyResult) {
        ctx.exec_ctx = Some(self.new_ctx(index, term));
        ctx.kv_wb_mut().set_save_point();

        let (resp, exec_result) = match self.exec_raft_cmd(ctx, &req) {
            Ok(a) => {
                ctx.kv_wb_mut().pop_save_point();
                a
            }
            Err(e) => {
                // clear dirty values.
                ctx.kv_wb_mut().rollback_to_save_point();
                match e {
                    // todo(glengeng): pick the error! macro back later.
                    // _ => error!(?e;
                    //     "execute raft command";
                    //     "region_id" => self.region_id(),
                    //     "peer_id" => self.id(),
                    // ),
                    _ => warn!(
                        "execute raft command";
                        "region_id" => self.region_id(),
                        "peer_id" => self.id(),
                    ),
                }
                (cmd_resp::new_error(e), ApplyResult::None)
            }
        };

        let mut exec_ctx = ctx.exec_ctx.take().unwrap();
        exec_ctx.apply_state.set_applied_index(index);

        self.apply_state = exec_ctx.apply_state;
        self.applied_index_term = term;

        (resp, exec_result)
    }

    fn clear_all_commands_as_stale(&mut self) {
        let (region_id, peer_id) = (self.region_id(), self.id());
        for cmd in self.pending_cmds.normals.drain(..) {
            notify_stale_command(region_id, peer_id, self.term, cmd);
        }
    }

    fn new_ctx(&self, index: u64, term: u64) -> ExecContext {
        ExecContext::new(self.apply_state.clone(), index, term)
    }
}

impl ApplyDelegate {
    // Only errors that will also occur on all other stores should be returned.
    fn exec_raft_cmd(
        &mut self,
        ctx: &mut ApplyContext,
        req: &RaftCmdRequest,
    ) -> Result<(RaftCmdResponse, ApplyResult)> {
        // // Include region for epoch not match after merge may cause key not in range.
        // let include_region =
        //     req.get_header().get_region_epoch().get_version() >= self.last_merge_version;
        // check_region_epoch(req, &self.region, include_region)?;
        if req.has_admin_request() {
            self.exec_admin_cmd(ctx, req)
        } else {
            self.exec_write_cmd(ctx, req)
        }
    }

    fn exec_admin_cmd(
        &mut self,
        ctx: &mut ApplyContext,
        req: &RaftCmdRequest,
    ) -> Result<(RaftCmdResponse, ApplyResult)> {
        let request = req.get_admin_request();
        let cmd_type = request.get_cmd_type();
        // if cmd_type != AdminCmdType::CompactLog && cmd_type != AdminCmdType::CommitMerge {
        info!(
            "execute admin command";
            "region_id" => self.region_id(),
            "peer_id" => self.id(),
            "term" => ctx.exec_ctx.as_ref().unwrap().term,
            "index" => ctx.exec_ctx.as_ref().unwrap().index,
            "command" => ?request,
        );
        // }

        let (mut response, exec_result) = match cmd_type {
            // AdminCmdType::ChangePeer => self.exec_change_peer(ctx, request),
            // AdminCmdType::ChangePeerV2 => self.exec_change_peer_v2(ctx, request),
            // AdminCmdType::Split => self.exec_split(ctx, request),
            // AdminCmdType::BatchSplit => self.exec_batch_split(ctx, request),
            AdminCmdType::CompactLog => self.exec_compact_log(ctx, request),
            // AdminCmdType::TransferLeader => Err(box_err!("transfer leader won't exec")),
            // AdminCmdType::ComputeHash => self.exec_compute_hash(ctx, request),
            // AdminCmdType::VerifyHash => self.exec_verify_hash(ctx, request),
            // // TODO: is it backward compatible to add new cmd_type?
            // AdminCmdType::PrepareMerge => self.exec_prepare_merge(ctx, request),
            // AdminCmdType::CommitMerge => self.exec_commit_merge(ctx, request),
            // AdminCmdType::RollbackMerge => self.exec_rollback_merge(ctx, request),
            AdminCmdType::InvalidAdmin => Err(box_err!("unsupported admin command type")),
        }?;
        response.set_cmd_type(cmd_type);

        let mut resp = RaftCmdResponse::default();
        if !req.get_header().get_uuid().is_empty() {
            let uuid = req.get_header().get_uuid().to_vec();
            resp.mut_header().set_uuid(uuid);
        }
        resp.set_admin_response(response);
        Ok((resp, exec_result))
    }

    fn exec_write_cmd(
        &mut self,
        ctx: &mut ApplyContext,
        req: &RaftCmdRequest,
    ) -> Result<(RaftCmdResponse, ApplyResult)> {
        let requests = req.get_requests();
        APPLY_EVENT_COUNTER.inc_by(requests.len() as u64);
        let mut responses = Vec::with_capacity(requests.len());

        let index = ctx.exec_ctx.as_ref().unwrap().index;
        for (offset, req) in requests.into_iter().enumerate() {
            let cmd_type = req.get_cmd_type();
            let mut resp = match cmd_type {
                CmdType::MessageQueue => {
                    self.handle_message_queue(ctx.kv_wb_mut(), req, index, offset as u64)
                }
                _ => unreachable!(),
            }?;

            resp.set_cmd_type(cmd_type);

            responses.push(resp);
        }

        let mut resp = RaftCmdResponse::default();
        if !req.get_header().get_uuid().is_empty() {
            let uuid = req.get_header().get_uuid().to_vec();
            resp.mut_header().set_uuid(uuid);
        }
        resp.set_responses(responses.into());

        Ok((resp, ApplyResult::None))
    }
}

// Write commands related.
impl ApplyDelegate {
    fn handle_message_queue(
        &mut self,
        wb: &mut RocksWriteBatch,
        req: &Request,
        index: u64,
        offset: u64,
    ) -> Result<Response> {
        let mut message_queue_resp = MessageQueueResponse::default();
        message_queue_resp.set_error("OK".to_string());

        // set index and offset
        message_queue_resp.set_index(index);
        message_queue_resp.set_offset(offset);

        trace!(
            "After handle_message_queue, the message queue response is {:?}",
            message_queue_resp
        );
        let mut resp = Response::default();
        resp.set_message_queue(message_queue_resp);

        Ok(resp)
    }
}

// Admin commands related.
impl ApplyDelegate {
    fn exec_compact_log(
        &mut self,
        ctx: &mut ApplyContext,
        req: &AdminRequest,
    ) -> Result<(AdminResponse, ApplyResult)> {
        // PEER_ADMIN_CMD_COUNTER.compact.all.inc();

        let compact_index = req.get_compact_log().get_compact_index();
        let resp = AdminResponse::default();
        let apply_state = &mut ctx.exec_ctx.as_mut().unwrap().apply_state;
        let first_index = peer_storage::first_index(apply_state);
        if compact_index <= first_index {
            debug!(
                "compact index <= first index, no need to compact";
                "region_id" => self.region_id(),
                "peer_id" => self.id(),
                "compact_index" => compact_index,
                "first_index" => first_index,
            );
            return Ok((resp, ApplyResult::None));
        }
        // if self.is_merging {
        //     info!(
        //         "in merging mode, skip compact";
        //         "region_id" => self.region_id(),
        //         "peer_id" => self.id(),
        //         "compact_index" => compact_index
        //     );
        //     return Ok((resp, ApplyResult::None));
        // }

        let compact_term = req.get_compact_log().get_compact_term();
        // TODO: add unit tests to cover all the message integrity checks.
        if compact_term == 0 {
            info!(
                "compact term missing, skip";
                "region_id" => self.region_id(),
                "peer_id" => self.id(),
                "command" => ?req.get_compact_log()
            );
            // old format compact log command, safe to ignore.
            return Err(box_err!(
                "command format is outdated, please upgrade leader"
            ));
        }

        // compact failure is safe to be omitted, no need to assert.
        compact_raft_log(&self.tag, apply_state, compact_index, compact_term)?;

        // PEER_ADMIN_CMD_COUNTER.compact.success.inc();

        Ok((
            resp,
            ApplyResult::Res(ExecResult::CompactLog {
                state: apply_state.get_truncated_state().clone(),
                first_index,
            }),
        ))
    }
}

/// Updates the `state` with given `compact_index` and `compact_term`.
///
/// Remember the Raft log is not deleted here.
pub fn compact_raft_log(
    tag: &str,
    state: &mut RaftApplyState,
    compact_index: u64,
    compact_term: u64,
) -> Result<()> {
    debug!("{} compact log entries to prior to {}", tag, compact_index);

    if compact_index <= state.get_truncated_state().get_index() {
        return Err(box_err!("try to truncate compacted entries"));
    } else if compact_index > state.get_applied_index() {
        return Err(box_err!(
            "compact index {} > applied index {}",
            compact_index,
            state.get_applied_index()
        ));
    }

    // we don't actually delete the logs now, we add an async task to do it.

    state.mut_truncated_state().set_index(compact_index);
    state.mut_truncated_state().set_term(compact_term);

    Ok(())
}

pub struct Apply {
    pub peer_id: u64,
    pub region_id: u64,
    pub term: u64,
    pub entries: CachedEntries,
    pub cbs: Vec<Proposal>,
}

impl Apply {
    pub(crate) fn new(
        peer_id: u64,
        region_id: u64,
        term: u64,
        entries: Vec<Entry>,
        cbs: Vec<Proposal>,
    ) -> Apply {
        let entries = CachedEntries::new(entries);
        Apply {
            peer_id,
            region_id,
            term,
            entries,
            cbs,
        }
    }
}

pub struct Registration {
    pub id: u64,
    pub term: u64,
    pub apply_state: RaftApplyState,
    pub applied_index_term: u64,
    pub region: Region,
    // pub pending_request_snapshot_count: Arc<AtomicUsize>,
    // pub is_merging: bool,
    // raft_engine: Box<dyn RaftEngineReadOnly>,
    pub engine: RocksEngine,
}

impl Registration {
    pub fn new(peer: &Peer) -> Registration {
        Registration {
            id: peer.peer_id(),
            term: peer.term(),
            apply_state: peer.get_store().apply_state().clone(),
            applied_index_term: peer.get_store().applied_index_term(),
            region: peer.region().clone(),
            // pending_request_snapshot_count: peer.pending_request_snapshot_count.clone(),
            // is_merging: peer.pending_merge_state.is_some(),
            // raft_engine: Box::new(peer.get_store().engines.raft.clone()),
            engine: peer.get_store().engines.kv.clone(),
        }
    }
}

pub struct Proposal {
    pub is_conf_change: bool,
    pub index: u64,
    pub term: u64,
    pub cb: Callback,
    /// `propose_time` is set to the last time when a peer starts to renew lease.
    pub propose_time: Option<Timespec>,
    pub must_pass_epoch_check: bool,
}

pub enum Msg {
    Apply { start: Instant, apply: Apply },
    Registration(Registration),
    Noop,
    // Snapshot(GenSnapTask),
}

impl Msg {
    pub fn apply(apply: Apply) -> Msg {
        Msg::Apply {
            start: Instant::now(),
            apply,
        }
    }

    pub fn register(peer: &Peer) -> Msg {
        Msg::Registration(Registration::new(peer))
    }
}

impl Debug for Msg {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Msg::Apply { apply, .. } => write!(f, "[region {}] async apply", apply.region_id),
            Msg::Registration(ref r) => {
                write!(f, "[region {}] Reg {:?}", r.region.get_id(), r.apply_state)
            }
            Msg::Noop => write!(f, "noop"),
            // Msg::Snapshot(GenSnapTask { region_id, .. }) => {
            //     write!(f, "[region {}] requests a snapshot", region_id)
            // }
        }
    }
}

#[derive(Debug)]
pub struct ApplyRes {
    pub region_id: u64,
    pub apply_state: RaftApplyState,
    pub applied_index_term: u64,
    pub exec_res: VecDeque<ExecResult>,
}

#[derive(Debug)]
pub enum TaskRes {
    Apply(ApplyRes),
}

pub struct ApplyFsm {
    delegate: ApplyDelegate,
    receiver: channel::Receiver<Msg>,
    // mailbox: Option<BasicMailbox<ApplyFsm>>,
}

impl ApplyFsm {
    // fn from_peer(
    //     peer: &Peer,
    // // ) -> (LooseBoundedSender<Msg>, Box<ApplyFsm>) {
    // ) -> (channel::Sender<Msg>, ApplyFsm) {
    //     let reg = Registration::new(peer);
    //     ApplyFsm::from_registration(reg)
    // }
    //
    // fn from_registration(reg: Registration) -> (channel::Sender<Msg>, ApplyFsm) {
    //     let (tx, rx) = channel::unbounded();
    //     let delegate = ApplyDelegate::from_registration(reg);
    //     (
    //         tx,
    //         ApplyFsm {
    //             delegate,
    //             receiver: rx,
    //             // mailbox: None,
    //         },
    //     )
    // }

    fn from_peer(peer: &Peer, receiver: channel::Receiver<Msg>) -> ApplyFsm {
        let reg = Registration::new(peer);
        let delegate = ApplyDelegate::from_registration(reg);

        ApplyFsm {
            delegate,
            receiver,
            // mailbox: None,
        }
    }

    /// Handles peer registration. When a peer is created, it will register an apply delegate.
    fn handle_registration(&mut self, reg: Registration) {
        unreachable!();
        info!(
            "re-register to apply delegates";
            "region_id" => self.delegate.region_id(),
            "peer_id" => self.delegate.id(),
            "term" => reg.term
        );
        assert_eq!(self.delegate.id, reg.id);
        self.delegate.term = reg.term;
        self.delegate.clear_all_commands_as_stale();
        self.delegate = ApplyDelegate::from_registration(reg);
    }

    /// Handles apply tasks, and uses the apply delegate to handle the committed entries.
    fn handle_apply(&mut self, apply_ctx: &mut ApplyContext, mut apply: Apply) {
        if apply_ctx.timer.is_none() {
            apply_ctx.timer = Some(Instant::now_coarse());
        }

        let (mut entries, _dangle_size) = apply.entries.take_entries();

        if entries.is_empty() {
            unreachable!()
        }

        self.delegate.term = apply.term;
        if let Some(entry) = entries.last() {
            let prev_state = (
                self.delegate.apply_state.get_commit_index(),
                self.delegate.apply_state.get_commit_term(),
            );
            let cur_state = (entry.get_index(), entry.get_term());
            if prev_state.0 > cur_state.0 || prev_state.1 > cur_state.1 {
                panic!(
                    "{} commit state jump backward {:?} -> {:?}",
                    self.delegate.tag, prev_state, cur_state
                );
            }
            self.delegate.apply_state.set_commit_index(cur_state.0);
            self.delegate.apply_state.set_commit_term(cur_state.1);
        }

        self.append_proposal(apply.cbs.drain(..));
        self.delegate
            .handle_raft_committed_entries(apply_ctx, entries.drain(..));
    }

    /// Handles proposals, and appends the commands to the apply delegate.
    fn append_proposal(&mut self, props_drainer: Drain<Proposal>) {
        for p in props_drainer {
            let cmd = PendingCmd::new(p.index, p.term, p.cb);
            self.delegate.pending_cmds.append_normal(cmd);
        }
    }

    fn handle_tasks(&mut self, apply_ctx: &mut ApplyContext, msgs: &mut Vec<Msg>) {
        if !msgs.is_empty() {
            debug!(
                "handle_tasks of applyLoop starts to handle {} tasks.",
                msgs.len()
            );
        }

        let mut drainer = msgs.drain(..);
        loop {
            match drainer.next() {
                Some(Msg::Apply { start, apply }) => {
                    self.handle_apply(apply_ctx, apply);
                }
                Some(Msg::Registration(reg)) => self.handle_registration(reg),
                Some(Msg::Noop) => {}
                // Some(Msg::Snapshot(snap_task)) => self.handle_snapshot(apply_ctx, snap_task),
                None => break,
            }
        }
    }
}

impl Drop for ApplyFsm {
    fn drop(&mut self) {
        self.delegate.clear_all_commands_as_stale();
    }
}

pub struct ApplyPoller {
    msg_buf: Vec<Msg>,
    apply_ctx: ApplyContext,
    messages_per_tick: usize,
}

impl ApplyPoller {
    fn begin(&mut self) {
        self.messages_per_tick = 4096; // todo(glengeng): magic num 4096 is copied from configuration.
    }

    fn handle_normal(&mut self, normal: &mut ApplyFsm) {
        normal.delegate.handle_start = Some(Instant::now_coarse());

        while self.msg_buf.len() < self.messages_per_tick {
            match normal.receiver.try_recv() {
                Ok(msg) => self.msg_buf.push(msg),
                Err(TryRecvError::Empty) => {
                    break;
                }
                Err(TryRecvError::Disconnected) => {
                    unreachable!() // todo(glengeng): the system can't work if the channel is disconnected
                }
            }
        }

        normal.handle_tasks(&mut self.apply_ctx, &mut self.msg_buf);
    }

    fn end(&mut self, fsm: &mut ApplyFsm) {
        self.apply_ctx.flush();
        fsm.delegate.last_flush_applied_index = fsm.delegate.apply_state.get_applied_index();
    }
}

pub struct ApplyPollerBuilder {
    tag: String,
    engine: RocksEngine,
    sender: Box<dyn Notifier>,
    router: ApplyRouter,
    store_id: u64,
}

impl ApplyPollerBuilder {
    pub fn new(
        builder: &RaftPollerBuilder,
        sender: Box<dyn Notifier>,
        router: ApplyRouter,
    ) -> ApplyPollerBuilder {
        ApplyPollerBuilder {
            tag: format!("[store {}]", builder.store_id),
            engine: builder.engines.kv.clone(),
            sender,
            router,
            store_id: builder.store_id,
        }
    }
}

impl ApplyPollerBuilder {
    fn build(&mut self) -> ApplyPoller {
        ApplyPoller {
            msg_buf: Vec::with_capacity(4096),
            apply_ctx: ApplyContext::new(
                self.tag.clone(),
                self.engine.clone(),
                self.router.clone(),
                self.sender.clone_box(),
                self.store_id,
            ),
            messages_per_tick: 4096,
        }
    }
}

#[derive(Clone)]
pub struct ApplyRouter {
    pub router: channel::Sender<Msg>,
}

impl ApplyRouter {
    pub fn schedule_task(&self, _region_id: u64, msg: Msg) {
        self.router.send(msg).unwrap();
    }
}

/// A system that can poll FSMs concurrently and in batch.
///
/// To use the system, two type of FSMs and their PollHandlers need
/// to be defined: Normal and Control. Normal FSM handles the general
/// task while Control FSM creates normal FSM instances.
pub struct ApplySystem {
    name_prefix: Option<String>,
    router: ApplyRouter,
    receiver: channel::Receiver<Msg>,
    fsm: Option<ApplyFsm>,
}

impl ApplySystem {
    pub fn router(&self) -> &ApplyRouter {
        &self.router
    }

    fn start_poller(
        &mut self,
        name: String,
        /*priority: Priority,*/ builder: &mut ApplyPollerBuilder,
    ) {
        let handler = builder.build(/*priority*/);

        let mut poller = Poller {
            handler,
            fsm: self.fsm.take().unwrap(),
        };
        let _t = thread::Builder::new()
            .name(name)
            .spawn(move || {
                poller.poll();
            })
            .unwrap();
    }

    /// Start the batch system.
    pub fn spawn(&mut self, name_prefix: String, mut builder: ApplyPollerBuilder) {
        self.start_poller(
            thd_name!(format!("{}-{}", name_prefix, 0)),
            // Priority::Normal,
            &mut builder,
        );
        self.name_prefix = Some(name_prefix);
    }

    /// Shutdown the batch system and wait till all background threads exit.
    pub fn shutdown(&mut self) {
        unreachable!()
    }

    pub fn schedule_all<'a>(&mut self, peers: impl Iterator<Item = &'a Peer>) {
        let mut len = 0;

        for peer in peers {
            let fsm = ApplyFsm::from_peer(peer, self.receiver.clone());
            self.fsm = Some(fsm);

            len += 1;
            assert_eq!(len, 1); // ugly, but valid iff under single raft.
        }
    }
}

/// Internal poller that fetches batch and call handler hooks for readiness.
struct Poller {
    handler: ApplyPoller,
    fsm: ApplyFsm,
}

impl Poller {
    fn poll(&mut self) {
        loop {
            let t = Instant::now_coarse();

            self.handler.begin();
            self.handler.handle_normal(&mut self.fsm);
            self.handler.end(&mut self.fsm);

            // yield CPU, avoid busy wait
            let interval_in_us = 50 as u64;
            let elapsed_in_us = t.elapsed().as_micros() as u64;
            if elapsed_in_us < interval_in_us {
                thread::sleep(Duration::from_micros(interval_in_us - elapsed_in_us));
            }
        }
    }
}

/// Create a batch system with the given thread name prefix and pool size.
///
/// `sender` and `controller` should be paired.
pub fn create_apply_system() -> (ApplyRouter, ApplySystem) {
    let (tx, rx) = channel::unbounded();
    let router = ApplyRouter { router: tx };
    let system = ApplySystem {
        name_prefix: None,
        router: router.clone(),
        receiver: rx,
        fsm: None,
    };
    (router, system)
}
