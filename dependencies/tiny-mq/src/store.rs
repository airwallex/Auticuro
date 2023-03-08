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

// use std::cell::Cell;
// use std::cmp::{Ord, Ordering as CmpOrdering};
// use std::collections::{BTreeMap, HashMap};
// use std::collections::Bound::{Excluded, Included, Unbounded};
// use std::ops::Deref;
// use std::sync::atomic::Ordering;
// use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::{mem, u64, thread};

// use batch_system::{
//     BasicMailbox, BatchRouter, BatchSystem, Fsm, HandlerBuilder, PollHandler, Priority,
// };
use crossbeam::channel::{self, TryRecvError, TrySendError};
// use engine_traits::PerfContext;
// use engine_traits::PerfContextKind;
// use engine_traits::{Engines, KvEngine, Mutable, WriteBatch, WriteBatchExt, WriteOptions};
// use engine_traits::{CF_DEFAULT, CF_LOCK, CF_RAFT, CF_WRITE};
// use fail::fail_point;
use futures::compat::Future01CompatExt;
use futures::FutureExt;
// use kvproto::import_sstpb::SstMeta;
use kvproto::metapb::{self, Region, RegionEpoch};
// use kvproto::pdpb::QueryStats;
// use kvproto::pdpb::StoreStats;
use kvproto::raft_cmdpb::{RaftCmdRequest, };
// use kvproto::raft_serverpb::{ExtraMessageType, PeerState, RaftMessage, RegionLocalState};
use kvproto::raft_serverpb::{RaftMessage, RegionLocalState};
// use kvproto::replication_modepb::{ReplicationMode, ReplicationStatus};
use protobuf::Message;
use raft::StateRole;
use time::{self, Timespec};

// use collections::HashMap;
// use engine_traits::CompactedEvent;
// use engine_traits::{RaftEngine, RaftLogBatch};
// use keys::{self, data_end_key, data_key, enc_end_key, enc_start_key};
// use pd_client::{FeatureGate, PdClient};
// use sst_importer::SSTImporter;
// use tikv_alloc::trace::TraceEvent;
// use tikv_util::config::{Tracker, VersionTrack};
// use tikv_util::mpsc::{self, LooseBoundedSender, Receiver};
// use tikv_util::sys::disk;
use tikv_util::time::Instant as TiInstant;
// use tikv_util::timer::SteadyTimer;
// use tikv_util::worker::{FutureScheduler, FutureWorker, Scheduler, Worker};
use tikv_util::worker::{Scheduler, Worker};
// use tikv_util::{
//     box_err, box_try, debug, defer, error, info, is_zero_duration, slow_log, sys as sys_util, warn,
//     Either, RingQueue,
// };
use tikv_util::{box_err, debug, error, info, slow_log, warn, Either, thd_name};

use crate::util::bytes_capacity;
// use crate::coprocessor::split_observer::SplitObserver;
// use crate::coprocessor::{BoxAdminObserver, CoprocessorHost, RegionChangeEvent};
// use crate::store::config::Config;
// use crate::store::fsm::metrics::*;
// use crate::store::fsm::peer::{
//     maybe_destroy_source, new_admin_request, PeerFsm, PeerFsmDelegate, SenderFsmPair,
// };
// use crate::store::fsm::ApplyNotifier;
use crate::apply::{Notifier as ApplyNotifier, TaskRes as ApplyTaskRes, ApplySystem, ApplyPollerBuilder, create_apply_system};
// use crate::store::fsm::ApplyTaskRes;
// use crate::store::fsm::{
//     create_apply_batch_system, ApplyBatchSystem, ApplyPollerBuilder, ApplyRes, ApplyRouter,
//     CollectedReady,
// };
// use crate::store::local_metrics::RaftMetrics;
// use crate::store::memory::*;
// use crate::store::metrics::*;
// use crate::store::peer_storage::{self, HandleRaftReadyContext};
use crate::peer_storage::{self, HandleRaftReadyContext};
// use crate::store::transport::Transport;
// use crate::store::util::{is_initial_msg, RegionReadProgressRegistry};
// use crate::store::worker::{
//     AutoSplitController, CleanupRunner, CleanupSSTRunner, CleanupSSTTask, CleanupTask,
//     CompactRunner, CompactTask, ConsistencyCheckRunner, ConsistencyCheckTask, PdRunner,
//     RaftlogGcRunner, RaftlogGcTask, ReadDelegate, RegionRunner, RegionTask, SplitCheckTask,
// };
use crate::raftlog_gc::{Runner as RaftlogGcRunner, Task as RaftlogGcTask};
// use crate::store::{
//     util, Callback, CasualMessage, GlobalReplicationState, InspectedRaftMessage, MergeResultKind,
//     PdTask, PeerTicks, RaftCommand, SignificantMsg, SnapManager, StoreMsg, StoreTick,
// };
use crate::msg::{PeerMsg, PeerTicks, InspectedRaftMessage, RaftCommand, Callback};
// use concurrency_manager::ConcurrencyManager;
// use tikv_util::future::poll_future_notify;
use crate::peer::{CollectedReady, PeerFsm, PeerFsmDelegate};
use crate::rocks_engine::{RocksWriteBatch, CF_RAFT, Engines};
use crate::raft_client::RaftClient;
use crate::apply::{ApplyRes, ApplyRouter};
use tikv_util::timer::SteadyTimer;
use tikv_util::future::poll_future_notify;
use crate::errors::{Result, Error};
use crate::keys;


type Key = Vec<u8>;

const KV_WB_SHRINK_SIZE: usize = 256 * 1024;
const RAFT_WB_SHRINK_SIZE: usize = 1024 * 1024;
pub const PENDING_MSG_CAP: usize = 100;
const UNREACHABLE_BACKOFF: Duration = Duration::from_secs(10);
const ENTRY_CACHE_EVICT_TICK_DURATION: Duration = Duration::from_secs(1);

// pub struct StoreInfo<E> {
//     pub engine: E,
//     pub capacity: u64,
// }

// pub struct StoreMeta {
//     /// store id
//     pub store_id: Option<u64>,
//     /// region_end_key -> region_id
//     pub region_ranges: BTreeMap<Vec<u8>, u64>,
//     /// region_id -> region
//     pub regions: HashMap<u64, Region>,
//     /// region_id -> reader
//     pub readers: HashMap<u64, ReadDelegate>,
//     /// region_id -> (term, leader_peer_id)
//     pub leaders: HashMap<u64, (u64, u64)>,
//     /// `MsgRequestPreVote`, `MsgRequestVote` or `MsgAppend` messages from newly split Regions shouldn't be
//     /// dropped if there is no such Region in this store now. So the messages are recorded temporarily and
//     /// will be handled later.
//     pub pending_msgs: RingQueue<RaftMessage>,
//     /// The regions with pending snapshots.
//     pub pending_snapshot_regions: Vec<Region>,
//     /// A marker used to indicate the peer of a Region has received a merge target message and waits to be destroyed.
//     /// target_region_id -> (source_region_id -> merge_target_region)
//     pub pending_merge_targets: HashMap<u64, HashMap<u64, metapb::Region>>,
//     /// An inverse mapping of `pending_merge_targets` used to let source peer help target peer to clean up related entry.
//     /// source_region_id -> target_region_id
//     pub targets_map: HashMap<u64, u64>,
//     /// `atomic_snap_regions` and `destroyed_region_for_snap` are used for making destroy overlapped regions
//     /// and apply snapshot atomically.
//     /// region_id -> wait_destroy_regions_map(source_region_id -> is_ready)
//     /// A target peer must wait for all source peer to ready before applying snapshot.
//     pub atomic_snap_regions: HashMap<u64, HashMap<u64, bool>>,
//     /// source_region_id -> need_atomic
//     /// Used for reminding the source peer to switch to ready in `atomic_snap_regions`.
//     pub destroyed_region_for_snap: HashMap<u64, bool>,
//     /// region_id -> `RegionReadProgress`
//     pub region_read_progress: RegionReadProgressRegistry,
// }

pub struct RaftRouter
{
    // pub router: BatchRouter<PeerFsm, StoreFsm>,
    pub router: channel::Sender<PeerMsg>,
}

impl Clone for RaftRouter
{
    fn clone(&self) -> Self {
        RaftRouter {
            router: self.router.clone(),
        }
    }
}

// impl<EK, ER> Deref for RaftRouter
// {
//     type Target = BatchRouter<PeerFsm, StoreFsm>;
//
//     fn deref(&self) -> &BatchRouter<PeerFsm, StoreFsm> {
//         &self.router
//     }
// }

impl ApplyNotifier for RaftRouter
{
    fn notify(&self, apply_res: Vec<ApplyRes>) {
        for r in apply_res {
            // self.router.try_send(
            //     r.region_id,
            //     PeerMsg::ApplyRes {
            //         res: ApplyTaskRes::Apply(r),
            //     },
            // );
            self.router.try_send(
                // r.region_id,
                PeerMsg::ApplyRes {
                    res: ApplyTaskRes::Apply(r),
                },
            ).unwrap();
        }
    }
    fn notify_one(&self, region_id: u64, msg: PeerMsg) {
        unreachable!()
        // self.router.try_send(region_id, msg);
    }

    fn clone_box(&self) -> Box<dyn ApplyNotifier> {
        Box::new(self.clone())
    }
}

impl RaftRouter
{
    pub fn send_raft_message(
        &self,
        msg: RaftMessage,
    ) -> std::result::Result<(), TrySendError<RaftMessage>> {
        let id = msg.get_region_id();

        let mut heap_size = 0;
        for e in msg.get_message().get_entries() {
            heap_size += bytes_capacity(&e.data) + bytes_capacity(&e.context);
        }
        let peer_msg = PeerMsg::RaftMessage(InspectedRaftMessage { heap_size, msg });
        // let event = TraceEvent::Add(heap_size);

        self.router.try_send(peer_msg).unwrap();
        Ok(())
        // let store_msg = match self.try_send(id, peer_msg) {
        //     Either::Left(Ok(())) => {
        //         // MEMTRACE_RAFT_MESSAGES.trace(event);
        //         return Ok(());
        //     }
        //     Either::Left(Err(TrySendError::Full(PeerMsg::RaftMessage(im)))) => {
        //         return Err(TrySendError::Full(im.msg));
        //     }
        //     Either::Left(Err(TrySendError::Disconnected(PeerMsg::RaftMessage(im)))) => {
        //         return Err(TrySendError::Disconnected(im.msg));
        //     }
        //     Either::Right(PeerMsg::RaftMessage(im)) => StoreMsg::RaftMessage(im),
        //     _ => unreachable!(),
        // };
        // match self.send_control(store_msg) {
        //     Ok(()) => {
        //         MEMTRACE_RAFT_MESSAGES.trace(event);
        //         Ok(())
        //     }
        //     Err(TrySendError::Full(StoreMsg::RaftMessage(im))) => Err(TrySendError::Full(im.msg)),
        //     Err(TrySendError::Disconnected(StoreMsg::RaftMessage(im))) => {
        //         Err(TrySendError::Disconnected(im.msg))
        //     }
        //     _ => unreachable!(),
        // }
    }

    // #[inline]
    // pub fn send_raft_command(
    //     &self,
    //     cmd: RaftCommand<EK::Snapshot>,
    // ) -> std::result::Result<(), TrySendError<RaftCommand<EK::Snapshot>>> {
    //     let region_id = cmd.request.get_header().get_region_id();
    //     match self.send(region_id, PeerMsg::RaftCommand(cmd)) {
    //         Ok(()) => Ok(()),
    //         Err(TrySendError::Full(PeerMsg::RaftCommand(cmd))) => Err(TrySendError::Full(cmd)),
    //         Err(TrySendError::Disconnected(PeerMsg::RaftCommand(cmd))) => {
    //             Err(TrySendError::Disconnected(cmd))
    //         }
    //         _ => unreachable!(),
    //     }
    // }

    #[inline]
    pub fn send_raft_command(
        &self,
        cmd: RaftCommand,
    ) -> std::result::Result<(), TrySendError<RaftCommand>> {
        // let region_id = cmd.request.get_header().get_region_id();
        self.router.send(PeerMsg::RaftCommand(cmd)).unwrap();
        Ok(())
    }

    /// Sends RaftCmdRequest to local store.
    pub fn send_command(&self, req: RaftCmdRequest, cb: Callback) -> Result<()> {
        let region_id = req.get_header().get_region_id();
        let mut cmd = RaftCommand::new(req, cb);
        // cmd.deadline = deadline;
        // self.send_raft_command(cmd).map_err(Error::from)
        self.send_raft_command(cmd).unwrap();
        Ok(())
    }

    // fn report_unreachable(&self, store_id: u64) {
    //     self.broadcast_normal(|| {
    //         PeerMsg::SignificantMsg(SignificantMsg::StoreUnreachable { store_id })
    //     });
    // }

    // fn report_status_update(&self) {
    //     self.broadcast_normal(|| PeerMsg::UpdateReplicationMode)
    // }

    // /// Broadcasts resolved result to all regions.
    // pub fn report_resolved(&self, store_id: u64, group_id: u64) {
    //     self.broadcast_normal(|| {
    //         PeerMsg::SignificantMsg(SignificantMsg::StoreResolved { store_id, group_id })
    //     })
    // }

    // pub fn register(&self, region_id: u64, mailbox: BasicMailbox<PeerFsm<EK, ER>>) {
    //     self.router.register(region_id, mailbox);
    //     self.update_trace();
    // }

    // pub fn register_all(&self, mailboxes: Vec<(u64, BasicMailbox<PeerFsm<EK, ER>>)>) {
    //     self.router.register_all(mailboxes);
    //     self.update_trace();
    // }

    // pub fn close(&self, region_id: u64) {
    //     self.router.close(region_id);
    //     self.update_trace();
    // }

    // pub fn clear_cache(&self) {
    //     self.router.clear_cache();
    // }

    // fn update_trace(&self) {
    //     let router_trace = self.router.trace();
    //     MEMTRACE_RAFT_ROUTER_ALIVE.trace(TraceEvent::Reset(router_trace.alive));
    //     MEMTRACE_RAFT_ROUTER_LEAK.trace(TraceEvent::Reset(router_trace.leak));
    // }
}

#[derive(Default)]
pub struct PeerTickBatch {
    pub ticks: Vec<Box<dyn FnOnce() + Send>>,
    pub wait_duration: Duration,
}

impl Clone for PeerTickBatch {
    fn clone(&self) -> PeerTickBatch {
        PeerTickBatch {
            ticks: vec![],
            wait_duration: self.wait_duration,
        }
    }
}

pub struct PollContext
{
    /// The count of processed normal Fsm.
    pub processed_fsm_count: usize,
    // pub cfg: Config,
    // pub store: metapb::Store,
    // pub pd_scheduler: FutureScheduler<PdTask<EK>>,
    // pub consistency_check_scheduler: Scheduler<ConsistencyCheckTask<EK::Snapshot>>,
    // pub split_check_scheduler: Scheduler<SplitCheckTask>,
    // handle Compact, CleanupSST task
    // pub cleanup_scheduler: Scheduler<CleanupTask>,
    pub raftlog_gc_scheduler: Scheduler<RaftlogGcTask>,
    // pub region_scheduler: Scheduler<RegionTask<EK::Snapshot>>,
    pub apply_router: ApplyRouter,
    pub router: RaftRouter,
    // pub importer: Arc<SSTImporter>,
    // pub store_meta: Arc<Mutex<StoreMeta>>,
    // pub feature_gate: FeatureGate,
    // /// region_id -> (peer_id, is_splitting)
    // /// Used for handling race between splitting and creating new peer.
    // /// An uninitialized peer can be replaced to the one from splitting iff they are exactly the same peer.
    // ///
    // /// WARNING:
    // /// To avoid deadlock, if you want to use `store_meta` and `pending_create_peers` together,
    // /// the lock sequence MUST BE:
    // /// 1. lock the store_meta.
    // /// 2. lock the pending_create_peers.
    // pub pending_create_peers: Arc<Mutex<HashMap<u64, (u64, bool)>>>,
    // pub raft_metrics: RaftMetrics,
    // pub snap_mgr: SnapManager,
    // pub coprocessor_host: CoprocessorHost<EK>,
    pub timer: SteadyTimer,
    pub trans: RaftClient,
    // pub global_replication_state: Arc<Mutex<GlobalReplicationState>>,
    // pub global_stat: GlobalStoreStat,
    // pub store_stat: LocalStoreStat,
    pub engines: Engines,
    pub kv_wb: RocksWriteBatch,
    pub raft_wb: RocksWriteBatch,
    pub pending_count: usize,
    pub sync_log: bool,
    pub has_ready: bool,
    pub ready_res: Vec<CollectedReady>,
    pub current_time: Option<Timespec>,
    // pub perf_context: EK::PerfContext,
    pub tick_batch: Vec<PeerTickBatch>,
    // pub node_start_time: Option<TiInstant>,
    // pub is_disk_full: bool,
}

// impl<EK, ER, T> HandleRaftReadyContext<EK::WriteBatch, ER::LogBatch> for PollContext<EK, ER, T>
//     where
//         EK: KvEngine,
//         ER: RaftEngine,
impl HandleRaftReadyContext for PollContext
{
    fn wb_mut(&mut self) -> (&mut RocksWriteBatch, &mut RocksWriteBatch) {
        (&mut self.kv_wb, &mut self.raft_wb)
    }

    #[inline]
    fn kv_wb_mut(&mut self) -> &mut RocksWriteBatch {
        &mut self.kv_wb
    }

    #[inline]
    fn raft_wb_mut(&mut self) -> &mut RocksWriteBatch {
        &mut self.raft_wb
    }

    #[inline]
    fn sync_log(&self) -> bool {
        self.sync_log
    }

    #[inline]
    fn set_sync_log(&mut self, sync: bool) {
        self.sync_log = sync;
    }
}

impl PollContext
{
    #[inline]
    // pub fn store_id(&self) -> u64 {
    //     self.store.get_id()
    // }

    pub fn update_ticks_timeout(&mut self) {
        self.tick_batch[PeerTicks::RAFT.bits() as usize].wait_duration =
            Duration::from_millis(200);
            // Duration::from_millis(1000);
        self.tick_batch[PeerTicks::RAFT_LOG_GC.bits() as usize].wait_duration =
            Duration::from_secs(10);
        // self.tick_batch[PeerTicks::ENTRY_CACHE_EVICT.bits() as usize].wait_duration =
        //     ENTRY_CACHE_EVICT_TICK_DURATION;
        // self.tick_batch[PeerTicks::PD_HEARTBEAT.bits() as usize].wait_duration =
        //     self.cfg.pd_heartbeat_tick_interval.0;
        // self.tick_batch[PeerTicks::SPLIT_REGION_CHECK.bits() as usize].wait_duration =
        //     self.cfg.split_region_check_tick_interval.0;
        // self.tick_batch[PeerTicks::CHECK_PEER_STALE_STATE.bits() as usize].wait_duration =
        //     self.cfg.peer_stale_state_check_interval.0;
        // self.tick_batch[PeerTicks::CHECK_MERGE.bits() as usize].wait_duration =
        //     self.cfg.merge_check_tick_interval.0;
    }
}

impl PollContext
{
    // #[inline]
    // fn schedule_store_tick(&self, tick: StoreTick, timeout: Duration) {
    //     if !is_zero_duration(&timeout) {
    //         let mb = self.router.control_mailbox();
    //         let delay = self.timer.delay(timeout).compat().map(move |_| {
    //             if let Err(e) = mb.force_send(StoreMsg::Tick(tick)) {
    //                 info!(
    //                     "failed to schedule store tick, are we shutting down?";
    //                     "tick" => ?tick,
    //                     "err" => ?e
    //                 );
    //             }
    //         });
    //         poll_future_notify(delay);
    //     }
    // }

    pub fn handle_stale_msg(
        &mut self,
        msg: &RaftMessage,
        cur_epoch: RegionEpoch,
        target_region: Option<metapb::Region>,
    ) {
        unreachable!()
    }
}

// pub struct RaftPoller<EK: KvEngine + 'static, ER: RaftEngine + 'static, T: 'static> {
pub struct RaftPoller {
    tag: String,
    // store_msg_buf: Vec<StoreMsg<EK>>,
    peer_msg_buf: Vec<PeerMsg>,
    // previous_metrics: RaftMetrics,
    // timer: TiInstant,
    poll_ctx: PollContext,
    messages_per_tick: usize,
    // cfg_tracker: Tracker<Config>,

    // trace_event: TraceEvent,
}

impl RaftPoller {
    // fn handle_raft_ready(&mut self, peers: &mut [Box<PeerFsm>]) {
    fn handle_raft_ready(&mut self, peer: &mut PeerFsm) {
        // Only enable the fail point when the store id is equal to 3, which is
        // the id of slow store in tests.
        // fail_point!("on_raft_ready", self.poll_ctx.store_id() == 3, |_| {});
        if self.poll_ctx.trans.need_flush()
            // && (!self.poll_ctx.kv_wb.is_empty() || !self.poll_ctx.raft_wb.is_empty())
        {
            self.poll_ctx.trans.flush();
        }
        let ready_cnt = self.poll_ctx.ready_res.len();
        // self.poll_ctx.raft_metrics.ready.has_ready_region += ready_cnt as u64;
        // fail_point!("raft_before_save");
        if !self.poll_ctx.kv_wb.is_empty() {
            // let mut write_opts = WriteOptions::new();
            // write_opts.set_sync(true);
            // self.poll_ctx
            //     .kv_wb
            //     .write_opt(&write_opts)
            //     .unwrap_or_else(|e| {
            //         panic!("{} failed to save append state result: {:?}", self.tag, e);
            //     });
            self.poll_ctx
                .kv_wb
                .write_opt(true);
            // let data_size = self.poll_ctx.kv_wb.data_size();
            // if data_size > KV_WB_SHRINK_SIZE {
            //     self.poll_ctx.kv_wb = self.poll_ctx.engines.kv.write_batch_with_cap(4 * 1024);
            // } else {
                self.poll_ctx.kv_wb.clear();
            // }
        }
        // fail_point!("raft_between_save");

        if !self.poll_ctx.raft_wb.is_empty() {
            // fail_point!(
            //     "raft_before_save_on_store_1",
            //     self.poll_ctx.store_id() == 1,
            //     |_| {}
            // );
            // self.poll_ctx
            //     .engines
            //     .raft
            //     .consume_and_shrink(
            //         &mut self.poll_ctx.raft_wb,
            //         true,
            //         RAFT_WB_SHRINK_SIZE,
            //         4 * 1024,
            //     )
            //     .unwrap_or_else(|e| {
            //         panic!("{} failed to save raft append result: {:?}", self.tag, e);
            //     });
            self.poll_ctx
                .engines
                .raft
                .consume(&mut self.poll_ctx.raft_wb, true);
        }

        // self.poll_ctx.perf_context.report_metrics();
        // fail_point!("raft_after_save");
        if ready_cnt != 0 {
            let mut ready_res = mem::take(&mut self.poll_ctx.ready_res);
            for ready in ready_res.drain(..) {
                // PeerFsmDelegate::new(&mut peers[ready.batch_offset], &mut self.poll_ctx)
                //     .post_raft_ready_append(ready);
                PeerFsmDelegate::new(peer, &mut self.poll_ctx)
                    .post_raft_ready_append(ready);
            }
        }
        // let dur = self.timer.elapsed();
        // if !self.poll_ctx.store_stat.is_busy {
        //     let election_timeout = Duration::from_millis(
        //         self.poll_ctx.cfg.raft_base_tick_interval.as_millis()
        //             * self.poll_ctx.cfg.raft_election_timeout_ticks as u64,
        //     );
        //     if dur >= election_timeout {
        //         self.poll_ctx.store_stat.is_busy = true;
        //     }
        // }

        // self.poll_ctx
        //     .raft_metrics
        //     .append_log
        //     .observe(duration_to_sec(dur) as f64);

        // slow_log!(
        //     dur,
        //     "{} handle {} pending peers include {} ready, {} entries, {} messages and {} \
        //      snapshots",
        //     self.tag,
        //     self.poll_ctx.pending_count,
        //     ready_cnt,
        //     self.poll_ctx.raft_metrics.ready.append - self.previous_metrics.ready.append,
        //     self.poll_ctx.raft_metrics.ready.message - self.previous_metrics.ready.message,
        //     self.poll_ctx.raft_metrics.ready.snapshot - self.previous_metrics.ready.snapshot
        // );
        if self.poll_ctx.trans.need_flush() {
            self.poll_ctx.trans.flush();
        }
    }

    fn flush_ticks(&mut self) {
        for t in PeerTicks::get_all_ticks() {
            let idx = t.bits() as usize;
            if self.poll_ctx.tick_batch[idx].ticks.is_empty() {
                continue;
            }
            let peer_ticks = std::mem::take(&mut self.poll_ctx.tick_batch[idx].ticks);
            let f = self
                .poll_ctx
                .timer
                .delay(self.poll_ctx.tick_batch[idx].wait_duration)
                .compat()
                .map(move |_| {
                    for tick in peer_ticks {
                        tick();
                    }
                });
            poll_future_notify(f);
        }
    }
}

impl RaftPoller
{
    fn begin(&mut self, /*_batch_size: usize*/) {
        // self.previous_metrics = self.poll_ctx.raft_metrics.clone();
        self.poll_ctx.processed_fsm_count = 0;
        self.poll_ctx.pending_count = 0;
        self.poll_ctx.sync_log = false;
        self.poll_ctx.has_ready = false;
        // self.poll_ctx.is_disk_full = disk::is_disk_full();
        // self.timer = TiInstant::now_coarse();
        // update config
        // self.poll_ctx.perf_context.start_observe();
        // if let Some(incoming) = self.cfg_tracker.any_new() {
        //     match Ord::cmp(
        //         &incoming.messages_per_tick,
        //         &self.poll_ctx.cfg.messages_per_tick,
        //     ) {
        //         CmpOrdering::Greater => {
        //             self.store_msg_buf.reserve(incoming.messages_per_tick);
        //             self.peer_msg_buf.reserve(incoming.messages_per_tick);
        //             self.messages_per_tick = incoming.messages_per_tick;
        //         }
        //         CmpOrdering::Less => {
        //             self.store_msg_buf.shrink_to(incoming.messages_per_tick);
        //             self.peer_msg_buf.shrink_to(incoming.messages_per_tick);
        //             self.messages_per_tick = incoming.messages_per_tick;
        //         }
        //         _ => {}
        //     }
        //     self.poll_ctx.cfg = incoming.clone();
        //     self.poll_ctx.update_ticks_timeout();
        // }
        self.peer_msg_buf.reserve(4096);
        self.poll_ctx.update_ticks_timeout();
    }

    // fn handle_control(&mut self, store: &mut StoreFsm<EK>) -> Option<usize> {
    //     unreachable!()
    // }

    fn handle_normal(&mut self, peer: &mut PeerFsm) -> Option<usize> {
        let mut expected_msg_count = None;

        // fail_point!(
        //     "pause_on_peer_collect_message",
        //     peer.peer_id() == 1,
        //     |_| unreachable!()
        // );
        //
        // fail_point!(
        //     "on_peer_collect_message_2",
        //     peer.peer_id() == 2,
        //     |_| unreachable!()
        // );

        while self.peer_msg_buf.len() < self.messages_per_tick {
            match peer.receiver.try_recv() {
                // TODO: we may need a way to optimize the message copy.
                Ok(msg) => {
                    // fail_point!(
                    //     "pause_on_peer_destroy_res",
                    //     peer.peer_id() == 1
                    //         && matches!(
                    //             msg,
                    //             PeerMsg::ApplyRes {
                    //                 res: ApplyTaskRes::Destroy { .. },
                    //             }
                    //         ),
                    //     |_| unreachable!()
                    // );
                    self.peer_msg_buf.push(msg);
                }
                Err(TryRecvError::Empty) => {
                    expected_msg_count = Some(0);
                    break;
                }
                Err(TryRecvError::Disconnected) => {
                    unreachable!()
                    // peer.stop();
                    // expected_msg_count = Some(0);
                    // break;
                }
            }
        }
        let mut delegate = PeerFsmDelegate::new(peer, &mut self.poll_ctx);
        delegate.handle_msgs(&mut self.peer_msg_buf);
        delegate.collect_ready();
        self.poll_ctx.processed_fsm_count += 1;
        expected_msg_count
    }

    fn end(&mut self, peer: &mut PeerFsm) {
        self.flush_ticks();
        if self.poll_ctx.has_ready {
            // self.handle_raft_ready(peers);
            self.handle_raft_ready(peer);
        }
        self.poll_ctx.current_time = None;
        // self.poll_ctx
        //     .raft_metrics
        //     .process_ready
        //     .observe(duration_to_sec(self.timer.elapsed()) as f64);
        // self.poll_ctx.raft_metrics.flush();
        // self.poll_ctx.store_stat.flush();

        // for peer in peers {
        //     peer.update_memory_trace(&mut self.trace_event);
        // }
        // MEMTRACE_PEERS.trace(mem::take(&mut self.trace_event));
    }

    // fn pause(&mut self) {
    //     if self.poll_ctx.trans.need_flush() {
    //         self.poll_ctx.trans.flush();
    //     }
    // }
}

pub struct RaftPollerBuilder {
    // pub cfg: Arc<VersionTrack<Config>>,
    // pub store: metapb::Store,
    pub store_id: u64,
    // pd_scheduler: FutureScheduler<PdTask<EK>>,
    // consistency_check_scheduler: Scheduler<ConsistencyCheckTask<EK::Snapshot>>,
    // split_check_scheduler: Scheduler<SplitCheckTask>,
    // cleanup_scheduler: Scheduler<CleanupTask>,
    raftlog_gc_scheduler: Scheduler<RaftlogGcTask>,
    // pub region_scheduler: Scheduler<RegionTask<EK::Snapshot>>,
    apply_router: ApplyRouter,
    pub router: RaftRouter,
    // pub importer: Arc<SSTImporter>,
    // pub store_meta: Arc<Mutex<StoreMeta>>,
    // pub pending_create_peers: Arc<Mutex<HashMap<u64, (u64, bool)>>>,
    // snap_mgr: SnapManager,
    // pub coprocessor_host: CoprocessorHost<EK>,
    trans: RaftClient,
    // global_stat: GlobalStoreStat,
    pub engines: Engines,
    // global_replication_state: Arc<Mutex<GlobalReplicationState>>,
    // feature_gate: FeatureGate,

    // todo(glengeng) - valid iff under single raft.
    receiver: channel::Receiver<PeerMsg>,
}

// impl<EK: KvEngine, ER: RaftEngine, T> RaftPollerBuilder<EK, ER, T> {
impl RaftPollerBuilder {
    /// Initialize this store. It scans the db engine, loads all regions
    /// and their peers from it, and schedules snapshot worker if necessary.
    /// WARN: This store should not be used before initialized.
    fn init(&mut self) -> Result<Vec<PeerFsm>> {
        // Scan region meta to get saved regions.
        // let start_key = keys::REGION_META_MIN_KEY;
        // let end_key = keys::REGION_META_MAX_KEY;
        let kv_engine = self.engines.kv.clone();
        // let store_id = self.store.get_id();
        // let mut total_count = 0;
        // let mut tombstone_count = 0;
        // let mut applying_count = 0;
        let mut region_peers = vec![];

        let t = Instant::now();
        let mut kv_wb = self.engines.kv.write_batch();
        let mut raft_wb = self.engines.raft.log_batch(4 * 1024);
        // let mut applying_regions = vec![];
        // let mut merging_count = 0;
        // let mut meta = self.store_meta.lock().unwrap();
        // let mut replication_state = self.global_replication_state.lock().unwrap();
        {
        // kv_engine.scan_cf(CF_RAFT, start_key, end_key, false, |key, value| {
        //     let (region_id, suffix) = box_try!(keys::decode_region_meta_key(key));
        //     if suffix != keys::REGION_STATE_SUFFIX {
        //         return Ok(true);
        //     }

            // total_count += 1;

            // let mut local_state = RegionLocalState::default();
            // local_state.merge_from_bytes(value)?;
            // todo(glengeng): under single raft hypothesis, region_id as 1 is a precondition.
            let region_id = 1;
            let local_state: RegionLocalState = self.engines.kv.get_msg_cf(CF_RAFT, &keys::region_state_key(region_id)).unwrap();
            let region = local_state.get_region();

            info!("recover region from store, {:?}", region);

            // let region = local_state.get_region();
            // if local_state.get_state() == PeerState::Tombstone {
            //     tombstone_count += 1;
            //     debug!("region is tombstone"; "region" => ?region, "store_id" => store_id);
            //     self.clear_stale_meta(&mut kv_wb, &mut raft_wb, &local_state);
            //     return Ok(true);
            // }
            // if local_state.get_state() == PeerState::Applying {
            //     // in case of restart happen when we just write region state to Applying,
            //     // but not write raft_local_state to raft rocksdb in time.
            //     box_try!(peer_storage::recover_from_applying_state(
            //         &self.engines,
            //         &mut raft_wb,
            //         region_id
            //     ));
            //     applying_count += 1;
            //     applying_regions.push(region.clone());
            //     return Ok(true);
            // }

            // let (tx, mut peer) = box_try!(PeerFsm::create(
            //     store_id,
            //     &self.cfg.value(),
            //     self.region_scheduler.clone(),
            //     self.engines.clone(),
            //     region,
            // ));
            let mut peer = PeerFsm::create(
                self.store_id,
                // &self.cfg.value(),
                // self.region_scheduler.clone(),
                self.engines.clone(),
                &region,
                self.receiver.clone()
            );

            // peer.peer.init_replication_mode(&mut *replication_state);
            // if local_state.get_state() == PeerState::Merging {
            //     info!("region is merging"; "region" => ?region, "store_id" => store_id);
            //     merging_count += 1;
            //     peer.set_pending_merge_state(local_state.get_merge_state().to_owned());
            // }
            // meta.region_ranges.insert(enc_end_key(region), region_id);
            // meta.regions.insert(region_id, region.clone());
            // meta.region_read_progress
            //     .insert(region_id, peer.peer.read_progress.clone());
            // No need to check duplicated here, because we use region id as the key
            // in DB.
            region_peers.push(peer);
            // self.coprocessor_host.on_region_changed(
            //     region,
            //     RegionChangeEvent::Create,
            //     StateRole::Follower,
            // );
            // Ok(true)
        // })?;
        }

        if !kv_wb.is_empty() {
            kv_wb.write_opt(false);
            // kv_wb.write().unwrap();
            // self.engines.kv.sync_wal().unwrap();
        }
        if !raft_wb.is_empty() {
            // self.engines.raft.consume(&mut raft_wb, true).unwrap();
            self.engines.raft.consume(&mut raft_wb, true);
        }

        // schedule applying snapshot after raft writebatch were written.
        // for region in applying_regions {
        //     info!("region is applying snapshot"; "region" => ?region, "store_id" => store_id);
        //     let (tx, mut peer) = PeerFsm::create(
        //         store_id,
        //         &self.cfg.value(),
        //         self.region_scheduler.clone(),
        //         self.engines.clone(),
        //         &region,
        //     )?;
        //     peer.peer.init_replication_mode(&mut *replication_state);
        //     peer.schedule_applying_snapshot();
        //     meta.region_ranges
        //         .insert(enc_end_key(&region), region.get_id());
        //     meta.region_read_progress
        //         .insert(region.get_id(), peer.peer.read_progress.clone());
        //     meta.regions.insert(region.get_id(), region);
        //     region_peers.push((tx, peer));
        // }

        info!(
            "start store";
            "store_id" => self.store_id,
            // "region_count" => total_count,
            // "tombstone_count" => tombstone_count,
            // "applying_count" =>  applying_count,
            // "merge_count" => merging_count,
            // "takes" => ?t.elapsed(),
        );

        // self.clear_stale_data(&meta)?;

        Ok(region_peers)
    }

    // fn clear_stale_meta(
    //     &self,
    //     kv_wb: &mut EK::WriteBatch,
    //     raft_wb: &mut ER::LogBatch,
    //     origin_state: &RegionLocalState,
    // ) {
    //     let rid = origin_state.get_region().get_id();
    //     let raft_state = match self.engines.raft.get_raft_state(rid).unwrap() {
    //         // it has been cleaned up.
    //         None => return,
    //         Some(value) => value,
    //     };
    //     peer_storage::clear_meta(&self.engines, kv_wb, raft_wb, rid, &raft_state).unwrap();
    //     let key = keys::region_state_key(rid);
    //     kv_wb.put_msg_cf(CF_RAFT, &key, origin_state).unwrap();
    // }

    // /// `clear_stale_data` clean up all possible garbage data.
    // fn clear_stale_data(&self, meta: &StoreMeta) -> Result<()> {
    //     let t = Instant::now();
    //
    //     let mut ranges = Vec::new();
    //     let mut last_start_key = keys::data_key(b"");
    //     for region_id in meta.region_ranges.values() {
    //         let region = &meta.regions[region_id];
    //         let start_key = keys::enc_start_key(region);
    //         ranges.push((last_start_key, start_key));
    //         last_start_key = keys::enc_end_key(region);
    //     }
    //     ranges.push((last_start_key, keys::DATA_MAX_KEY.to_vec()));
    //
    //     self.engines.kv.roughly_cleanup_ranges(&ranges)?;
    //
    //     info!(
    //         "cleans up garbage data";
    //         "store_id" => self.store.get_id(),
    //         "garbage_range_count" => ranges.len(),
    //         "takes" => ?t.elapsed()
    //     );
    //
    //     Ok(())
    // }
}

impl RaftPollerBuilder
{
    fn build(&mut self,) -> RaftPoller {
        let mut ctx = PollContext {
            processed_fsm_count: 0,
            // cfg: self.cfg.value().clone(),
            // store: self.store.clone(),
            // pd_scheduler: self.pd_scheduler.clone(),
            // consistency_check_scheduler: self.consistency_check_scheduler.clone(),
            // split_check_scheduler: self.split_check_scheduler.clone(),
            // region_scheduler: self.region_scheduler.clone(),
            apply_router: self.apply_router.clone(),
            router: self.router.clone(),
            // cleanup_scheduler: self.cleanup_scheduler.clone(),
            raftlog_gc_scheduler: self.raftlog_gc_scheduler.clone(),
            // importer: self.importer.clone(),
            // store_meta: self.store_meta.clone(),
            // pending_create_peers: self.pending_create_peers.clone(),
            // raft_metrics: RaftMetrics::default(),
            // snap_mgr: self.snap_mgr.clone(),
            // coprocessor_host: self.coprocessor_host.clone(),
            timer: SteadyTimer::default(),
            trans: self.trans.clone(),
            // global_replication_state: self.global_replication_state.clone(),
            // global_stat: self.global_stat.clone(),
            // store_stat: self.global_stat.local(),
            engines: self.engines.clone(),
            kv_wb: self.engines.kv.write_batch(),
            raft_wb: self.engines.raft.log_batch(4 * 1024),
            pending_count: 0,
            sync_log: false,
            has_ready: false,
            ready_res: Vec::new(),
            current_time: None,
            // perf_context: self
            //     .engines
            //     .kv
            //     .get_perf_context(self.cfg.value().perf_level, PerfContextKind::RaftstoreStore),
            tick_batch: vec![PeerTickBatch::default(); 256],
            // node_start_time: Some(TiInstant::now_coarse()),
            // feature_gate: self.feature_gate.clone(),
            // is_disk_full: false,
        };
        ctx.update_ticks_timeout();
        // let tag = format!("[store {}]", ctx.store.get_id());
        let tag = format!("[store {}]", self.store_id);
        RaftPoller {
            tag: tag.clone(),
            // store_msg_buf: Vec::with_capacity(ctx.cfg.messages_per_tick),
            // peer_msg_buf: Vec::with_capacity(ctx.cfg.messages_per_tick),
            peer_msg_buf: Vec::with_capacity(4096),
            // previous_metrics: ctx.raft_metrics.clone(),
            // timer: TiInstant::now_coarse(),
            // messages_per_tick: ctx.cfg.messages_per_tick,
            messages_per_tick: 4096,
            poll_ctx: ctx,
            // cfg_tracker: self.cfg.clone().tracker(tag),
            // trace_event: TraceEvent::default(),
        }
    }
}

pub struct RaftSystem {
    name_prefix: Option<String>,
    fsm: Option<PeerFsm>,

    router: RaftRouter,
    receiver: channel::Receiver<PeerMsg>,

    apply_system: ApplySystem,
    // workers: Option<Workers<EK>>,
}

impl RaftSystem {
    pub fn router(&self) -> RaftRouter {
        self.router.clone()
    }

    pub fn apply_router(&self) -> ApplyRouter {
        self.apply_system.router().clone()
    }

    fn start_poller(&mut self, name: String, /*priority: Priority,*/ builder: &mut RaftPollerBuilder)
    {
        let handler = builder.build(/*priority*/);

        let mut poller = Poller {
            // router: self.router.clone(),
            // fsm_receiver: receiver,
            handler,
            fsm: self.fsm.take().unwrap(),
        };

        let t = thread::Builder::new()
            .name(name)
            .spawn(move || {
                poller.poll();
            })
            .unwrap();
    }

    /// Start the batch system.
    pub fn spawn(&mut self, name_prefix: String, mut builder: RaftPollerBuilder)
    {
        self.start_poller(
            thd_name!(format!("{}-{}", name_prefix, 0)),
            &mut builder,
        );

        self.name_prefix = Some(name_prefix);
    }

    /// Shutdown the batch system and wait till all background threads exit.
    pub fn shutdown(&mut self) {
        unreachable!()
    }

    // TODO: reduce arguments
    pub fn spawn_ex(
        &mut self,
        // meta: metapb::Store,
        store_id: u64,
        // cfg: Arc<VersionTrack<Config>>,
        engines: Engines,
        trans: RaftClient,
        // pd_client: Arc<C>,
        // mgr: SnapManager,
        // pd_worker: FutureWorker<PdTask<EK>>,
        // store_meta: Arc<Mutex<StoreMeta>>,
        // mut coprocessor_host: CoprocessorHost<EK>,
        // importer: Arc<SSTImporter>,
        // split_check_scheduler: Scheduler<SplitCheckTask>,
        background_worker: Worker,
        // auto_split_controller: AutoSplitController,
        // global_replication_state: Arc<Mutex<GlobalReplicationState>>,
        // concurrency_manager: ConcurrencyManager,
    ) -> Result<()> {
        // let raftlog_gc_runner = RaftlogGcRunner::new(self.router(), engines.clone());
        let raftlog_gc_runner = RaftlogGcRunner::new(engines.clone());
        let raftlog_gc_scheduler = background_worker
            .start_with_timer("raft-gc-worker", raftlog_gc_runner);

        let mut builder = RaftPollerBuilder {
            // cfg,
            // store: meta,
            store_id,
            engines,
            router: self.router.clone(),
            // split_check_scheduler,
            // region_scheduler,
            // pd_scheduler: workers.pd_worker.scheduler(),
            // consistency_check_scheduler,
            // cleanup_scheduler,
            raftlog_gc_scheduler,
            apply_router: self.apply_router(),
            trans,
            // coprocessor_host,
            // importer,
            // snap_mgr: mgr.clone(),
            // global_replication_state,
            // global_stat: GlobalStoreStat::default(),
            // store_meta,
            // pending_create_peers: Arc::new(Mutex::new(HashMap::default())),
            // feature_gate: pd_client.feature_gate().clone(),
            receiver: self.receiver.clone()
        };
        let region_peers = builder.init()?;
        // let engine = builder.engines.clone();

        self.start_system(
            // workers,
            region_peers,
            builder,
            // auto_split_controller,
            // concurrency_manager,
            // mgr,
            // pd_client,
        )?;

        Ok(())
    }

    fn start_system(
        &mut self,
        // mut workers: Workers<EK>,
        region_peers: Vec<PeerFsm>,
        builder: RaftPollerBuilder,
        // auto_split_controller: AutoSplitController,
        // concurrency_manager: ConcurrencyManager,
        // snap_mgr: SnapManager,
        // pd_client: Arc<C>,
    ) -> Result<()> {
        // let cfg = builder.cfg.value().clone();
        // let store = builder.store.clone();

        let apply_poller_builder = ApplyPollerBuilder::new(
            &builder,
            Box::new(self.router.clone()),
            self.apply_system.router().clone(),
        );
        self.apply_system
            .schedule_all(region_peers.iter().map(|peer_fsm| peer_fsm.get_peer()));

        // let tag = format!("raftstore-{}", builder.store_id);
        // // self.system.spawn(tag, builder);
        // self.spawn(tag, builder);
        // let mut mailboxes = Vec::with_capacity(region_peers.len());
        // let mut address = Vec::with_capacity(region_peers.len());

        assert_eq!(region_peers.len(), 1);
        // for (tx, fsm) in region_peers {
        for fsm in region_peers {
            // address.push(fsm.region_id());
            // mailboxes.push((
            //     fsm.region_id(),
            //     BasicMailbox::new(tx, fsm, self.router.state_cnt().clone()),
            // ));
            self.fsm = Some(fsm);
        }
        // self.router.register_all(mailboxes);

        let tag = format!("raftstore-{}", builder.store_id);
        // self.system.spawn(tag, builder);
        self.spawn(tag, builder);

        // Make sure Msg::Start is the first message each FSM received.
        // for addr in address {
        //     self.router.force_send(addr, PeerMsg::Start).unwrap();
        // }
        self.router.router.send(PeerMsg::Start).unwrap();
        // self.router
        //     .send_control(StoreMsg::Start {
        //         store: store.clone(),
        //     })
        //     .unwrap();

        self.apply_system
            .spawn("apply".to_owned(), apply_poller_builder);

        Ok(())
    }
}

pub fn create_raft_system() -> (RaftRouter, RaftSystem) {
    let (apply_router, apply_system) = create_apply_system();

    let (tx, rx) = channel::unbounded();
    let router = RaftRouter { router: tx };
    let system = RaftSystem {
        name_prefix: None,
        router: router.clone(),
        receiver: rx,
        fsm: None,
        apply_system
    };
    (router, system)
}

/// Internal poller that fetches batch and call handler hooks for readiness.
struct Poller {
    // router: Router<N, C, NormalScheduler<N, C>, ControlScheduler<N, C>>,
    // fsm_receiver: channel::Receiver<FsmTypes<N, C>>,
    handler: RaftPoller,
    // max_batch_size: usize,
    // reschedule_duration: Duration,

    fsm: PeerFsm,
}

impl Poller {
    // Poll for readiness and forward to handler. Remove stale peer if necessary.
    fn poll(&mut self) {
        loop {
            let t = TiInstant::now_coarse();

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
