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

use std::cell::RefCell;
use std::collections::{VecDeque, HashMap};
// use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::{cmp, mem, u64, usize};

use bitflags::bitflags;
// use crossbeam::atomic::AtomicCell;
// use crossbeam::channel::TrySendError;
// use engine_traits::{Engines, KvEngine, RaftEngine, Snapshot, WriteBatch, WriteOptions};
// use error_code::ErrorCodeExt;
// use fail::fail_point;
use kvproto::errorpb;
// use kvproto::kvrpcpb::ExtraOp as TxnExtraOp;
use kvproto::metapb::{self, Region, PeerRole};
// use kvproto::pdpb::PeerStats;
// use kvproto::raft_cmdpb::{
//     self, AdminCmdType, AdminResponse, ChangePeerRequest, CmdType, CommitMergeRequest,
//     RaftCmdRequest, RaftCmdResponse, TransferLeaderRequest, TransferLeaderResponse,
// };
use kvproto::raft_cmdpb::{
    self, AdminCmdType, AdminRequest, AdminResponse, CmdType, RaftCmdRequest, RaftCmdResponse,
    Request, StatusCmdType, StatusResponse
};
use kvproto::raft_serverpb::{RaftApplyState, RaftMessage, RaftTruncatedState};
// use kvproto::replication_modepb::{
//     DrAutoSyncState, RegionReplicationState, RegionReplicationStatus, ReplicationMode,
// };
use protobuf::Message;
use raft::eraftpb::{self, ConfChangeType, Entry, EntryType, MessageType};
use raft::{
    self, Changer, LightReady, ProgressState, ProgressTracker, RawNode, Ready, SnapshotStatus,
    StateRole, INVALID_INDEX, NO_LIMIT,
};
// use raft_proto::ConfChangeI;
// use smallvec::SmallVec;
use time::Timespec;
use uuid::Uuid;

// use crate::coprocessor::{CoprocessorHost, RegionChangeEvent};
// use crate::errors::{RAFTSTORE_IS_BUSY, Error};
// use crate::errors::Error;
// use crate::store::fsm::apply::CatchUpLogs;
// use crate::store::fsm::store::PollContext;
// use crate::store::fsm::{apply, Apply, ApplyMetrics, ApplyTask, CollectedReady, Proposal};
// use crate::store::hibernate_state::GroupState;
// use crate::store::memory::{needs_evict_entry_cache, MEMTRACE_RAFT_ENTRIES};
// use crate::store::msg::RaftCommand;
// use crate::store::util::{admin_cmd_epoch_lookup, RegionReadProgress};
// use crate::store::worker::{
//     HeartbeatTask, QueryStats, ReadDelegate, ReadExecutor, ReadProgress, RegionTask,
// };
// use crate::store::{Callback, Config, GlobalReplicationState, PdTask, ReadIndexContext, ReadResponse, PollContext};
use crate::store::{PollContext};
use crate::errors::{Error, Result};
// use collections::{HashMap, HashSet};
// use pd_client::INVALID_ID;
// use tikv_alloc::trace::TraceEvent;
// use tikv_util::codec::number::decode_u64;
// use tikv_util::sys::disk;
// use tikv_util::time::{duration_to_sec, monotonic_raw_now};
use tikv_util::time::monotonic_raw_now;
use tikv_util::time::{Instant as UtilInstant, ThreadReadId};
// use tikv_util::worker::{FutureScheduler, Scheduler};
use tikv_util::Either;
use tikv_util::{box_err, debug, error, info, warn, trace};
// use txn_types::WriteBatchFlags;

use super::cmd_resp;
// use super::local_metrics::{RaftReadyMetrics, RaftSendMessageMetrics};
// use super::metrics::*;
use crate::peer_storage::{InvokeContext, PeerStorage,};
// use super::read_queue::{ReadIndexQueue, ReadIndexRequest};
// use super::transport::Transport;
// use super::util::{
//     self, check_region_epoch, is_initial_msg, AdminCmdEpochState, ChangePeerI, ConfChangeKind,
//     Lease, LeaseState, NORMAL_REQ_CHECK_CONF_VER, NORMAL_REQ_CHECK_VER,
// };
// use super::DestroyPeerJob;
use crate::apply::{self, Proposal, Apply, ExecResult, Msg as ApplyTask, TaskRes as ApplyTaskRes};
use crate::msg::{Callback, PeerTicks, PeerMsg, RaftCommand, ExtCallback, InspectedRaftMessage};
use crossbeam::channel;
use crate::rocks_engine::Engines;
use crate::raft_client::RaftClient;
use crate::util;
use crate::cmd_resp::{new_error, bind_term};
use crate::metrics::IS_LEADER_GAUGE;
use crate::raftlog_gc::Task as RaftlogGcTask;

const SHRINK_CACHE_CAPACITY: usize = 64;
const MIN_BCAST_WAKE_UP_INTERVAL: u64 = 1_000; // 1s
const REGION_READ_PROGRESS_CAP: usize = 128;
const MAX_COMMITTED_SIZE_PER_READY: u64 = 16 * 1024 * 1024;

// /// The returned states of the peer after checking whether it is stale
// #[derive(Debug, PartialEq, Eq)]
// pub enum StaleState {
//     Valid,
//     ToValidate,
//     LeaderMissing,
// }

struct ProposalQueue
{
    tag: String,
    queue: VecDeque<Proposal>,
}

impl ProposalQueue {
    fn new(tag: String) -> ProposalQueue {
        ProposalQueue {
            tag,
            queue: VecDeque::new(),
        }
    }

    fn find_propose_time(&self, term: u64, index: u64) -> Option<Timespec> {
        self.queue
            .binary_search_by_key(&(term, index), |p: &Proposal| (p.term, p.index))
            .ok()
            .map(|i| self.queue[i].propose_time)
            .flatten()
    }

    // Find proposal in front or at the given term and index
    fn pop(&mut self, term: u64, index: u64) -> Option<Proposal> {
        self.queue.pop_front().and_then(|p| {
            // Comparing the term first then the index, because the term is
            // increasing among all log entries and the index is increasing
            // inside a given term
            if (p.term, p.index) > (term, index) {
                self.queue.push_front(p);
                return None;
            }
            Some(p)
        })
    }

    /// Find proposal at the given term and index and notify stale proposals
    /// in front that term and index
    fn find_proposal(&mut self, term: u64, index: u64, current_term: u64) -> Option<Proposal> {
        while let Some(p) = self.pop(term, index) {
            if p.term == term {
                if p.index == index {
                    return if p.cb.is_none() { None } else { Some(p) };
                } else {
                    panic!(
                        "{} unexpected callback at term {}, found index {}, expected {}",
                        self.tag, term, p.index, index
                    );
                }
            } else {
                apply::notify_stale_req(current_term, p.cb);
            }
        }
        None
    }

    fn push(&mut self, p: Proposal) {
        if let Some(f) = self.queue.back() {
            // The term must be increasing among all log entries and the index
            // must be increasing inside a given term
            assert!((p.term, p.index) > (f.term, f.index));
        }
        self.queue.push_back(p);
    }

    fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    fn gc(&mut self) {
        if self.queue.capacity() > SHRINK_CACHE_CAPACITY && self.queue.len() < SHRINK_CACHE_CAPACITY
        {
            self.queue.shrink_to_fit();
        }
    }
}

// bitflags! {
//     // TODO: maybe declare it as protobuf struct is better.
//     /// A bitmap contains some useful flags when dealing with `eraftpb::Entry`.
//     pub struct ProposalContext: u8 {
//         const SYNC_LOG       = 0b0000_0001;
//         const SPLIT          = 0b0000_0010;
//         const PREPARE_MERGE  = 0b0000_0100;
//     }
// }

//
// /// `ConsistencyState` is used for consistency check.
// pub struct ConsistencyState {
//     pub last_check_time: Instant,
//     // (computed_result_or_to_be_verified, index, hash)
//     pub index: u64,
//     pub context: Vec<u8>,
//     pub hash: Vec<u8>,
// }

// /// Statistics about raft peer.
// #[derive(Default, Clone)]
// pub struct PeerStat {
//     pub written_bytes: u64,
//     pub written_keys: u64,
//     pub written_query_stats: QueryStats,
// }

// #[derive(Default, Debug, Clone, Copy)]
// pub struct CheckTickResult {
//     leader: bool,
//     up_to_date: bool,
//     reason: &'static str,
// }

// pub struct ProposedAdminCmd<S: Snapshot> {
//     cmd_type: AdminCmdType,
//     epoch_state: AdminCmdEpochState,
//     index: u64,
//     cbs: Vec<Callback<S>>,
// }

// struct CmdEpochChecker<S: Snapshot> {
//     // Although it's a deque, because of the characteristics of the settings from `admin_cmd_epoch_lookup`,
//     // the max size of admin cmd is 2, i.e. split/merge and change peer.
//     proposed_admin_cmd: VecDeque<ProposedAdminCmd<S>>,
//     term: u64,
// }

pub struct Peer
{
    /// The ID of the Region which this Peer belongs to.
    region_id: u64,
    // TODO: remove it once panic!() support slog fields.
    /// Peer_tag, "[region <region_id>] <peer_id>"
    pub tag: String,
    /// The Peer meta information.
    pub peer: metapb::Peer,

    /// The Raft state machine of this Peer.
    pub raft_group: RawNode<PeerStorage>,
    /// The cache of meta information for Region's other Peers.
    peer_cache: RefCell<HashMap<u64, metapb::Peer>>,
    /// Record the last instant of each peer's heartbeat response.
    pub peer_heartbeats: HashMap<u64, Instant>,

    proposals: ProposalQueue,
    leader_missing_time: Option<Instant>,
    // leader_lease: Lease,
    // pending_reads: ReadIndexQueue<EK::Snapshot>,

    /// If it fails to send messages to leader.
    pub leader_unreachable: bool,
    // /// Indicates whether the peer should be woken up.
    // pub should_wake_up: bool,
    // /// Whether this peer is destroyed asynchronously.
    // /// If it's true,
    // /// 1. when merging, its data in storeMeta will be removed early by the target peer.
    // /// 2. all read requests must be rejected.
    // pub pending_remove: bool,
    /// If a snapshot is being applied asynchronously, messages should not be sent.
    pending_messages: Vec<eraftpb::Message>,

    // /// Record the instants of peers being added into the configuration.
    // /// Remove them after they are not pending any more.
    // pub peers_start_pending_time: Vec<(u64, Instant)>,
    // /// A inaccurate cache about which peer is marked as down.
    // down_peer_ids: Vec<u64>,

    // /// An inaccurate difference in region size since last reset.
    // /// It is used to decide whether split check is needed.
    // pub size_diff_hint: u64,
    // /// The count of deleted keys since last reset.
    // delete_keys_hint: u64,
    // /// An inaccurate difference in region size after compaction.
    // /// It is used to trigger check split to update approximate size and keys after space reclamation
    // /// of deleted entries.
    // pub compaction_declined_bytes: u64,
    // /// Approximate size of the region.
    // pub approximate_size: u64,
    // /// Approximate keys of the region.
    // pub approximate_keys: u64,
    // /// Whether this region has calculated region size by split-check thread. If we just splitted
    // ///  the region or ingested one file which may be overlapped with the existed data, the
    // /// `approximate_size` is not very accurate.
    // pub has_calculated_region_size: bool,

    // /// The state for consistency check.
    // pub consistency_state: ConsistencyState,

    // /// The counter records pending snapshot requests.
    // pub pending_request_snapshot_count: Arc<AtomicUsize>,
    /// The index of last scheduled committed raft log.
    pub last_applying_idx: u64,
    /// The index of last compacted raft log. It is used for the next compact log task.
    pub last_compacted_idx: u64,
    /// The index of the latest urgent proposal index.
    last_urgent_proposal_idx: u64,
    // /// The index of the latest committed split command.
    // last_committed_split_idx: u64,
    // /// Approximate size of logs that is applied but not compacted yet.
    // pub raft_log_size_hint: u64,

    // /// The index of the latest proposed prepare merge command.
    // last_proposed_prepare_merge_idx: u64,
    // /// The index of the latest committed prepare merge command.
    // last_committed_prepare_merge_idx: u64,
    // /// The merge related state. It indicates this Peer is in merging.
    // pub pending_merge_state: Option<MergeState>,
    // /// The rollback merge proposal can be proposed only when the number
    // /// of peers is greater than the majority of all peers.
    // /// There are more details in the annotation above
    // /// `test_node_merge_write_data_to_source_region_after_merging`
    // /// The peers who want to rollback merge.
    // pub want_rollback_merge_peers: HashSet<u64>,
    // /// Source region is catching up logs for merge.
    // pub catch_up_logs: Option<CatchUpLogs>,

    // /// Write Statistics for PD to schedule hot spot.
    // pub peer_stat: PeerStat,

    // /// Time of the last attempt to wake up inactive leader.
    // pub bcast_wake_up_time: Option<UtilInstant>,
    // /// Current replication mode version.
    // pub replication_mode_version: u64,
    // /// The required replication state at current version.
    // pub dr_auto_sync_state: DrAutoSyncState,
    // /// A flag that caches sync state. It's set to true when required replication
    // /// state is reached for current region.
    // pub replication_sync: bool,

    // /// The known newest conf version and its corresponding peer list
    // /// Send to these peers to check whether itself is stale.
    // pub check_stale_conf_ver: u64,
    // pub check_stale_peers: Vec<metapb::Peer>,
    // /// Whether this peer is created by replication and is the first
    // /// one of this region on local store.
    // pub local_first_replicate: bool,

    // pub txn_extra_op: Arc<AtomicCell<TxnExtraOp>>,

    // /// The max timestamp recorded in the concurrency manager is only updated at leader.
    // /// So if a peer becomes leader from a follower, the max timestamp can be outdated.
    // /// We need to update the max timestamp with a latest timestamp from PD before this
    // /// peer can work.
    // /// From the least significant to the most, 1 bit marks whether the timestamp is
    // /// updated, 31 bits for the current epoch version, 32 bits for the current term.
    // /// The version and term are stored to prevent stale UpdateMaxTimestamp task from
    // /// marking the lowest bit.
    // pub max_ts_sync_status: Arc<AtomicU64>,

    // /// Check whether this proposal can be proposed based on its epoch.
    // cmd_epoch_checker: CmdEpochChecker<EK::Snapshot>,

    /// The number of the last unpersisted ready.
    last_unpersisted_number: u64,

    // pub read_progress: Arc<RegionReadProgress>,

    // pub memtrace_raft_entries: usize,
}

impl Peer
{
    pub fn new(
        store_id: u64,
        // cfg: &Config,
        // sched: Scheduler<RegionTask<EK::Snapshot>>,
        // engines: Engines<EK, ER>,
        engines: Engines,
        region: &metapb::Region,
        peer: metapb::Peer,
    ) -> Result<Peer> {
        if peer.get_id() == raft::INVALID_ID {
            return Err(box_err!("invalid peer id"));
        }

        let tag = format!("[region {}] {}", region.get_id(), peer.get_id());

        // let ps = PeerStorage::new(engines, region, sched, peer.get_id(), tag.clone())?;
        let ps = PeerStorage::new(engines, region,  peer.get_id(), tag.clone());

        let applied_index = ps.applied_index();

        let raft_cfg = raft::Config {
            id: peer.get_id(),
            election_tick: 10,
            heartbeat_tick: 2,
            min_election_tick: 0,
            max_election_tick: 0,
            max_size_per_msg: 4 * 1024 * 1024,
            max_inflight_msgs: 8,
            applied: applied_index,
            check_quorum: true,
            skip_bcast_commit: true,
            pre_vote: true,
            max_committed_size_per_ready: MAX_COMMITTED_SIZE_PER_READY,
            ..Default::default()
        };

        let logger = slog_global::get_global().new(slog::o!("region_id" => region.get_id()));
        let raft_group = RawNode::new(&raft_cfg, ps, &logger)?;
        let mut peer = Peer {
            peer,
            region_id: region.get_id(),
            raft_group,
            proposals: ProposalQueue::new(tag.clone()),
            // pending_reads: Default::default(),
            peer_cache: RefCell::new(HashMap::default()),
            peer_heartbeats: HashMap::default(),
            // peers_start_pending_time: vec![],
            // down_peer_ids: vec![],
            // size_diff_hint: 0,
            // delete_keys_hint: 0,
            // approximate_size: 0,
            // approximate_keys: 0,
            // has_calculated_region_size: false,
            // compaction_declined_bytes: 0,
            leader_unreachable: false,
            // pending_remove: false,
            // should_wake_up: false,
            // pending_merge_state: None,
            // want_rollback_merge_peers: HashSet::default(),
            // pending_request_snapshot_count: Arc::new(AtomicUsize::new(0)),
            // last_proposed_prepare_merge_idx: 0,
            // last_committed_prepare_merge_idx: 0,
            leader_missing_time: Some(Instant::now()),
            tag: tag.clone(),
            last_applying_idx: applied_index,
            last_compacted_idx: 0,
            last_urgent_proposal_idx: u64::MAX,
            // last_committed_split_idx: 0,
            // consistency_state: ConsistencyState {
            //     last_check_time: Instant::now(),
            //     index: INVALID_INDEX,
            //     context: vec![],
            //     hash: vec![],
            // },
            // raft_log_size_hint: 0,
            // leader_lease: Lease::new(cfg.raft_store_max_leader_lease()),
            pending_messages: vec![],
            // peer_stat: PeerStat::default(),
            // catch_up_logs: None,
            // bcast_wake_up_time: None,
            // replication_mode_version: 0,
            // dr_auto_sync_state: DrAutoSyncState::Async,
            // replication_sync: false,
            // check_stale_conf_ver: 0,
            // check_stale_peers: vec![],
            // local_first_replicate: false,
            // txn_extra_op: Arc::new(AtomicCell::new(TxnExtraOp::Noop)),
            // max_ts_sync_status: Arc::new(AtomicU64::new(0)),
            // cmd_epoch_checker: Default::default(),
            last_unpersisted_number: 0,
            // read_progress: Arc::new(RegionReadProgress::new(
            //     region,
            //     applied_index,
            //     REGION_READ_PROGRESS_CAP,
            //     tag,
            // )),
            // memtrace_raft_entries: 0,
        };

        // If this region has only one peer and I am the one, campaign directly.
        if region.get_peers().len() == 1 && region.get_peers()[0].get_store_id() == store_id {
            peer.raft_group.campaign()?;
        }

        Ok(peer)
    }

    // /// Sets commit group to the peer.
    // pub fn init_replication_mode(&mut self, state: &mut GlobalReplicationState) {
    //     unreachable!()
    // }

    // /// Updates replication mode.
    // pub fn switch_replication_mode(&mut self, state: &Mutex<GlobalReplicationState>) {
    //     unreachable!()
    // }

    /// Register self to apply_scheduler so that the peer is then usable.
    /// Also trigger `RegionChangeEvent::Create` here.
    pub fn activate<T>(&self, ctx: &PollContext) {
        unreachable!()
    }

    #[inline]
    fn next_proposal_index(&self) -> u64 {
        self.raft_group.raft.raft_log.last_index() + 1
    }

    #[inline]
    pub fn get_index_term(&self, idx: u64) -> u64 {
        match self.raft_group.raft.raft_log.term(idx) {
            Ok(t) => t,
            Err(e) => panic!("{} fail to load term for {}: {:?}", self.tag, idx, e),
        }
    }

    // pub fn maybe_append_merge_entries(&mut self, merge: &CommitMergeRequest) -> Option<u64> {
    //     unreachable!()
    // }

    // /// Tries to destroy itself. Returns a job (if needed) to do more cleaning tasks.
    // pub fn maybe_destroy<T>(&mut self, ctx: &PollContext) -> Option<DestroyPeerJob> {
    //     unreachable!()
    // }

    /// Does the real destroy task which includes:
    /// 1. Set the region to tombstone;
    /// 2. Clear data;
    /// 3. Notify all pending requests.
    pub fn destroy<T>(&mut self, ctx: &PollContext, keep_data: bool) -> Result<()> {
        unreachable!()
    }

    #[inline]
    pub fn is_initialized(&self) -> bool {
        self.get_store().is_initialized()
    }

    #[inline]
    pub fn region(&self) -> &metapb::Region {
        self.get_store().region()
    }

    /// Check whether the peer can be hibernated.
    ///
    /// This should be used with `check_after_tick` to get a correct conclusion.
    // pub fn check_before_tick(&self, cfg: &Config) -> () {
    pub fn check_before_tick(&self) -> () {
        unreachable!()
    }

    // pub fn check_after_tick(&self, state: GroupState, res: CheckTickResult) -> bool {
    pub fn check_after_tick(&self) -> bool {
        unreachable!()
    }

    #[inline]
    pub fn has_valid_leader(&self) -> bool {
        if self.raft_group.raft.leader_id == raft::INVALID_ID {
            return false;
        }
        for p in self.region().get_peers() {
            if p.get_id() == self.raft_group.raft.leader_id && p.get_role() != PeerRole::Learner {
                return true;
            }
        }
        false
    }

    /// Pings if followers are still connected.
    ///
    /// Leader needs to know exact progress of followers, and
    /// followers just need to know whether leader is still alive.
    pub fn ping(&mut self) {
        if self.is_leader() {
            self.raft_group.ping();
        }
    }

    pub fn has_uncommitted_log(&self) -> bool {
        self.raft_group.raft.raft_log.committed < self.raft_group.raft.raft_log.last_index()
    }

    /// Set the region of a peer.
    ///
    /// This will update the region of the peer, caller must ensure the region
    /// has been preserved in a durable device.
    pub fn set_region(
        &mut self,
        // host: &CoprocessorHost<impl KvEngine>,
        // reader: &mut ReadDelegate,
        region: metapb::Region,
    ) {
        // if self.region().get_region_epoch().get_version() < region.get_region_epoch().get_version()
        // {
        //     // Epoch version changed, disable read on the localreader for this region.
        //     self.leader_lease.expire_remote_lease();
        // }
        self.mut_store().set_region(region.clone());
        // let progress = ReadProgress::region(region);
        // Always update read delegate's region to avoid stale region info after a follower
        // becoming a leader.
        // self.maybe_update_read_progress(reader, progress);

        // // Update leader info
        // self.read_progress
        //     .update_leader_info(self.leader_id(), self.term(), self.region());

        // if !self.pending_remove {
        //     host.on_region_changed(self.region(), RegionChangeEvent::Update, self.get_role());
        // }
    }

    #[inline]
    pub fn peer_id(&self) -> u64 {
        self.peer.get_id()
    }

    #[inline]
    pub fn leader_id(&self) -> u64 {
        self.raft_group.raft.leader_id
    }

    #[inline]
    pub fn is_leader(&self) -> bool {
        self.raft_group.raft.state == StateRole::Leader
    }

    #[inline]
    pub fn get_role(&self) -> StateRole {
        self.raft_group.raft.state
    }

    #[inline]
    pub fn get_store(&self) -> &PeerStorage {
        self.raft_group.store()
    }

    #[inline]
    pub fn mut_store(&mut self) -> &mut PeerStorage {
        self.raft_group.mut_store()
    }

    #[inline]
    pub fn is_applying_snapshot(&self) -> bool {
        false
        // self.get_store().is_applying_snapshot()
    }

    /// Returns `true` if the raft group has replicated a snapshot but not committed it yet.
    #[inline]
    pub fn has_pending_snapshot(&self) -> bool {
        false
        // self.get_pending_snapshot().is_some()
    }

    #[inline]
    pub fn get_pending_snapshot(&self) -> Option<&eraftpb::Snapshot> {
        self.raft_group.snap()
    }

    fn add_ready_metric(&self, ready: &Ready) {
        unreachable!()
    }

    fn add_light_ready_metric(&self, light_ready: &LightReady) {
        unreachable!()
    }

    #[inline]
    pub fn in_joint_state(&self) -> bool {
        self.region().get_peers().iter().any(|p| {
            p.get_role() == PeerRole::IncomingVoter || p.get_role() == PeerRole::DemotingVoter
        })
    }

    #[inline]
    // fn send<T, I>(&mut self, trans: &mut T, msgs: I)
    //     where
    //         T: Transport,
    //         I: IntoIterator<Item = eraftpb::Message>,
    fn send<I>(&mut self, trans: &mut RaftClient, msgs: I)
        where
            I: IntoIterator<Item = eraftpb::Message>,
    {
        for msg in msgs {
            let msg_type = msg.get_msg_type();
            let snapshot_index = msg.get_request_snapshot();
            let i = self.send_raft_message(msg, trans) as usize;
            // match msg_type {
            //     MessageType::MsgAppend => metrics.append[i] += 1,
            //     MessageType::MsgAppendResponse => {
            //         if snapshot_index != raft::INVALID_INDEX {
            //             metrics.request_snapshot[i] += 1;
            //         }
            //         metrics.append_resp[i] += 1;
            //     }
            //     MessageType::MsgRequestPreVote => metrics.prevote[i] += 1,
            //     MessageType::MsgRequestPreVoteResponse => metrics.prevote_resp[i] += 1,
            //     MessageType::MsgRequestVote => metrics.vote[i] += 1,
            //     MessageType::MsgRequestVoteResponse => metrics.vote_resp[i] += 1,
            //     MessageType::MsgSnapshot => metrics.snapshot[i] += 1,
            //     MessageType::MsgHeartbeat => metrics.heartbeat[i] += 1,
            //     MessageType::MsgHeartbeatResponse => metrics.heartbeat_resp[i] += 1,
            //     MessageType::MsgTransferLeader => metrics.transfer_leader[i] += 1,
            //     MessageType::MsgReadIndex => metrics.read_index[i] += 1,
            //     MessageType::MsgReadIndexResp => metrics.read_index_resp[i] += 1,
            //     MessageType::MsgTimeoutNow => {
            //         // After a leader transfer procedure is triggered, the lease for
            //         // the old leader may be expired earlier than usual, since a new leader
            //         // may be elected and the old leader doesn't step down due to
            //         // network partition from the new leader.
            //         // For lease safety during leader transfer, transit `leader_lease`
            //         // to suspect.
            //         self.leader_lease.suspect(monotonic_raw_now());
            //
            //         metrics.timeout_now[i] += 1;
            //     }
            //     // We do not care about these message types for metrics.
            //     // Explicitly declare them so when we add new message types we are forced to
            //     // decide.
            //     MessageType::MsgHup
            //     | MessageType::MsgBeat
            //     | MessageType::MsgPropose
            //     | MessageType::MsgUnreachable
            //     | MessageType::MsgSnapStatus
            //     | MessageType::MsgCheckQuorum => {}
            // }
        }
    }

    /// Steps the raft message.
    pub fn step(
        &mut self,
        ctx: &mut PollContext,
        mut m: eraftpb::Message,
    ) -> Result<()> {
        // fail_point!(
        //     "step_message_3_1",
        //     self.peer.get_store_id() == 3 && self.region_id == 1,
        //     |_| Ok(())
        // );
        // if self.is_leader() && m.get_from() != INVALID_ID {
        if self.is_leader() && m.get_from() != 0 {
            self.peer_heartbeats.insert(m.get_from(), Instant::now());
            // As the leader we know we are not missing.
            self.leader_missing_time.take();
        } else if m.get_from() == self.leader_id() {
            // As another role know we're not missing.
            self.leader_missing_time.take();
        }
        let msg_type = m.get_msg_type();
        if msg_type == MessageType::MsgReadIndex {
            unreachable!()
            // fail_point!("on_step_read_index_msg");
            // ctx.coprocessor_host.on_step_read_index(&mut m);
            // // Must use the commit index of `PeerStorage` instead of the commit index
            // // in raft-rs which may be greater than the former one.
            // // For more details, see the annotations above `on_leader_commit_idx_changed`.
            // let index = self.get_store().commit_index();
            // // Check if the log term of this index is equal to current term, if so,
            // // this index can be used to reply the read index request if the leader holds
            // // the lease. Please also take a look at raft-rs.
            // if self.get_store().term(index).unwrap() == self.term() {
            //     let state = self.inspect_lease();
            //     if let LeaseState::Valid = state {
            //         // If current peer has valid lease, then we could handle the
            //         // request directly, rather than send a heartbeat to check quorum.
            //         let mut resp = eraftpb::Message::default();
            //         resp.set_msg_type(MessageType::MsgReadIndexResp);
            //         resp.term = self.term();
            //         resp.to = m.from;
            //
            //         resp.index = index;
            //         resp.set_entries(m.take_entries());
            //
            //         self.raft_group.raft.msgs.push(resp);
            //         return Ok(());
            //     }
            //     self.should_wake_up = state == LeaseState::Expired;
            // }
        } else if msg_type == MessageType::MsgTransferLeader {
            unreachable!()
            // self.execute_transfer_leader(ctx, &m);
            // return Ok(());
        }

        let from_id = m.get_from();
        // let has_snap_task = self.get_store().has_gen_snap_task();
        self.raft_group.step(m)?;

        // let mut for_balance = false;
        // if !has_snap_task && self.get_store().has_gen_snap_task() {
        //     if let Some(progress) = self.raft_group.status().progress {
        //         if let Some(pr) = progress.get(from_id) {
        //             // When a peer is uninitialized (e.g. created by load balance),
        //             // the last index of the peer is 0 which makes the matched index to be 0.
        //             if pr.matched == 0 {
        //                 for_balance = true;
        //             }
        //         }
        //     }
        // }
        // if for_balance {
        //     if let Some(gen_task) = self.mut_store().mut_gen_snap_task() {
        //         gen_task.set_for_balance();
        //     }
        // }
        Ok(())
    }

    /// Checks and updates `peer_heartbeats` for the peer.
    pub fn check_peers(&mut self) {
        unreachable!()
    }

    /// Collects all down peers.
    // pub fn collect_down_peers(&mut self, max_duration: Duration) -> Vec<PeerStats> {
    pub fn collect_down_peers(&mut self, max_duration: Duration) {
        unreachable!()
    }

    /// Collects all pending peers and update `peers_start_pending_time`.
    pub fn collect_pending_peers<T>(&mut self, ctx: &PollContext) -> Vec<metapb::Peer> {
        unreachable!()
    }

    /// Returns `true` if any peer recover from connectivity problem.
    ///
    /// A peer can become pending or down if it has not responded for a
    /// long time. If it becomes normal again, PD need to be notified.
    pub fn any_new_peer_catch_up(&mut self, peer_id: u64) -> bool {
        unreachable!()
    }

    // pub fn check_stale_state(&mut self, ctx: &mut PollContext) -> StaleState {
    pub fn check_stale_state(&mut self, ctx: &mut PollContext) {
        unreachable!()
    }

    fn on_role_changed(&mut self, ctx: &mut PollContext, ready: &Ready) {
        // Update leader lease when the Raft state changes.
        if let Some(ss) = ready.ss() {
            match ss.raft_state {
                StateRole::Leader => {
                    // // The local read can only be performed after a new leader has applied
                    // // the first empty entry on its term. After that the lease expiring time
                    // // should be updated to
                    // //   send_to_quorum_ts + max_lease
                    // // as the comments in `Lease` explain.
                    // // It is recommended to update the lease expiring time right after
                    // // this peer becomes leader because it's more convenient to do it here and
                    // // it has no impact on the correctness.
                    // let progress_term = ReadProgress::term(self.term());
                    // self.maybe_renew_leader_lease(monotonic_raw_now(), ctx, Some(progress_term));
                    info!(
                        "becomes leader with lease";
                        "region_id" => self.region_id,
                        "peer_id" => self.peer.get_id(),
                        // "lease" => ?self.leader_lease,
                    );
                    IS_LEADER_GAUGE.set(1);
                    // If the predecessor reads index during transferring leader and receives
                    // quorum's heartbeat response after that, it may wait for applying to
                    // current term to apply the read. So broadcast eagerly to avoid unexpected
                    // latency.
                    //
                    // TODO: Maybe the predecessor should just drop all the read requests directly?
                    // All the requests need to be redirected in the end anyway and executing
                    // prewrites or commits will be just a waste.
                    self.last_urgent_proposal_idx = self.raft_group.raft.raft_log.last_index();
                    self.raft_group.skip_bcast_commit(false);

                    // // A more recent read may happen on the old leader. So max ts should
                    // // be updated after a peer becomes leader.
                    // self.require_updating_max_ts(&ctx.pd_scheduler);
                }
                StateRole::Follower => {
                    // self.leader_lease.expire();
                    // self.mut_store().cancel_generating_snap(None);
                    IS_LEADER_GAUGE.set(0);
                }
                _ => {}
            }
            self.on_leader_changed(ctx, ss.leader_id, self.term());
            // // TODO: it may possible that only the `leader_id` change and the role
            // // didn't change
            // ctx.coprocessor_host
            //     .on_role_change(self.region(), ss.raft_state);
            // self.cmd_epoch_checker.maybe_update_term(self.term());
        } else if ready.must_sync() {
            match ready.hs() {
                Some(hs) if hs.get_term() != self.get_store().hard_state().get_term() => {
                    self.on_leader_changed(ctx, self.leader_id(), hs.get_term());
                }
                _ => (),
            }
        }
    }

    /// Correctness depends on the order between calling this function and notifying other peers
    /// the new commit index.
    /// It is due to the interaction between lease and split/merge.(details are decribed below)
    ///
    /// Note that in addition to the hearbeat/append msg, the read index response also can notify
    /// other peers the new commit index. There are three place where TiKV handles read index resquest.
    /// The first place is in raft-rs, so it's like hearbeat/append msg, call this function and
    /// then send the response. The second place is in `Step`, we should use the commit index
    /// of `PeerStorage` which is the greatest commit index that can be observed outside.
    /// The third place is in `read_index`, handle it like the second one.
    fn on_leader_commit_idx_changed(&mut self, pre_commit_index: u64, commit_index: u64) {
        if commit_index <= pre_commit_index || !self.is_leader() {
            return;
        }

        // // The admin cmds in `CmdEpochChecker` are proposed by the current leader so we can
        // // use it to get the split/prepare-merge cmds which was committed just now.
        //
        // // BatchSplit and Split cmd are mutually exclusive because they both change epoch's
        // // version so only one of them can be proposed and the other one will be rejected
        // // by `CmdEpochChecker`.
        // let last_split_idx = self
        //     .cmd_epoch_checker
        //     .last_cmd_index(AdminCmdType::BatchSplit)
        //     .or_else(|| self.cmd_epoch_checker.last_cmd_index(AdminCmdType::Split));
        // if let Some(idx) = last_split_idx {
        //     if idx > pre_commit_index && idx <= commit_index {
        //         // We don't need to suspect its lease because peers of new region that
        //         // in other store do not start election before theirs election timeout
        //         // which is longer than the max leader lease.
        //         // It's safe to read local within its current lease, however, it's not
        //         // safe to renew its lease.
        //         self.last_committed_split_idx = idx;
        //     }
        // } else {
        //     // BatchSplit/Split and PrepareMerge cmd are mutually exclusive too.
        //     // So if there is no Split cmd, we should check PrepareMerge cmd.
        //     let last_prepare_merge_idx = self
        //         .cmd_epoch_checker
        //         .last_cmd_index(AdminCmdType::PrepareMerge);
        //     if let Some(idx) = last_prepare_merge_idx {
        //         if idx > pre_commit_index && idx <= commit_index {
        //             // We committed prepare merge, to prevent unsafe read index,
        //             // we must record its index.
        //             self.last_committed_prepare_merge_idx = idx;
        //             // After prepare_merge is committed and the leader broadcasts commit
        //             // index to followers, the leader can not know when the target region
        //             // merges majority of this region, also it can not know when the target
        //             // region writes new values.
        //             // To prevent unsafe local read, we suspect its leader lease.
        //             self.leader_lease.suspect(monotonic_raw_now());
        //             // Stop updating `safe_ts`
        //             self.read_progress.discard();
        //         }
        //     }
        // }
    }

    fn on_leader_changed(
        &mut self,
        ctx: &mut PollContext,
        leader_id: u64,
        term: u64,
    ) {
        debug!(
            "insert leader info to meta";
            "region_id" => self.region_id,
            "leader_id" => leader_id,
            "term" => term,
            "peer_id" => self.peer_id(),
        );

        // self.read_progress
        //     .update_leader_info(leader_id, term, self.region());

        // let mut meta = ctx.store_meta.lock().unwrap();
        // meta.leaders.insert(self.region_id, (term, leader_id));
    }

    #[inline]
    pub fn ready_to_handle_pending_snap(&self) -> bool {
        unreachable!()
    }

    #[inline]
    fn ready_to_handle_read(&self) -> bool {
        unreachable!()
    }

    fn ready_to_handle_unsafe_replica_read(&self, read_index: u64) -> bool {
        unreachable!()
    }

    #[inline]
    fn is_splitting(&self) -> bool {
        unreachable!()
        // self.last_committed_split_idx > self.get_store().applied_index()
    }

    #[inline]
    fn is_merging(&self) -> bool {
        unreachable!()
        // self.last_committed_prepare_merge_idx > self.get_store().applied_index()
        //     || self.pending_merge_state.is_some()
    }

    // Checks merge strictly, it checks whether there is any ongoing merge by
    // tracking last proposed prepare merge.
    // TODO: There is a false positives, proposed prepare merge may never be
    //       committed.
    fn is_merging_strict(&self) -> bool {
        unreachable!()
        // self.last_proposed_prepare_merge_idx > self.get_store().applied_index() || self.is_merging()
    }

    // Check if this peer can handle request_snapshot.
    pub fn ready_to_handle_request_snapshot(&mut self, request_index: u64) -> bool {
        unreachable!()
    }

    /// Checks if leader needs to keep sending logs for follower.
    ///
    /// In DrAutoSync mode, if leader goes to sleep before the region is sync,
    /// PD may wait longer time to reach sync state.
    pub fn replication_mode_need_catch_up(&self) -> bool {
        unreachable!()
        // self.replication_mode_version > 0
        //     && self.dr_auto_sync_state != DrAutoSyncState::Async
        //     && !self.replication_sync
    }

    pub fn handle_raft_ready_append(
        &mut self,
        ctx: &mut PollContext,
    ) -> Option<CollectedReady> {
        // if self.pending_remove {
        //     return None;
        // }
        // match self.mut_store().check_applying_snap() {
        //     CheckApplyingSnapStatus::Applying => {
        //         // If this peer is applying snapshot, we should not get a new ready.
        //         // There are two reasons in my opinion:
        //         //   1. If we handle a new ready and persist the data(e.g. entries),
        //         //      we can not tell raft-rs that this ready has been persisted because
        //         //      the ready need to be persisted one by one from raft-rs's view.
        //         //   2. When this peer is applying snapshot, the response msg should not
        //         //      be sent to leader, thus the leader will not send new entries to
        //         //      this peer. Although it's possible a new leader may send a AppendEntries
        //         //      msg to this peer, this possibility is very low. In most cases, there
        //         //      is no msg need to be handled.
        //         // So we choose to not get a new ready which makes the logic more clear.
        //         debug!(
        //             "still applying snapshot, skip further handling";
        //             "region_id" => self.region_id,
        //             "peer_id" => self.peer.get_id(),
        //         );
        //         return None;
        //     }
        //     CheckApplyingSnapStatus::Success => {
        //         fail_point!("raft_before_applying_snap_finished");
        //         // 0 means snapshot is scheduled after being restarted.
        //         if self.last_unpersisted_number != 0 {
        //             // Because we only handle raft ready when not applying snapshot, so following
        //             // line won't be called twice for the same snapshot.
        //             self.raft_group.advance_apply_to(self.last_applying_idx);
        //             self.cmd_epoch_checker.advance_apply(
        //                 self.last_applying_idx,
        //                 self.term(),
        //                 self.raft_group.store().region(),
        //             );
        //         }
        //         self.post_pending_read_index_on_replica(ctx);
        //         // Resume `read_progress`
        //         self.read_progress.resume();
        //         // Update apply index to `last_applying_idx`
        //         self.read_progress.update_applied(self.last_applying_idx);
        //         if !self.pending_messages.is_empty() {
        //             let msgs = mem::take(&mut self.pending_messages);
        //             self.send(&mut ctx.trans, msgs, &mut ctx.raft_metrics.send_message);
        //         }
        //     }
        //     CheckApplyingSnapStatus::Idle => {
        //         // FIXME: It's possible that the snapshot applying task is canceled.
        //         // Although it only happens when shutting down the store or destroying
        //         // the peer, it's still dengerous if continue to handle ready for the
        //         // peer. So it's better to revoke `JOB_STATUS_CANCELLING` to ensure all
        //         // started tasks can get finished correctly.
        //     }
        // }

        // let mut destroy_regions = vec![];
        if self.has_pending_snapshot() {
            unreachable!()
            // if !self.ready_to_handle_pending_snap() {
            //     let count = self.pending_request_snapshot_count.load(Ordering::SeqCst);
            //     debug!(
            //         "not ready to apply snapshot";
            //         "region_id" => self.region_id,
            //         "peer_id" => self.peer.get_id(),
            //         "apply_index" => self.get_store().applied_index(),
            //         "last_applying_index" => self.last_applying_idx,
            //         "pending_request_snapshot_count" => count,
            //     );
            //     return None;
            // }
            //
            // let meta = ctx.store_meta.lock().unwrap();
            // // For merge process, the stale source peer is destroyed asynchronously when applying
            // // snapshot or creating new peer. So here checks whether there is any overlap, if so,
            // // wait and do not handle raft ready.
            // if let Some(wait_destroy_regions) = meta.atomic_snap_regions.get(&self.region_id) {
            //     for (source_region_id, is_ready) in wait_destroy_regions {
            //         if !is_ready {
            //             info!(
            //                 "snapshot range overlaps, wait source destroy finish";
            //                 "region_id" => self.region_id,
            //                 "peer_id" => self.peer.get_id(),
            //                 "apply_index" => self.get_store().applied_index(),
            //                 "last_applying_index" => self.last_applying_idx,
            //                 "overlap_region_id" => source_region_id,
            //             );
            //             return None;
            //         }
            //         destroy_regions.push(meta.regions[source_region_id].clone());
            //     }
            // }
        }

        // if !self.raft_group.has_ready() {
        //     fail_point!("before_no_ready_gen_snap_task", |_| None);
        //     // Generating snapshot task won't set ready for raft group.
        //     if let Some(gen_task) = self.mut_store().take_gen_snap_task() {
        //         self.pending_request_snapshot_count
        //             .fetch_add(1, Ordering::SeqCst);
        //         ctx.apply_router
        //             .schedule_task(self.region_id, ApplyTask::Snapshot(gen_task));
        //     }
        //     return None;
        // }

        // fail_point!(
        //     "before_handle_raft_ready_1003",
        //     self.peer.get_id() == 1003 && self.is_leader(),
        //     |_| None
        // );
        //
        // fail_point!(
        //     "before_handle_snapshot_ready_3",
        //     self.peer.get_id() == 3 && self.get_pending_snapshot().is_some(),
        //     |_| None
        // );

        debug!(
            "handle raft ready";
            "region_id" => self.region_id,
            "peer_id" => self.peer.get_id(),
        );

        let mut ready = self.raft_group.ready();

        // Update it after unstable entries pagination is introduced.
        debug_assert!(ready.entries().last().map_or_else(
            || true,
            |entry| entry.index == self.raft_group.raft.raft_log.last_index()
        ));
        // MEMTRACE_RAFT_ENTRIES.trace(TraceEvent::Sub(self.memtrace_raft_entries));
        // self.memtrace_raft_entries = 0;

        self.last_unpersisted_number = ready.number();

        if !ready.must_sync() {
            // If this ready need not to sync, the term, vote must not be changed,
            // entries and snapshot must be empty.
            if let Some(hs) = ready.hs() {
                assert_eq!(hs.get_term(), self.get_store().hard_state().get_term());
                assert_eq!(hs.get_vote(), self.get_store().hard_state().get_vote());
            }
            assert!(ready.entries().is_empty());
            assert!(ready.snapshot().is_empty());
        }

        // self.add_ready_metric(&ready, &mut ctx.raft_metrics.ready);

        self.on_role_changed(ctx, &ready);

        if let Some(hs) = ready.hs() {
            let pre_commit_index = self.get_store().commit_index();
            assert!(hs.get_commit() >= pre_commit_index);
            if self.is_leader() {
                self.on_leader_commit_idx_changed(pre_commit_index, hs.get_commit());
            }
        }

        if !ready.messages().is_empty() {
            if !self.is_leader() {
                // fail_point!("raft_before_follower_send");
            }
            let msgs = ready.take_messages();
            // self.send(&mut ctx.trans, msgs, &mut ctx.raft_metrics.send_message);
            self.send(&mut ctx.trans, msgs);
        }

        // self.apply_reads(ctx, &ready);

        if !ready.committed_entries().is_empty() {
            self.handle_raft_committed_entries(ctx, ready.take_committed_entries());
        }
        // // Check whether there is a pending generate snapshot task, the task
        // // needs to be sent to the apply system.
        // // Always sending snapshot task behind apply task, so it gets latest
        // // snapshot.
        // if let Some(gen_task) = self.mut_store().take_gen_snap_task() {
        //     self.pending_request_snapshot_count
        //         .fetch_add(1, Ordering::SeqCst);
        //     ctx.apply_router
        //         .schedule_task(self.region_id, ApplyTask::Snapshot(gen_task));
        // }

        let invoke_ctx = self.mut_store().handle_raft_ready(ctx, &mut ready);

        Some(CollectedReady::new(invoke_ctx, ready))
    }

    pub fn post_raft_ready_append(
        &mut self,
        ctx: &mut PollContext,
        invoke_ctx: InvokeContext,
        ready: &mut Ready,
    // ) -> Option<ApplySnapResult> {
    ) -> () {
        // if invoke_ctx.has_snapshot() {
        //     // When apply snapshot, there is no log applied and not compacted yet.
        //     self.raft_log_size_hint = 0;
        //     // Pause `read_progress` to prevent serving stale read while applying snapshot
        //     self.read_progress.pause();
        // }

        // let apply_snap_result = self.mut_store().post_ready(invoke_ctx);
        self.mut_store().post_ready(invoke_ctx);
        let has_msg = !ready.persisted_messages().is_empty();

        // if apply_snap_result.is_some() {
        //     self.pending_messages = ready.take_persisted_messages();
        //
        //     // The peer may change from learner to voter after snapshot applied.
        //     let peer = self
        //         .region()
        //         .get_peers()
        //         .iter()
        //         .find(|p| p.get_id() == self.peer.get_id())
        //         .unwrap()
        //         .clone();
        //     if peer != self.peer {
        //         info!(
        //             "meta changed in applying snapshot";
        //             "region_id" => self.region_id,
        //             "peer_id" => self.peer.get_id(),
        //             "before" => ?self.peer,
        //             "after" => ?peer,
        //         );
        //         self.peer = peer;
        //     };
        //
        //     self.activate(ctx);
        //     let mut meta = ctx.store_meta.lock().unwrap();
        //     meta.readers
        //         .insert(self.region_id, ReadDelegate::from_peer(self));
        // } else
        if has_msg {
            let msgs = ready.take_persisted_messages();
            // self.send(&mut ctx.trans, msgs, &mut ctx.raft_metrics.send_message);
            self.send(&mut ctx.trans, msgs);
        }

        // apply_snap_result
    }

    pub fn handle_raft_committed_entries(
        &mut self,
        ctx: &mut PollContext,
        committed_entries: Vec<Entry>,
    ) {
        // fail_point!(
        //     "before_leader_handle_committed_entries",
        //     self.is_leader(),
        //     |_| ()
        // );

        assert!(
            !self.is_applying_snapshot(),
            "{} is applying snapshot when it is ready to handle committed entries",
            self.tag
        );
        if !committed_entries.is_empty() {
            // We must renew current_time because this value may be created a long time ago.
            // If we do not renew it, this time may be smaller than propose_time of a command,
            // which was proposed in another thread while this thread receives its AppendEntriesResponse
            // and is ready to calculate its commit-log-duration.
            ctx.current_time.replace(monotonic_raw_now());
        }
        // // Leader needs to update lease.
        // let mut lease_to_be_updated = self.is_leader();
        // for entry in committed_entries.iter().rev() {
        //     // raft meta is very small, can be ignored.
        //     self.raft_log_size_hint += entry.get_data().len() as u64;
        //     if lease_to_be_updated {
        //         let propose_time = self
        //             .proposals
        //             .find_propose_time(entry.get_term(), entry.get_index());
        //         if let Some(propose_time) = propose_time {
        //             ctx.raft_metrics.commit_log.observe(duration_to_sec(
        //                 (ctx.current_time.unwrap() - propose_time).to_std().unwrap(),
        //             ));
        //             self.maybe_renew_leader_lease(propose_time, ctx, None);
        //             lease_to_be_updated = false;
        //         }
        //     }
        //
        //     fail_point!(
        //         "leader_commit_prepare_merge",
        //         {
        //             let ctx = ProposalContext::from_bytes(&entry.context);
        //             self.is_leader()
        //                 && entry.term == self.term()
        //                 && ctx.contains(ProposalContext::PREPARE_MERGE)
        //         },
        //         |_| {}
        //     );
        // }
        if let Some(last_entry) = committed_entries.last() {
            self.last_applying_idx = last_entry.get_index();
            if self.last_applying_idx >= self.last_urgent_proposal_idx {
                // Urgent requests are flushed, make it lazy again.
                self.raft_group.skip_bcast_commit(true);
                self.last_urgent_proposal_idx = u64::MAX;
            }
            let cbs = if !self.proposals.is_empty() {
                let current_term = self.term();
                let cbs = committed_entries
                    .iter()
                    .filter_map(|e| {
                        self.proposals
                            .find_proposal(e.get_term(), e.get_index(), current_term)
                    })
                    .map(|mut p| {
                        if p.must_pass_epoch_check {
                            // In this case the apply can be guaranteed to be successful. Invoke the
                            // on_committed callback if necessary.
                            p.cb.invoke_committed();
                        }
                        p
                    })
                    .collect();
                self.proposals.gc();
                cbs
            } else {
                vec![]
            };
            let apply = Apply::new(
                self.peer_id(),
                self.region_id,
                self.term(),
                committed_entries,
                cbs,
            );
            // self.mut_store().trace_cached_entries(apply.entries.clone());
            // if needs_evict_entry_cache() {
            //     // Compact all cached entries instead of half evict.
            //     self.mut_store().evict_cache(false);
            // }
            ctx.apply_router
                .schedule_task(self.region_id, ApplyTask::apply(apply));
        }
        // fail_point!("after_send_to_apply_1003", self.peer_id() == 1003, |_| {});
    }

    pub fn handle_raft_ready_advance(
        &mut self,
        ctx: &mut PollContext,
        ready: Ready,
    ) {
        assert_eq!(ready.number(), self.last_unpersisted_number);
        if !ready.snapshot().is_empty() {
            unreachable!();
            // Snapshot's metadata has been applied.
            self.last_applying_idx = self.get_store().truncated_index();
            self.raft_group.advance_append_async(ready);
            // The ready is persisted, but we don't want to handle following light
            // ready immediately to avoid flow out of control, so use
            // `on_persist_ready` instead of `advance_append`.
            // We don't need to set `has_ready` to true, as snapshot is always
            // checked when ticking.
            self.raft_group
                .on_persist_ready(self.last_unpersisted_number);
            return;
        }

        let mut light_rd = self.raft_group.advance_append(ready);

        // self.add_light_ready_metric(&light_rd, &mut ctx.raft_metrics.ready);

        if let Some(commit_index) = light_rd.commit_index() {
            let pre_commit_index = self.get_store().commit_index();
            assert!(commit_index >= pre_commit_index);
            // No need to persist the commit index but the one in memory
            // (i.e. commit of hardstate in PeerStorage) should be updated.
            self.mut_store().set_commit_index(commit_index);
            if self.is_leader() {
                self.on_leader_commit_idx_changed(pre_commit_index, commit_index);
            }
        }

        if !light_rd.messages().is_empty() {
            if !self.is_leader() {
                // fail_point!("raft_before_follower_send");
            }
            let msgs = light_rd.take_messages();
            // self.send(&mut ctx.trans, msgs, &mut ctx.raft_metrics.send_message);
            self.send(&mut ctx.trans, msgs);
        }

        if !light_rd.committed_entries().is_empty() {
            self.handle_raft_committed_entries(ctx, light_rd.take_committed_entries());
        }
    }

    fn response_read(
        &self,
        // read: &mut ReadIndexRequest<EK::Snapshot>,
        ctx: &mut PollContext,
        replica_read: bool,
    ) {
        unreachable!()
    }

    /// Responses to the ready read index request on the replica, the replica is not a leader.
    fn post_pending_read_index_on_replica(&mut self, ctx: &mut PollContext) {
        unreachable!()
    }

    fn send_read_command(
        &self,
        ctx: &mut PollContext,
        read_cmd: RaftCommand,
    ) {
        unreachable!()
    }

    fn apply_reads(&mut self, ctx: &mut PollContext, ready: &Ready) {
        unreachable!()
    }

    pub fn post_apply(
        &mut self,
        ctx: &mut PollContext,
        apply_state: RaftApplyState,
        applied_index_term: u64,
        // apply_metrics: &ApplyMetrics,
    ) -> bool {
        let mut has_ready = false;

        if self.is_applying_snapshot() {
            panic!("{} should not applying snapshot.", self.tag);
        }

        let applied_index = apply_state.get_applied_index();
        self.raft_group.advance_apply_to(applied_index);

        // self.cmd_epoch_checker.advance_apply(
        //     applied_index,
        //     self.term(),
        //     self.raft_group.store().region(),
        // );

        // if !self.is_leader() {
        //     self.mut_store()
        //         .compact_cache_to(apply_state.applied_index + 1);
        // }

        let progress_to_be_updated = self.mut_store().applied_index_term() != applied_index_term;
        self.mut_store().set_applied_state(apply_state);
        self.mut_store().set_applied_term(applied_index_term);

        // self.peer_stat.written_keys += apply_metrics.written_keys;
        // self.peer_stat.written_bytes += apply_metrics.written_bytes;
        // self.peer_stat
        //     .written_query_stats
        //     .add_query_stats(&apply_metrics.written_query_stats.0);
        // self.delete_keys_hint += apply_metrics.delete_keys_hint;
        // let diff = self.size_diff_hint as i64 + apply_metrics.size_diff_hint;
        // self.size_diff_hint = cmp::max(diff, 0) as u64;

        // if self.has_pending_snapshot() && self.ready_to_handle_pending_snap() {
        //     has_ready = true;
        // }
        // if !self.is_leader() {
        //     self.post_pending_read_index_on_replica(ctx)
        // } else if self.ready_to_handle_read() {
        //     while let Some(mut read) = self.pending_reads.pop_front() {
        //         self.response_read(&mut read, ctx, false);
        //     }
        // }
        // self.pending_reads.gc();

        // self.read_progress.update_applied(applied_index);

        // // Only leaders need to update applied_index_term.
        // if progress_to_be_updated && self.is_leader() {
        //     if applied_index_term == self.term() {
        //         ctx.coprocessor_host
        //             .on_applied_current_term(StateRole::Leader, self.region());
        //     }
        //     let progress = ReadProgress::applied_index_term(applied_index_term);
        //     let mut meta = ctx.store_meta.lock().unwrap();
        //     let reader = meta.readers.get_mut(&self.region_id).unwrap();
        //     self.maybe_update_read_progress(reader, progress);
        // }
        has_ready
    }

    pub fn post_split(&mut self) {
        unreachable!()
        // // Reset delete_keys_hint and size_diff_hint.
        // self.delete_keys_hint = 0;
        // self.size_diff_hint = 0;
    }

    /// Try to renew leader lease.
    fn maybe_renew_leader_lease(
        &mut self,
        ts: Timespec,
        ctx: &mut PollContext,
        // progress: Option<ReadProgress>,
    ) {
        unreachable!()
    }

    // fn maybe_update_read_progress(&self, reader: &mut ReadDelegate, progress: ReadProgress) {
    fn maybe_update_read_progress(&self,) {
        unreachable!()
    }

    pub fn maybe_campaign(&mut self, parent_is_leader: bool) -> bool {
        if self.region().get_peers().len() <= 1 {
            // The peer campaigned when it was created, no need to do it again.
            return false;
        }

        if !parent_is_leader {
            return false;
        }

        // If last peer is the leader of the region before split, it's intuitional for
        // it to become the leader of new split region.
        let _ = self.raft_group.campaign();
        true
    }

    /// Propose a request.
    ///
    /// Return true means the request has been proposed successfully.
    pub fn propose(
        &mut self,
        ctx: &mut PollContext,
        // mut cb: Callback<EK::Snapshot>,
        mut cb: Callback,
        req: RaftCmdRequest,
        mut err_resp: RaftCmdResponse,
    ) -> bool {
        // if self.pending_remove {
        //     return false;
        // }

        // ctx.raft_metrics.propose.all += 1;

        let req_admin_cmd_type = if !req.has_admin_request() {
            None
        } else {
            Some(req.get_admin_request().get_cmd_type())
        };
        let is_urgent = is_request_urgent(&req);

        let policy = self.inspect(&req);
        let res = match policy {
            Ok(RequestPolicy::ReadLocal) | Ok(RequestPolicy::StaleRead) => {
                unreachable!()
                // self.read_local(ctx, req, cb);
                // return false;
            }
            Ok(RequestPolicy::ReadIndex) => {
                unreachable!( )
                // return self.read_index(ctx, req, err_resp, cb),
            }
            Ok(RequestPolicy::ProposeNormal) => {
                // let store_id = ctx.store_id();
                // if disk::disk_full_precheck(store_id) || ctx.is_disk_full {
                //     Err(Error::Timeout("disk full".to_owned()))
                // } else {
                    self.propose_normal(ctx, req)
                // }
            }
            Ok(RequestPolicy::ProposeTransferLeader) => {
                unreachable!()
                // return self.propose_transfer_leader(ctx, req, cb);
            }
            Ok(RequestPolicy::ProposeConfChange) => {
                unreachable!()
                // self.propose_conf_change(ctx, &req),
            }
            Err(e) => Err(e),
        };

        match res {
            Err(e) => {
                cmd_resp::bind_error(&mut err_resp, e);
                cb.invoke_with_response(err_resp);
                false
            }
            Ok(Either::Right(idx)) => {
                // if !cb.is_none() {
                //     self.cmd_epoch_checker.attach_to_conflict_cmd(idx, cb);
                // }
                // false
                unreachable!()
            }
            Ok(Either::Left(idx)) => {
                let has_applied_to_current_term = self.has_applied_to_current_term();
                if has_applied_to_current_term {
                    // After this peer has applied to current term and passed above checking including `cmd_epoch_checker`,
                    // we can safely guarantee that this proposal will be committed if there is no abnormal leader transfer
                    // in the near future. Thus proposed callback can be called.
                    cb.invoke_proposed();
                }
                if is_urgent {
                    self.last_urgent_proposal_idx = idx;
                    // Eager flush to make urgent proposal be applied on all nodes as soon as
                    // possible.
                    self.raft_group.skip_bcast_commit(false);
                }
                // self.should_wake_up = true;
                let p = Proposal {
                    // is_conf_change: req_admin_cmd_type == Some(AdminCmdType::ChangePeer)
                    //     || req_admin_cmd_type == Some(AdminCmdType::ChangePeerV2),
                    is_conf_change: false,
                    index: idx,
                    term: self.term(),
                    cb,
                    propose_time: None,
                    must_pass_epoch_check: has_applied_to_current_term,
                };
                // if let Some(cmd_type) = req_admin_cmd_type {
                //     self.cmd_epoch_checker
                //         .post_propose(cmd_type, idx, self.term());
                // }
                self.post_propose(ctx, p);
                true
            }
        }
    }

    fn post_propose(
        &mut self,
        poll_ctx: &mut PollContext,
        // mut p: Proposal<EK::Snapshot>,
        mut p: Proposal,
    ) {
        // Try to renew leader lease on every consistent read/write request.
        if poll_ctx.current_time.is_none() {
            poll_ctx.current_time = Some(monotonic_raw_now());
        }
        p.propose_time = poll_ctx.current_time;

        self.proposals.push(p);
    }

    // TODO: set higher election priority of voter/incoming voter than demoting voter
    /// Validate the `ConfChange` requests and check whether it's safe to
    /// propose these conf change requests.
    /// It's safe iff at least the quorum of the Raft group is still healthy
    /// right after all conf change is applied.
    /// If 'allow_remove_leader' is false then the peer to be removed should
    /// not be the leader.
    fn check_conf_change(
        &mut self,
        ctx: &mut PollContext,
        // change_peers: &[ChangePeerRequest],
        // cc: &impl ConfChangeI,
    ) -> Result<()> {
        unreachable!()
    }

    /// Check if current joint state can handle this confchange
    // fn check_joint_state(&mut self, cc: &impl ConfChangeI) -> Result<ProgressTracker> {
    fn check_joint_state(&mut self,) {
        unreachable!()
    }

    fn transfer_leader(&mut self, peer: &metapb::Peer) {
        unreachable!()
    }

    fn pre_transfer_leader(&mut self, peer: &metapb::Peer) -> bool {
        unreachable!()
    }

    fn ready_to_transfer_leader(
        &self,
        ctx: &mut PollContext,
        mut index: u64,
        peer: &metapb::Peer,
    ) -> Option<&'static str> {
        unreachable!()
    }

    fn read_local(
        &mut self,
        ctx: &mut PollContext,
        req: RaftCmdRequest,
        // cb: Callback<EK::Snapshot>,
        cb: Callback,
    ) {
        unreachable!()
    }

    fn pre_read_index(&self) -> Result<()> {
        unreachable!()
    }

    pub fn has_unresolved_reads(&self) -> bool {
        unreachable!()
    }

    /// `ReadIndex` requests could be lost in network, so on followers commands could queue in
    /// `pending_reads` forever. Sending a new `ReadIndex` periodically can resolve this.
    // pub fn retry_pending_reads(&mut self, cfg: &Config) {
    pub fn retry_pending_reads(&mut self) {
        unreachable!()
    }

    // Returns a boolean to indicate whether the `read` is proposed or not.
    // For these cases it won't be proposed:
    // 1. The region is in merging or splitting;
    // 2. The message is stale and dropped by the Raft group internally;
    // 3. There is already a read request proposed in the current lease;
    fn read_index(
        &mut self,
        poll_ctx: &mut PollContext,
        mut req: RaftCmdRequest,
        mut err_resp: RaftCmdResponse,
        // cb: Callback<EK::Snapshot>,
        cb: Callback,
    ) -> bool {
        unreachable!()
    }

    /// Returns (minimal matched, minimal committed_index)
    ///
    /// For now, it is only used in merge.
    pub fn get_min_progress(&self) -> Result<(u64, u64)> {
        unreachable!()
    }

    fn pre_propose_prepare_merge(
        &self,
        ctx: &mut PollContext,
        req: &mut RaftCmdRequest,
    ) -> Result<()> {
        unreachable!()
    }

    fn pre_propose(
        &self,
        poll_ctx: &mut PollContext,
        req: &mut RaftCmdRequest,
    // ) -> Result<ProposalContext> {
    ) -> Result<()> {
        // poll_ctx.coprocessor_host.pre_propose(self.region(), req)?;
        // let mut ctx = ProposalContext::empty();
        //
        // if get_sync_log_from_request(req) {
        //     ctx.insert(ProposalContext::SYNC_LOG);
        // }
        //
        // if !req.has_admin_request() {
        //     return Ok(ctx);
        // }
        //
        // match req.get_admin_request().get_cmd_type() {
        //     AdminCmdType::Split | AdminCmdType::BatchSplit => ctx.insert(ProposalContext::SPLIT),
        //     AdminCmdType::PrepareMerge => {
        //         self.pre_propose_prepare_merge(poll_ctx, req)?;
        //         ctx.insert(ProposalContext::PREPARE_MERGE);
        //     }
        //     _ => {}
        // }
        //
        // Ok(ctx)
        Ok(())
    }

    /// Propose normal request to raft
    ///
    /// Returns Ok(Either::Left(index)) means the proposal is proposed successfully and is located on `index` position.
    /// Ok(Either::Right(index)) means the proposal is rejected by `CmdEpochChecker` and the `index` is the position of
    /// the last conflict admin cmd.
    fn propose_normal(
        &mut self,
        poll_ctx: &mut PollContext,
        mut req: RaftCmdRequest,
    ) -> Result<Either<u64, u64>> {
        // if self.pending_merge_state.is_some()
        //     && req.get_admin_request().get_cmd_type() != AdminCmdType::RollbackMerge
        // {
        //     return Err(Error::ProposalInMergingMode(self.region_id));
        // }

        // poll_ctx.raft_metrics.propose.normal += 1;

        // if self.has_applied_to_current_term() {
        //     // Only when applied index's term is equal to current leader's term, the information
        //     // in epoch checker is up to date and can be used to check epoch.
        //     if let Some(index) = self
        //         .cmd_epoch_checker
        //         .propose_check_epoch(&req, self.term())
        //     {
        //         return Ok(Either::Right(index));
        //     }
        // } else if req.has_admin_request() {
        //     // The admin request is rejected because it may need to update epoch checker which
        //     // introduces an uncertainty and may breaks the correctness of epoch checker.
        //     return Err(box_err!(
        //         "{} peer has not applied to current term, applied_term {}, current_term {}",
        //         self.tag,
        //         self.get_store().applied_index_term(),
        //         self.term()
        //     ));
        // }

        // // TODO: validate request for unexpected changes.
        // let ctx = match self.pre_propose(poll_ctx, &mut req) {
        //     Ok(ctx) => ctx,
        //     Err(e) => {
        //         warn!(
        //             "skip proposal";
        //             "region_id" => self.region_id,
        //             "peer_id" => self.peer.get_id(),
        //             "err" => ?e,
        //             "error_code" => %e.error_code(),
        //         );
        //         return Err(e);
        //     }
        // };

        let data = req.write_to_bytes()?;

        // // TODO: use local histogram metrics
        // PEER_PROPOSE_LOG_SIZE_HISTOGRAM.observe(data.len() as f64);

        // if data.len() as u64 > poll_ctx.cfg.raft_entry_max_size.0 {
        if data.len() as u64 > 8 * 1024 * 1024 * 1024 {
            error!(
                "entry is too large";
                "region_id" => self.region_id,
                "peer_id" => self.peer.get_id(),
                "size" => data.len(),
            );
            return Err(Error::RaftEntryTooLarge {
                region_id: self.region_id,
                entry_size: data.len() as u64,
            });
        }

        let propose_index = self.next_proposal_index();
        // self.raft_group.propose(ctx.to_vec(), data)?;
        self.raft_group.propose(vec![], data)?;
        if self.next_proposal_index() == propose_index {
            // The message is dropped silently, this usually due to leader absence
            // or transferring leader. Both cases can be considered as NotLeader error.
            return Err(Error::NotLeader(self.region_id, None));
        }

        // if ctx.contains(ProposalContext::PREPARE_MERGE) {
        //     self.last_proposed_prepare_merge_idx = propose_index;
        // }

        Ok(Either::Left(propose_index))
    }

    fn execute_transfer_leader(
        &mut self,
        ctx: &mut PollContext,
        msg: &eraftpb::Message,
    ) {
        unreachable!()
    }

    /// Return true to if the transfer leader request is accepted.
    ///
    /// When transferring leadership begins, leader sends a pre-transfer
    /// to target follower first to ensures it's ready to become leader.
    /// After that the real transfer leader process begin.
    ///
    /// 1. pre_transfer_leader on leader:
    ///     Leader will send a MsgTransferLeader to follower.
    /// 2. execute_transfer_leader on follower
    ///     If follower passes all necessary checks, it will reply an
    ///     ACK with type MsgTransferLeader and its promised persistent index.
    /// 3. execute_transfer_leader on leader:
    ///     Leader checks if it's appropriate to transfer leadership. If it
    ///     does, it calls raft transfer_leader API to do the remaining work.
    ///
    /// See also: tikv/rfcs#37.
    fn propose_transfer_leader(
        &mut self,
        ctx: &mut PollContext,
        req: RaftCmdRequest,
        // cb: Callback<EK::Snapshot>,
        cb: Callback,
    ) -> bool {
        unreachable!()
    }

    // Fails in such cases:
    // 1. A pending conf change has not been applied yet;
    // 2. Removing the leader is not allowed in the configuration;
    // 3. The conf change makes the raft group not healthy;
    // 4. The conf change is dropped by raft group internally.
    /// Returns Ok(Either::Left(index)) means the proposal is proposed successfully and is located on `index` position.
    /// Ok(Either::Right(index)) means the proposal is rejected by `CmdEpochChecker` and the `index` is the position of
    /// the last conflict admin cmd.
    fn propose_conf_change(
        &mut self,
        ctx: &mut PollContext,
        req: &RaftCmdRequest,
    ) -> Result<Either<u64, u64>> {
        unreachable!()
    }

    // Fails in such cases:
    // 1. A pending conf change has not been applied yet;
    // 2. Removing the leader is not allowed in the configuration;
    // 3. The conf change makes the raft group not healthy;
    // 4. The conf change is dropped by raft group internally.
    fn propose_conf_change_internal(
        &mut self,
        // ctx: &mut PollContext<EK, ER, T>,
        // change_peer: CP,
        data: Vec<u8>,
    ) -> Result<u64> {
        unreachable!()
    }

    // fn handle_read(
    //     &self,
    //     // ctx: &mut PollContext<EK, ER, T>,
    //     req: RaftCmdRequest,
    //     check_epoch: bool,
    //     read_index: Option<u64>,
    // ) -> ReadResponse<EK::Snapshot> {
    fn handle_read(
        &self,
        // ctx: &mut PollContext<EK, ER, T>,
        // req: RaftCmdRequest,
        // check_epoch: bool,
        // read_index: Option<u64>,
    ) -> () {
        unreachable!()
    }

    pub fn term(&self) -> u64 {
        self.raft_group.raft.term
    }

    pub fn stop(&mut self) {
        // self.mut_store().cancel_applying_snap();
        // self.pending_reads.clear_all(None);
    }

    // pub fn maybe_add_want_rollback_merge_peer(&mut self, peer_id: u64, extra_msg: &ExtraMessage) {
    pub fn maybe_add_want_rollback_merge_peer(&mut self,) {
        unreachable!()
    }

    pub fn add_want_rollback_merge_peer(&mut self, peer_id: u64) {
        unreachable!()
        // assert!(self.pending_merge_state.is_some());
        // self.want_rollback_merge_peers.insert(peer_id);
    }
}

impl Peer
{
    pub fn insert_peer_cache(&mut self, peer: metapb::Peer) {
        self.peer_cache.borrow_mut().insert(peer.get_id(), peer);
    }

    pub fn remove_peer_from_cache(&mut self, peer_id: u64) {
        self.peer_cache.borrow_mut().remove(&peer_id);
    }

    pub fn get_peer_from_cache(&self, peer_id: u64) -> Option<metapb::Peer> {
        if peer_id == 0 {
            return None;
        }
        // fail_point!("stale_peer_cache_2", peer_id == 2, |_| None);
        if let Some(peer) = self.peer_cache.borrow().get(&peer_id) {
            return Some(peer.clone());
        }

        // Try to find in region, if found, set in cache.
        for peer in self.region().get_peers() {
            if peer.get_id() == peer_id {
                self.peer_cache.borrow_mut().insert(peer_id, peer.clone());
                return Some(peer.clone());
            }
        }

        None
    }

    // fn region_replication_status(&mut self) -> Option<RegionReplicationStatus> {
    fn region_replication_status(&mut self) -> () {
        unreachable!()
    }

    pub fn heartbeat_pd(&mut self, ctx: &PollContext) {
        unreachable!()
    }

    fn prepare_raft_message(&self) -> RaftMessage {
        let mut send_msg = RaftMessage::default();
        send_msg.set_region_id(self.region_id);
        // set current epoch
        // send_msg.set_region_epoch(self.region().get_region_epoch().clone());
        send_msg.set_from_peer(self.peer.clone());
        send_msg
    }

    // pub fn send_extra_message(
    //     &self,
    //     msg: ExtraMessage,
    //     // trans: &mut T,
    //     to: &metapb::Peer,
    // ) {
    //     unreachable!()
    // }

    fn send_raft_message(&mut self, msg: eraftpb::Message, trans: &mut RaftClient) -> bool {
        let mut send_msg = self.prepare_raft_message();

        let to_peer = match self.get_peer_from_cache(msg.get_to()) {
            Some(p) => p,
            None => {
                warn!(
                    "failed to look up recipient peer";
                    "region_id" => self.region_id,
                    "peer_id" => self.peer.get_id(),
                    "to_peer" => msg.get_to(),
                );
                return false;
            }
        };

        let to_peer_id = to_peer.get_id();
        let to_store_id = to_peer.get_store_id();
        let msg_type = msg.get_msg_type();
        debug!(
            "send raft msg";
            "region_id" => self.region_id,
            "peer_id" => self.peer.get_id(),
            "msg_type" => ?msg_type,
            "msg_size" => msg.compute_size(),
            "to" => to_peer_id,
        );

        send_msg.set_to_peer(to_peer);

        // // There could be two cases:
        // // 1. Target peer already exists but has not established communication with leader yet
        // // 2. Target peer is added newly due to member change or region split, but it's not
        // //    created yet
        // // For both cases the region start key and end key are attached in RequestVote and
        // // Heartbeat message for the store of that peer to check whether to create a new peer
        // // when receiving these messages, or just to wait for a pending region split to perform
        // // later.
        // if self.get_store().is_initialized() && is_initial_msg(&msg) {
        //     let region = self.region();
        //     send_msg.set_start_key(region.get_start_key().to_vec());
        //     send_msg.set_end_key(region.get_end_key().to_vec());
        // }

        send_msg.set_message(msg);

        trans.send(send_msg);
        // if let Err(e) = trans.send(send_msg) {
        //     // We use metrics to observe failure on production.
        //     debug!(
        //         "failed to send msg to other peer";
        //         "region_id" => self.region_id,
        //         "peer_id" => self.peer.get_id(),
        //         "target_peer_id" => to_peer_id,
        //         "target_store_id" => to_store_id,
        //         "err" => ?e,
        //         "error_code" => %e.error_code(),
        //     );
        //     if to_peer_id == self.leader_id() {
        //         self.leader_unreachable = true;
        //     }
        //     // unreachable store
        //     self.raft_group.report_unreachable(to_peer_id);
        //     if msg_type == eraftpb::MessageType::MsgSnapshot {
        //         self.raft_group
        //             .report_snapshot(to_peer_id, SnapshotStatus::Failure);
        //     }
        //     return false;
        // }
        true
    }

    pub fn bcast_wake_up_message(&self, ctx: &mut PollContext) {
        unreachable!()
    }

    pub fn send_wake_up_message(
        &self,
        ctx: &mut PollContext,
        peer: &metapb::Peer,
    ) {
        unreachable!()
    }

    pub fn bcast_check_stale_peer_message(
        &mut self,
        ctx: &mut PollContext,
    ) {
        unreachable!()
    }

    pub fn on_check_stale_peer_response(
        &mut self,
        check_conf_ver: u64,
        check_peers: Vec<metapb::Peer>,
    ) {
        unreachable!()
    }

    pub fn send_want_rollback_merge(
        &self,
        premerge_commit: u64,
        ctx: &mut PollContext,
    ) {
        unreachable!()
    }

    // pub fn require_updating_max_ts(&self, pd_scheduler: &FutureScheduler<PdTask<EK>>) {
    pub fn require_updating_max_ts(&self) {
        unreachable!()
    }
}

/// `RequestPolicy` decides how we handle a request.
#[derive(Clone, PartialEq, Debug)]
pub enum RequestPolicy {
    // Handle the read request directly without dispatch.
    ReadLocal,
    StaleRead,
    // Handle the read request via raft's SafeReadIndex mechanism.
    ReadIndex,
    ProposeNormal,
    ProposeTransferLeader,
    ProposeConfChange,
}

/// `RequestInspector` makes `RequestPolicy` for requests.
pub trait RequestInspector {
    /// Has the current term been applied?
    fn has_applied_to_current_term(&mut self) -> bool;
    /// Inspects its lease.
    // fn inspect_lease(&mut self) -> LeaseState;

    /// Inspect a request, return a policy that tells us how to
    /// handle the request.
    fn inspect(&mut self, req: &RaftCmdRequest) -> Result<RequestPolicy> {
        if req.has_admin_request() {
            // if apply::is_conf_change_cmd(req) {
            //     return Ok(RequestPolicy::ProposeConfChange);
            // }
            // if get_transfer_leader_cmd(req).is_some() {
            //     return Ok(RequestPolicy::ProposeTransferLeader);
            // }
            return Ok(RequestPolicy::ProposeNormal);
        }

        let mut has_read = false;
        let mut has_write = false;
        for r in req.get_requests() {
            match r.get_cmd_type() {
                // CmdType::Get | CmdType::Snap | CmdType::ReadIndex => has_read = true,
                // CmdType::Delete | CmdType::Put | CmdType::DeleteRange | CmdType::IngestSst => {
                CmdType::MessageQueue => {
                    has_write = true
                }
                // CmdType::Prewrite | CmdType::Invalid => {
                CmdType::Invalid => {
                    unreachable!();
                    // return Err(box_err!(
                    //     "invalid cmd type {:?}, message maybe corrupted",
                    //     r.get_cmd_type()
                    // ));
                }
            }

            if has_read && has_write {
                return Err(box_err!("read and write can't be mixed in one batch"));
            }
        }

        if has_write {
            return Ok(RequestPolicy::ProposeNormal);
        }

        unreachable!()
        // let flags = WriteBatchFlags::from_bits_check(req.get_header().get_flags());
        // if flags.contains(WriteBatchFlags::STALE_READ) {
        //     return Ok(RequestPolicy::StaleRead);
        // }
        //
        // if req.get_header().get_read_quorum() {
        //     return Ok(RequestPolicy::ReadIndex);
        // }
        //
        // // If applied index's term is differ from current raft's term, leader transfer
        // // must happened, if read locally, we may read old value.
        // if !self.has_applied_to_current_term() {
        //     return Ok(RequestPolicy::ReadIndex);
        // }
        //
        // // Local read should be performed, if and only if leader is in lease.
        // // None for now.
        // match self.inspect_lease() {
        //     LeaseState::Valid => Ok(RequestPolicy::ReadLocal),
        //     LeaseState::Expired | LeaseState::Suspect => {
        //         // Perform a consistent read to Raft quorum and try to renew the leader lease.
        //         Ok(RequestPolicy::ReadIndex)
        //     }
        // }
    }
}

impl RequestInspector for Peer
{
    fn has_applied_to_current_term(&mut self) -> bool {
        self.get_store().applied_index_term() == self.term()
    }

    // fn inspect_lease(&mut self) -> LeaseState {
    // fn inspect_lease(&mut self) -> () {
    //     unreachable!()
    // }
}

// impl<EK, ER, T> ReadExecutor<EK> for PollContext<EK, ER, T>
//     where
//         EK: KvEngine,
//         ER: RaftEngine,
// {
//     fn get_engine(&self) -> &EK {
//         &self.engines.kv
//     }
//
//     fn get_snapshot(&mut self, _: Option<ThreadReadId>) -> Arc<EK::Snapshot> {
//         Arc::new(self.engines.kv.snapshot())
//     }
// }

// fn get_transfer_leader_cmd(msg: &RaftCmdRequest) -> Option<&TransferLeaderRequest> {
//     if !msg.has_admin_request() {
//         return None;
//     }
//     let req = msg.get_admin_request();
//     if !req.has_transfer_leader() {
//         return None;
//     }
//
//     Some(req.get_transfer_leader())
// }

// fn get_sync_log_from_request(msg: &RaftCmdRequest) -> bool {
//     if msg.has_admin_request() {
//         let req = msg.get_admin_request();
//         return matches!(
//             req.get_cmd_type(),
//             AdminCmdType::ChangePeer
//                 | AdminCmdType::ChangePeerV2
//                 | AdminCmdType::Split
//                 | AdminCmdType::BatchSplit
//                 | AdminCmdType::PrepareMerge
//                 | AdminCmdType::CommitMerge
//                 | AdminCmdType::RollbackMerge
//         );
//     }
//
//     msg.get_header().get_sync_log()
// }

/// We enable follower lazy commit to get a better performance.
/// But it may not be appropriate for some requests. This function
/// checks whether the request should be committed on all followers
/// as soon as possible.
fn is_request_urgent(req: &RaftCmdRequest) -> bool {
    if !req.has_admin_request() {
        return false;
    }

    // matches!(
    //     req.get_admin_request().get_cmd_type(),
    //     AdminCmdType::Split
    //         | AdminCmdType::BatchSplit
    //         | AdminCmdType::ChangePeer
    //         | AdminCmdType::ChangePeerV2
    //         | AdminCmdType::ComputeHash
    //         | AdminCmdType::VerifyHash
    //         | AdminCmdType::PrepareMerge
    //         | AdminCmdType::CommitMerge
    //         | AdminCmdType::RollbackMerge
    // )
    false
}

// fn make_transfer_leader_response() -> RaftCmdResponse {
//     let mut response = AdminResponse::default();
//     response.set_cmd_type(AdminCmdType::TransferLeader);
//     response.set_transfer_leader(TransferLeaderResponse::default());
//     let mut resp = RaftCmdResponse::default();
//     resp.set_admin_response(response);
//     resp
// }

/// A poor version of `Peer` to avoid port generic variables everywhere.
pub trait AbstractPeer {
    fn meta_peer(&self) -> &metapb::Peer;
    // fn group_state(&self) -> GroupState;
    fn region(&self) -> &metapb::Region;
    fn apply_state(&self) -> &RaftApplyState;
    fn raft_status(&self) -> raft::Status;
    fn raft_commit_index(&self) -> u64;
    fn raft_request_snapshot(&mut self, index: u64);
    // fn pending_merge_state(&self) -> Option<&MergeState>;
}




// Copyright 2018 TiKV Project Authors. Licensed under Apache-2.0.

// use std::borrow::Cow;
// use std::cell::Cell;
// use std::collections::Bound::{Excluded, Unbounded};
// use std::collections::VecDeque;
// use std::iter::Iterator;
// use std::time::Instant;
// use std::{cmp, mem, u64};
//
// use batch_system::{BasicMailbox, Fsm};
// use collections::HashMap;
// use engine_traits::CF_RAFT;
// use engine_traits::{Engines, KvEngine, RaftEngine, SSTMetaInfo, WriteBatchExt};
// use error_code::ErrorCodeExt;
// use fail::fail_point;
// use kvproto::errorpb;
// use kvproto::import_sstpb::SwitchMode;
// use kvproto::metapb::{self, Region, RegionEpoch};
// use kvproto::pdpb::CheckPolicy;
// use kvproto::raft_cmdpb::{
//     AdminCmdType, AdminRequest, CmdType, RaftCmdRequest, RaftCmdResponse, Request, StatusCmdType,
//     StatusResponse,
// };
// use kvproto::raft_serverpb::{
//     ExtraMessage, ExtraMessageType, MergeState, PeerState, RaftApplyState, RaftMessage,
//     RaftSnapshotData, RaftTruncatedState, RegionLocalState,
// };
// use kvproto::replication_modepb::{DrAutoSyncState, ReplicationMode};
// use protobuf::Message;
// use raft::eraftpb::{ConfChangeType, EntryType, MessageType};
// use raft::{self, Progress, ReadState, Ready, SnapshotStatus, StateRole, INVALID_INDEX, NO_LIMIT};
// use tikv_alloc::trace::TraceEvent;
// use tikv_util::mpsc::{self, LooseBoundedSender, Receiver};
// use tikv_util::sys::{disk, memory_usage_reaches_high_water};
// use tikv_util::time::duration_to_sec;
// use tikv_util::worker::{Scheduler, Stopped};
// use tikv_util::{box_err, debug, defer, error, info, trace, warn};
// use tikv_util::{escape, is_zero_duration, Either};
// use txn_types::WriteBatchFlags;
//
// use self::memtrace::*;
// use crate::coprocessor::RegionChangeEvent;
// use crate::store::cmd_resp::{bind_term, new_error};
// use crate::store::fsm::store::{PollContext, StoreMeta};
// use crate::store::fsm::{
//     apply, ApplyMetrics, ApplyTask, ApplyTaskRes, CatchUpLogs, ChangeObserver, ChangePeer,
//     ExecResult,
// };
// use crate::store::hibernate_state::{GroupState, HibernateState};
// use crate::store::local_metrics::RaftProposeMetrics;
// use crate::store::memory::*;
// use crate::store::metrics::*;
// use crate::store::msg::{Callback, ExtCallback, InspectedRaftMessage};
// use crate::store::peer::{ConsistencyState, Peer, StaleState};
// use crate::store::peer_storage::{ApplySnapResult, InvokeContext};
// use crate::store::transport::Transport;
// use crate::store::util::{is_learner, KeysInfoFormatter};
// use crate::store::worker::{
//     ConsistencyCheckTask, RaftlogGcTask, ReadDelegate, RegionTask, SplitCheckTask,
// };
// use crate::store::PdTask;
// use crate::store::{
//     util, AbstractPeer, CasualMessage, Config, MergeResultKind, PeerMsg, PeerTicks, RaftCommand,
//     SignificantMsg, SnapKey, StoreMsg,
// };
// use crate::{Error, Result};
// use keys::{self, enc_end_key, enc_start_key};
// use crate::rocks_engine::RocksEngine;
// use crate::cmd_resp::{new_error, bind_term};
// use std::hint::unreachable_unchecked;

/// Limits the maximum number of regions returned by error.
///
/// Another choice is using coprocessor batch limit, but 10 should be a good fit in most case.
const MAX_REGIONS_IN_ERROR: usize = 10;
const REGION_SPLIT_SKIP_MAX_COUNT: usize = 3;

// pub struct DestroyPeerJob {
//     pub initialized: bool,
//     pub region_id: u64,
//     pub peer: metapb::Peer,
// }

pub struct CollectedReady {
    /// The offset of source peer in the batch system.
    pub batch_offset: usize,
    pub ctx: InvokeContext,
    pub ready: Ready,
}

impl CollectedReady {
    pub fn new(ctx: InvokeContext, ready: Ready) -> CollectedReady {
        CollectedReady {
            batch_offset: 0,
            ctx,
            ready,
        }
    }
}

pub struct PeerFsm
{
    pub peer: Peer,
    /// A registry for all scheduled ticks. This can avoid scheduling ticks twice accidentally.
    tick_registry: PeerTicks,
    /// Ticks for speed up campaign in chaos state.
    ///
    /// Followers will keep ticking in Idle mode to measure how many ticks have been skipped.
    /// Once it becomes chaos, those skipped ticks will be ticked so that it can campaign
    /// quickly instead of waiting an election timeout.
    ///
    /// This will be reset to 0 once it receives any messages from leader.
    missing_ticks: usize,
    // hibernate_state: HibernateState,
    // stopped: bool,
    has_ready: bool,
    // mailbox: Option<BasicMailbox<PeerFsm>,
    pub receiver: channel::Receiver<PeerMsg>,
    // /// when snapshot is generating or sending, skip split check at most REGION_SPLIT_SKIT_MAX_COUNT times.
    // skip_split_count: usize,
    // /// Sometimes applied raft logs won't be compacted in time, because less compact means less
    // /// sync-log in apply threads. Stale logs will be deleted if the skip time reaches this
    // /// `skip_gc_raft_log_ticks`.
    // skip_gc_raft_log_ticks: usize,

    // Batch raft command which has the same header into an entry
    batch_req_builder: BatchRaftCmdRequestBuilder,

    max_inflight_msgs: usize,

    // trace: PeerMemoryTrace,
}

impl Drop for PeerFsm
{
    fn drop(&mut self) {
        self.peer.stop();
        let mut raft_messages_size = 0;
        while let Ok(msg) = self.receiver.try_recv() {
            let callback = match msg {
                PeerMsg::RaftCommand(cmd) => cmd.callback,
                // PeerMsg::CasualMessage(CasualMessage::SplitRegion { callback, .. }) => callback,
                PeerMsg::RaftMessage(im) => {
                    raft_messages_size += im.heap_size;
                    continue;
                }
                _ => continue,
            };

            let mut err = errorpb::Error::default();
            err.set_message("region is not found".to_owned());
            err.mut_region_not_found().set_region_id(self.region_id());
            let mut resp = RaftCmdResponse::default();
            resp.mut_header().set_error(err);
            callback.invoke_with_response(resp);
        }
        // (match self.hibernate_state.group_state() {
        //     GroupState::Idle => &HIBERNATED_PEER_STATE_GAUGE.hibernated,
        //     _ => &HIBERNATED_PEER_STATE_GAUGE.awaken,
        // })
        //     .dec();
        //
        // MEMTRACE_RAFT_MESSAGES.trace(TraceEvent::Sub(raft_messages_size));
        // MEMTRACE_RAFT_ENTRIES.trace(TraceEvent::Sub(self.peer.memtrace_raft_entries));
        //
        // let mut event = TraceEvent::default();
        // if let Some(e) = self.trace.reset(PeerMemoryTrace::default()) {
        //     event = event + e;
        // }
        // MEMTRACE_PEERS.trace(event);
    }
}

// pub type SenderFsmPair<EK, ER> = (LooseBoundedSender<PeerMsg<EK>>, Box<PeerFsm<EK, ER>>);
// pub type SenderFsmPair = (channel::Sender<PeerMsg>, PeerFsm);

impl PeerFsm
{
    // If we create the peer actively, like bootstrap/split/merge region, we should
    // use this function to create the peer. The region must contain the peer info
    // for this store.
    // pub fn create(
    //     store_id: u64,
    //     // cfg: &Config,
    //     // sched: Scheduler<RegionTask<EK::Snapshot>>,
    //     // engines: Engines<EK, ER>,
    //     engines: RocksEngine,
    //     region: &metapb::Region,
    // ) -> Result<SenderFsmPair> {
    //     let meta_peer = match util::find_peer(region, store_id) {
    //         None => {
    //             return Err(box_err!(
    //                 "find no peer for store {} in region {:?}",
    //                 store_id,
    //                 region
    //             ));
    //         }
    //         Some(peer) => peer.clone(),
    //     };
    //
    //     info!(
    //         "create peer";
    //         "region_id" => region.get_id(),
    //         "peer_id" => meta_peer.get_id(),
    //     );
    //     // HIBERNATED_PEER_STATE_GAUGE.awaken.inc();
    //     // let (tx, rx) = mpsc::loose_bounded(cfg.notify_capacity);
    //     let (tx, rx) = channel::unbounded();
    //     Ok((
    //         tx,
    //         PeerFsm {
    //             // peer: Peer::new(store_id, cfg, sched, engines, region, meta_peer)?,
    //             peer: Peer::new(store_id, engines, region, meta_peer)?,
    //             tick_registry: PeerTicks::empty(),
    //             missing_ticks: 0,
    //             // hibernate_state: HibernateState::ordered(),
    //             // stopped: false,
    //             has_ready: false,
    //             // mailbox: None,
    //             receiver: rx,
    //             // skip_split_count: 0,
    //             // skip_gc_raft_log_ticks: 0,
    //             // batch_req_builder: BatchRaftCmdRequestBuilder::new(
    //             //     cfg.raft_entry_max_size.0 as f64,
    //             // ),
    //             batch_req_builder: BatchRaftCmdRequestBuilder::new(
    //                 (8 * 1024 * 1024) as f64,
    //             ),
    //             // max_inflight_msgs: cfg.raft_max_inflight_msgs,
    //             max_inflight_msgs: 256,
    //             // trace: PeerMemoryTrace::default(),
    //         },
    //     ))
    // }

    pub fn create(
        store_id: u64,
        // cfg: &Config,
        // sched: Scheduler<RegionTask<EK::Snapshot>>,
        // engines: Engines<EK, ER>,
        engines: Engines,
        region: &metapb::Region,
        receiver: channel::Receiver<PeerMsg>,
    ) -> PeerFsm {
        let meta_peer = match util::find_peer(region, store_id) {
            None => {
                unreachable!();
            }
            Some(peer) => peer.clone(),
        };

        info!(
            "create peer";
            "region_id" => region.get_id(),
            "peer_id" => meta_peer.get_id(),
        );
        // HIBERNATED_PEER_STATE_GAUGE.awaken.inc();
        // let (tx, rx) = mpsc::loose_bounded(cfg.notify_capacity);

        PeerFsm {
            // peer: Peer::new(store_id, cfg, sched, engines, region, meta_peer)?,
            peer: Peer::new(store_id, engines, region, meta_peer).unwrap(),
            tick_registry: PeerTicks::empty(),
            missing_ticks: 0,
            // hibernate_state: HibernateState::ordered(),
            // stopped: false,
            has_ready: false,
            // mailbox: None,
            receiver,
            // skip_split_count: 0,
            // skip_gc_raft_log_ticks: 0,
            // batch_req_builder: BatchRaftCmdRequestBuilder::new(
            //     cfg.raft_entry_max_size.0 as f64,
            // ),
            batch_req_builder: BatchRaftCmdRequestBuilder::new(
                (8 * 1024 * 1024) as f64,
            ),
            // max_inflight_msgs: cfg.raft_max_inflight_msgs,
            max_inflight_msgs: 256,
            // trace: PeerMemoryTrace::default(),
        }
    }

    // The peer can be created from another node with raft membership changes, and we only
    // know the region_id and peer_id when creating this replicated peer, the region info
    // will be retrieved later after applying snapshot.
    pub fn replicate(
        store_id: u64,
        // cfg: &Config,
        // sched: Scheduler<RegionTask<EK::Snapshot>>,
        // engines: Engines<EK, ER>,
        region_id: u64,
        peer: metapb::Peer,
    ) -> Result<PeerFsm> {
        unreachable!()
    }

    #[inline]
    pub fn region_id(&self) -> u64 {
        self.peer.region().get_id()
    }

    #[inline]
    // pub fn get_peer(&self) -> &Peer<EK, ER> {
    pub fn get_peer(&self) -> &Peer {
        &self.peer
    }

    #[inline]
    pub fn peer_id(&self) -> u64 {
        self.peer.peer_id()
    }

    #[inline]
    pub fn stop(&mut self) {
        unreachable!()
        // self.stopped = true;
    }

    // pub fn set_pending_merge_state(&mut self, state: MergeState) {
    pub fn set_pending_merge_state(&mut self) {
        unreachable!()
        // self.peer.pending_merge_state = Some(state);
    }

    pub fn schedule_applying_snapshot(&mut self) {
        unreachable!()
        // self.peer.mut_store().schedule_applying_snapshot();
    }

    // pub fn reset_hibernate_state(&mut self, state: GroupState) {
    pub fn reset_hibernate_state(&mut self) {
        unreachable!()
        // self.hibernate_state.reset(state);
    }

    pub fn maybe_hibernate(&mut self) -> bool {
        unreachable!()
        // self.hibernate_state
        //     .maybe_hibernate(self.peer.peer_id(), self.peer.region())
    }

    // pub fn update_memory_trace(&mut self, event: &mut TraceEvent) {
    pub fn update_memory_trace(&mut self) {
        unreachable!()
    }
}

pub struct BatchRaftCmdRequestBuilder
{
    raft_entry_max_size: f64,
    batch_req_size: u32,
    request: Option<RaftCmdRequest>,
    callbacks: Vec<(Callback, usize)>,
}

impl BatchRaftCmdRequestBuilder
{
    fn new(raft_entry_max_size: f64) -> BatchRaftCmdRequestBuilder {
        BatchRaftCmdRequestBuilder {
            raft_entry_max_size,
            request: None,
            batch_req_size: 0,
            callbacks: vec![],
        }
    }

    fn can_batch(&self, req: &RaftCmdRequest, req_size: u32) -> bool {
        // No batch request whose size exceed 20% of raft_entry_max_size,
        // so total size of request in batch_raft_request would not exceed
        // (40% + 20%) of raft_entry_max_size
        if req.get_requests().is_empty() || f64::from(req_size) > self.raft_entry_max_size * 0.2 {
            return false;
        }
        for r in req.get_requests() {
            match r.get_cmd_type() {
                // CmdType::Delete | CmdType::Put | CmdType::Transfer | CmdType::MessageQueue=> (),
                CmdType::MessageQueue=> (),
                _ => {
                    return false;
                }
            }
        }

        if let Some(batch_req) = self.request.as_ref() {
            if batch_req.get_header() != req.get_header() {
                return false;
            }
        }
        true
    }

    // fn add(&mut self, cmd: RaftCommand<E::Snapshot>, req_size: u32) {
    fn add(&mut self, cmd: RaftCommand, req_size: u32) {
        let req_num = cmd.request.get_requests().len();
        let RaftCommand {
            mut request,
            callback,
            ..
        } = cmd;
        if let Some(batch_req) = self.request.as_mut() {
            let requests: Vec<_> = request.take_requests().into();
            for q in requests {
                batch_req.mut_requests().push(q);
            }
        } else {
            self.request = Some(request);
        };
        self.callbacks.push((callback, req_num));
        self.batch_req_size += req_size;
    }

    fn should_finish(&self) -> bool {
        if let Some(batch_req) = self.request.as_ref() {
            // Limit the size of batch request so that it will not exceed raft_entry_max_size after
            // adding header.
            if f64::from(self.batch_req_size) > self.raft_entry_max_size * 0.4 {
                return true;
            }
            // if batch_req.get_requests().len() > <E as WriteBatchExt>::WRITE_BATCH_MAX_KEYS {
            if batch_req.get_requests().len() > 256 {
                return true;
            }
        }
        false
    }

    // fn build(&mut self, metric: &mut RaftProposeMetrics) -> Option<RaftCommand<E::Snapshot>> {
    fn build(&mut self) -> Option<RaftCommand> {
        if let Some(req) = self.request.take() {
            debug!(
                "BatchRaftCmdRequestBuilder.build() is called, request_num {}, batch_size {}.",
                req.get_requests().len(),
                self.batch_req_size
            );

            self.batch_req_size = 0;
            if self.callbacks.len() == 1 {
                let (cb, _) = self.callbacks.pop().unwrap();
                return Some(RaftCommand::new(req, cb));
            }
            // metric.batch += self.callbacks.len() - 1;
            let mut cbs = std::mem::take(&mut self.callbacks);
            let proposed_cbs: Vec<ExtCallback> = cbs
                .iter_mut()
                .filter_map(|cb| {
                    if let Callback::Write { proposed_cb, .. } = &mut cb.0 {
                        proposed_cb.take()
                    } else {
                        None
                    }
                })
                .collect();
            let proposed_cb: Option<ExtCallback> = if proposed_cbs.is_empty() {
                None
            } else {
                Some(Box::new(move || {
                    for proposed_cb in proposed_cbs {
                        proposed_cb();
                    }
                }))
            };
            let committed_cbs: Vec<_> = cbs
                .iter_mut()
                .filter_map(|cb| {
                    if let Callback::Write { committed_cb, .. } = &mut cb.0 {
                        committed_cb.take()
                    } else {
                        None
                    }
                })
                .collect();
            let committed_cb: Option<ExtCallback> = if committed_cbs.is_empty() {
                None
            } else {
                Some(Box::new(move || {
                    for committed_cb in committed_cbs {
                        committed_cb();
                    }
                }))
            };
            let cb = Callback::write_ext(
                Box::new(move |resp| {
                    let mut last_index = 0;
                    let has_error = resp.response.get_header().has_error();
                    for (cb, req_num) in cbs {
                        let next_index = last_index + req_num;
                        let mut cmd_resp = RaftCmdResponse::default();
                        cmd_resp.set_header(resp.response.get_header().clone());
                        if !has_error {
                            cmd_resp.set_responses(
                                resp.response.get_responses()[last_index..next_index].into(),
                            );
                        }
                        cb.invoke_with_response(cmd_resp);
                        last_index = next_index;
                    }
                }),
                proposed_cb,
                committed_cb,
            );
            return Some(RaftCommand::new(req, cb));
        }
        None
    }
}

// impl<EK, ER> Fsm for PeerFsm<EK, ER>
//     where
//         EK: KvEngine,
//         ER: RaftEngine,
// {
//     type Message = PeerMsg<EK>;
//
//     #[inline]
//     fn is_stopped(&self) -> bool {
//         self.stopped
//     }
//
//     /// Set a mailbox to Fsm, which should be used to send message to itself.
//     #[inline]
//     fn set_mailbox(&mut self, mailbox: Cow<'_, BasicMailbox<Self>>)
//         where
//             Self: Sized,
//     {
//         self.mailbox = Some(mailbox.into_owned());
//     }
//
//     /// Take the mailbox from Fsm. Implementation should ensure there will be
//     /// no reference to mailbox after calling this method.
//     #[inline]
//     fn take_mailbox(&mut self) -> Option<BasicMailbox<Self>>
//         where
//             Self: Sized,
//     {
//         self.mailbox.take()
//     }
// }

pub struct PeerFsmDelegate<'a>
{
    fsm: &'a mut PeerFsm,
    ctx: &'a mut PollContext,
}

impl<'a> PeerFsmDelegate<'a>
{
    pub fn new(
        fsm: &'a mut PeerFsm,
        ctx: &'a mut PollContext,
    ) -> PeerFsmDelegate<'a> {
        PeerFsmDelegate { fsm, ctx }
    }

    pub fn handle_msgs(&mut self, msgs: &mut Vec<PeerMsg>) {
        if !msgs.is_empty() {
            debug!("handle_msgs of peerLoop starts to handle {} msgs", msgs.len());
        }

        for m in msgs.drain(..) {
            match m {
                PeerMsg::RaftMessage(msg) => {
                    if let Err(e) = self.on_raft_message(msg) {
                        // error!(%e;
                        //     "handle raft message err";
                        //     "region_id" => self.fsm.region_id(),
                        //     "peer_id" => self.fsm.peer_id(),
                        // );
                        warn!(
                            "handle raft message err";
                            "region_id" => self.fsm.region_id(),
                            "peer_id" => self.fsm.peer_id(),
                        );
                    }
                }
                PeerMsg::RaftCommand(cmd) => {
                    // self.ctx
                    //     .raft_metrics
                    //     .propose
                    //     .request_wait_time
                    //     .observe(duration_to_sec(cmd.send_time.elapsed()) as f64);
                    // if let Some(Err(e)) = cmd.deadline.map(|deadline| deadline.check()) {
                    //     cmd.callback.invoke_with_response(new_error(e.into()));
                    //     continue;
                    // }

                    let req_size = cmd.request.compute_size();
                    if self.fsm.batch_req_builder.can_batch(&cmd.request, req_size) {
                        self.fsm.batch_req_builder.add(cmd, req_size);
                        if self.fsm.batch_req_builder.should_finish() {
                            self.propose_batch_raft_command();
                        }
                    } else {
                        self.propose_batch_raft_command();
                        self.propose_raft_command(cmd.request, cmd.callback)
                    }
                }
                PeerMsg::Tick(tick) => self.on_tick(tick),
                PeerMsg::ApplyRes { res } => {
                    self.on_apply_res(res);
                }
                // PeerMsg::SignificantMsg(msg) => self.on_significant_msg(msg),
                // PeerMsg::CasualMessage(msg) => self.on_casual_msg(msg),
                PeerMsg::Start => self.start(),
                // PeerMsg::HeartbeatPd => {
                //     if self.fsm.peer.is_leader() {
                //         self.register_pd_heartbeat_tick()
                //     }
                // }
                // PeerMsg::Noop => {}
                // PeerMsg::UpdateReplicationMode => self.on_update_replication_mode(),
                // PeerMsg::Destroy(peer_id) => {
                //     if self.fsm.peer.peer_id() == peer_id {
                //         match self.fsm.peer.maybe_destroy(&self.ctx) {
                //             None => self.ctx.raft_metrics.message_dropped.applying_snap += 1,
                //             Some(job) => {
                //                 self.handle_destroy_peer(job);
                //             }
                //         }
                //     }
                // }
            }
        }
        // Propose batch request which may be still waiting for more raft-command
        self.propose_batch_raft_command();
    }

    fn propose_batch_raft_command(&mut self) {
        if let Some(cmd) = self
            .fsm
            .batch_req_builder
            // .build(&mut self.ctx.raft_metrics.propose)
            .build()
        {
            self.propose_raft_command(cmd.request, cmd.callback)
        }
    }

    fn on_update_replication_mode(&mut self) {
        unreachable!()
    }

    // fn on_casual_msg(&mut self, msg: CasualMessage<EK>) {
    fn on_casual_msg(&mut self) {
        unreachable!()
    }

    fn on_tick(&mut self, tick: PeerTicks) {
        // if self.fsm.stopped {
        //     return;
        // }
        trace!(
            "tick";
            "tick" => ?tick,
            "peer_id" => self.fsm.peer_id(),
            "region_id" => self.region_id(),
        );
        self.fsm.tick_registry.remove(tick);
        match tick {
            PeerTicks::RAFT => self.on_raft_base_tick(),
            PeerTicks::RAFT_LOG_GC => self.on_raft_gc_log_tick(false),
            // PeerTicks::PD_HEARTBEAT => self.on_pd_heartbeat_tick(),
            // PeerTicks::SPLIT_REGION_CHECK => self.on_split_region_check_tick(),
            // PeerTicks::CHECK_MERGE => self.on_check_merge(),
            // PeerTicks::CHECK_PEER_STALE_STATE => self.on_check_peer_stale_state_tick(),
            // PeerTicks::ENTRY_CACHE_EVICT => self.on_entry_cache_evict_tick(),
            _ => unreachable!(),
        }
    }

    fn start(&mut self) {
        self.register_raft_base_tick();
        self.register_raft_gc_log_tick();
        // self.register_pd_heartbeat_tick();
        // self.register_split_region_check_tick();
        // self.register_check_peer_stale_state_tick();
        // self.on_check_merge();
        // Apply committed entries more quickly.
        // Or if it's a leader. This implicitly means it's a singleton
        // because it becomes leader in `Peer::new` when it's a
        // singleton. It has a no-op entry that need to be persisted,
        // committed, and then it should apply it.
        if self.fsm.peer.raft_group.store().commit_index()
            > self.fsm.peer.raft_group.store().applied_index()
            || self.fsm.peer.is_leader()
        {
            self.fsm.has_ready = true;
        }
    }

    // fn on_gc_snap(&mut self, snaps: Vec<(SnapKey, bool)>) {
    //     unreachable!()
    // }

    fn on_clear_region_size(&mut self) {
        unreachable!()
    }

    fn on_capture_change(
        &mut self,
        // cmd: ChangeObserver,
        // region_epoch: RegionEpoch,
        // cb: Callback<EK::Snapshot>,
    ) {
        unreachable!()
    }

    // fn on_significant_msg(&mut self, msg: SignificantMsg<EK::Snapshot>) {
    fn on_significant_msg(&mut self) {
        unreachable!()
    }

    fn report_snapshot_status(&mut self, to_peer_id: u64, status: SnapshotStatus) {
        unreachable!()
    }

    fn on_leader_callback(&mut self, cb: Callback) {
        unreachable!()
    }

    fn on_role_changed(&mut self, ready: &Ready) {
        // Update leader lease when the Raft state changes.
        if let Some(ss) = ready.ss() {
            if StateRole::Leader == ss.raft_state {
                self.fsm.missing_ticks = 0;
                // self.register_split_region_check_tick();
                // self.fsm.peer.heartbeat_pd(&self.ctx);
                // self.register_pd_heartbeat_tick();
            }
        }
    }

    pub fn collect_ready(&mut self) {
        let has_ready = self.fsm.has_ready;
        self.fsm.has_ready = false;
        // if !has_ready || self.fsm.stopped {
        //     return;
        // }
        if !has_ready {
            return;
        }
        self.ctx.pending_count += 1;
        self.ctx.has_ready = true;
        let res = self.fsm.peer.handle_raft_ready_append(self.ctx);
        if let Some(mut r) = res {
            // This bases on an assumption that fsm array passed in `end` method will have
            // the same order of processing.
            r.batch_offset = self.ctx.processed_fsm_count;
            self.on_role_changed(&r.ready);
            if r.ctx.has_new_entries {
                self.register_raft_gc_log_tick();
                // self.register_entry_cache_evict_tick();
            }
            self.ctx.ready_res.push(r);
        }
    }

    pub fn post_raft_ready_append(&mut self, ready: CollectedReady) {
        if ready.ctx.region_id != self.fsm.region_id() {
            panic!(
                "{} region id not matched: {} # {}",
                self.fsm.peer.tag,
                ready.ctx.region_id,
                self.fsm.region_id()
            );
        }
        // let is_merging = self.fsm.peer.pending_merge_state.is_some();
        let CollectedReady { ctx, mut ready, .. } = ready;
        let res = self
            .fsm
            .peer
            .post_raft_ready_append(self.ctx, ctx, &mut ready);
        self.fsm.peer.handle_raft_ready_advance(self.ctx, ready);
        // if let Some(apply_res) = res {
        //     self.on_ready_apply_snapshot(apply_res);
        //     if is_merging {
        //         // After applying a snapshot, merge is rollbacked implicitly.
        //         self.on_ready_rollback_merge(0, None);
        //     }
        //     self.register_raft_base_tick();
        // }
        if self.fsm.peer.leader_unreachable {
            // self.fsm.reset_hibernate_state(GroupState::Chaos);
            self.register_raft_base_tick();
            self.fsm.peer.leader_unreachable = false;
        }
    }

    #[inline]
    fn region_id(&self) -> u64 {
        self.fsm.peer.region().get_id()
    }

    #[inline]
    fn region(&self) -> &Region {
        self.fsm.peer.region()
    }

    #[inline]
    fn store_id(&self) -> u64 {
        self.fsm.peer.peer.get_store_id()
    }

    #[inline]
    fn schedule_tick(&mut self, tick: PeerTicks) {
        if self.fsm.tick_registry.contains(tick) {
            return;
        }
        let idx = tick.bits() as usize;
        // if is_zero_duration(&self.ctx.tick_batch[idx].wait_duration) {
        //     return;
        // }
        trace!(
            "schedule tick";
            "tick" => ?tick,
            "timeout" => ?self.ctx.tick_batch[idx].wait_duration,
            "region_id" => self.region_id(),
            "peer_id" => self.fsm.peer_id(),
        );
        self.fsm.tick_registry.insert(tick);

        // let region_id = self.region_id();
        // let mb = match self.ctx.router.mailbox(region_id) {
        //     Some(mb) => mb,
        //     None => {
        //         self.fsm.tick_registry.remove(tick);
        //         error!(
        //             "failed to get mailbox";
        //             "region_id" => self.fsm.region_id(),
        //             "peer_id" => self.fsm.peer_id(),
        //             "tick" => ?tick,
        //         );
        //         return;
        //     }
        // };
        let mb = self.ctx.router.router.clone();
        let peer_id = self.fsm.peer.peer_id();
        let cb = Box::new(move || {
            // This can happen only when the peer is about to be destroyed
            // or the node is shutting down. So it's OK to not to clean up
            // registry.
            // if let Err(e) = mb.force_send(PeerMsg::Tick(tick)) {
            //     debug!(
            //         "failed to schedule peer tick";
            //         "region_id" => region_id,
            //         "peer_id" => peer_id,
            //         "tick" => ?tick,
            //         "err" => %e,
            //     );
            // }
            mb.send(PeerMsg::Tick(tick)).unwrap();
        });
        self.ctx.tick_batch[idx].ticks.push(cb);
    }

    fn register_raft_base_tick(&mut self) {
        // If we register raft base tick failed, the whole raft can't run correctly,
        // TODO: shutdown the store?
        self.schedule_tick(PeerTicks::RAFT)
    }

    fn on_raft_base_tick(&mut self) {
        // if self.fsm.peer.pending_remove {
        //     self.fsm.peer.mut_store().flush_cache_metrics();
        //     return;
        // }
        // When having pending snapshot, if election timeout is met, it can't pass
        // the pending conf change check because first index has been updated to
        // a value that is larger than last index.
        // if self.fsm.peer.is_applying_snapshot() || self.fsm.peer.has_pending_snapshot() {
        //     // need to check if snapshot is applied.
        //     self.fsm.has_ready = true;
        //     self.fsm.missing_ticks = 0;
        //     self.register_raft_base_tick();
        //     return;
        // }

        // self.fsm.peer.retry_pending_reads(&self.ctx.cfg);

        // let mut res = None;
        // if self.ctx.cfg.hibernate_regions {
        //     if self.fsm.hibernate_state.group_state() == GroupState::Idle {
        //         // missing_ticks should be less than election timeout ticks otherwise
        //         // follower may tick more than an election timeout in chaos state.
        //         // Before stopping tick, `missing_tick` should be `raft_election_timeout_ticks` - 2
        //         // - `raft_heartbeat_ticks` (default 10 - 2 - 2 = 6)
        //         // and the follwer's `election_elapsed` in raft-rs is 1.
        //         // After the group state becomes Chaos, the next tick will call `raft_group.tick`
        //         // `missing_tick` + 1 times(default 7).
        //         // Then the follower's `election_elapsed` will be 1 + `missing_tick` + 1
        //         // (default 1 + 6 + 1 = 8) which is less than the min election timeout.
        //         // The reason is that we don't want let all followers become (pre)candidate if one
        //         // follower may receive a request, then becomes (pre)candidate and sends (pre)vote msg
        //         // to others. As long as the leader can wake up and broadcast hearbeats in one `raft_heartbeat_ticks`
        //         // time(default 2s), no more followers will wake up and sends vote msg again.
        //         if self.fsm.missing_ticks + 2 + self.ctx.cfg.raft_heartbeat_ticks
        //             < self.ctx.cfg.raft_election_timeout_ticks
        //         {
        //             self.register_raft_base_tick();
        //             self.fsm.missing_ticks += 1;
        //         }
        //         return;
        //     }
        //     res = Some(self.fsm.peer.check_before_tick(&self.ctx.cfg));
        //     if self.fsm.missing_ticks > 0 {
        //         for _ in 0..self.fsm.missing_ticks {
        //             if self.fsm.peer.raft_group.tick() {
        //                 self.fsm.has_ready = true;
        //             }
        //         }
        //         self.fsm.missing_ticks = 0;
        //     }
        // }
        if self.fsm.peer.raft_group.tick() {
            self.fsm.has_ready = true;
        }

        // self.fsm.peer.mut_store().flush_cache_metrics();

        // // Keep ticking if there are still pending read requests or this node is within hibernate timeout.
        // if res.is_none() /* hibernate_region is false */ ||
        //     !self.fsm.peer.check_after_tick(self.fsm.hibernate_state.group_state(), res.unwrap()) ||
        //     (self.fsm.peer.is_leader() && !self.all_agree_to_hibernate())
        // {
        //     self.register_raft_base_tick();
        //     // We need pd heartbeat tick to collect down peers and pending peers.
        //     self.register_pd_heartbeat_tick();
        //     return;
        // }

        // todo(glengeng): added by me
        self.register_raft_base_tick();

        // debug!("stop ticking"; "region_id" => self.region_id(), "peer_id" => self.fsm.peer_id(), "res" => ?res);
        // self.fsm.reset_hibernate_state(GroupState::Idle);
        // // Followers will stop ticking at L789. Keep ticking for followers
        // // to allow it to campaign quickly when abnormal situation is detected.
        // if !self.fsm.peer.is_leader() {
        //     self.register_raft_base_tick();
        // } else {
        //     self.register_pd_heartbeat_tick();
        // }
    }

    fn on_apply_res(&mut self, res: ApplyTaskRes) {
        // fail_point!("on_apply_res", |_| {});
        match res {
            ApplyTaskRes::Apply(mut res) => {
                debug!(
                    "async apply finish";
                    "region_id" => self.region_id(),
                    "peer_id" => self.fsm.peer_id(),
                    "res" => ?res,
                );
                // self.on_ready_result(&mut res.exec_res, &res.metrics);
                self.on_ready_result(&mut res.exec_res);
                // if self.fsm.stopped {
                //     return;
                // }
                self.fsm.has_ready |= self.fsm.peer.post_apply(
                    self.ctx,
                    res.apply_state,
                    res.applied_index_term,
                    // &res.metrics,
                );
                // After applying, several metrics are updated, report it to pd to
                // get fair schedule.
                // if self.fsm.peer.is_leader() {
                //     self.register_pd_heartbeat_tick();
                //     self.register_split_region_check_tick();
                // }
            }
            // ApplyTaskRes::Destroy {
            //     region_id,
            //     peer_id,
            //     merge_from_snapshot,
            // } => {
            //     assert_eq!(peer_id, self.fsm.peer.peer_id());
            //     if !merge_from_snapshot {
            //         self.destroy_peer(false);
            //     } else {
            //         // Wait for its target peer to apply snapshot and then send `MergeResult` back
            //         // to destroy itself
            //         let mut meta = self.ctx.store_meta.lock().unwrap();
            //         // The `need_atomic` flag must be true
            //         assert!(*meta.destroyed_region_for_snap.get(&region_id).unwrap());
            //
            //         let target_region_id = *meta.targets_map.get(&region_id).unwrap();
            //         let is_ready = meta
            //             .atomic_snap_regions
            //             .get_mut(&target_region_id)
            //             .unwrap()
            //             .get_mut(&region_id)
            //             .unwrap();
            //         *is_ready = true;
            //     }
            // }
        }
    }

    fn on_raft_message(&mut self, msg: InspectedRaftMessage) -> Result<()> {
        let InspectedRaftMessage { heap_size, mut msg } = msg;
        // let stepped = Cell::new(false);
        // let memtrace_raft_entries = &mut self.fsm.peer.memtrace_raft_entries as *mut usize;
        // defer!({
        //     MEMTRACE_RAFT_MESSAGES.trace(TraceEvent::Sub(heap_size));
        //     if stepped.get() {
        //         unsafe {
        //             // It could be less than exact for entry overwritting.
        //             *memtrace_raft_entries += heap_size;
        //             MEMTRACE_RAFT_ENTRIES.trace(TraceEvent::Add(heap_size));
        //         }
        //     }
        // });

        debug!(
            "handle raft message";
            "region_id" => self.region_id(),
            "peer_id" => self.fsm.peer_id(),
            "message_type" => %util::MsgType(&msg),
            "from_peer_id" => msg.get_from_peer().get_id(),
            "to_peer_id" => msg.get_to_peer().get_id(),
        );

        let msg_type = msg.get_message().get_msg_type();
        // let store_id = self.ctx.store_id();

        // if disk::disk_full_precheck(store_id) || self.ctx.is_disk_full {
        //     let mut flag = false;
        //     if MessageType::MsgAppend == msg_type {
        //         let entries = msg.get_message().get_entries();
        //         for i in entries {
        //             let entry_type = i.get_entry_type();
        //             if EntryType::EntryNormal == entry_type && !i.get_data().is_empty() {
        //                 flag = true;
        //                 break;
        //             }
        //         }
        //     } else if MessageType::MsgTimeoutNow == msg_type {
        //         flag = true;
        //     }
        //
        //     if flag {
        //         debug!(
        //             "skip {:?} because of disk full", msg_type;
        //             "region_id" => self.region_id(), "peer_id" => self.fsm.peer_id()
        //         );
        //         return Err(Error::Timeout("disk full".to_owned()));
        //     }
        // }

        if !self.validate_raft_msg(&msg) {
            return Ok(());
        }
        // if self.fsm.peer.pending_remove || self.fsm.stopped {
        //     return Ok(());
        // }

        // if msg.get_is_tombstone() {
        //     // we receive a message tells us to remove ourself.
        //     self.handle_gc_peer_msg(&msg);
        //     return Ok(());
        // }

        // if msg.has_merge_target() {
        //     fail_point!("on_has_merge_target", |_| Ok(()));
        //     if self.need_gc_merge(&msg)? {
        //         self.on_stale_merge(msg.get_merge_target().get_id());
        //     }
        //     return Ok(());
        // }

        // if self.check_msg(&msg) {
        //     return Ok(());
        // }

        // if msg.has_extra_msg() {
        //     self.on_extra_message(msg);
        //     return Ok(());
        // }

        // let is_snapshot = msg.get_message().has_snapshot();
        // let regions_to_destroy = match self.check_snapshot(&msg)? {
        //     Either::Left(key) => {
        //         // If the snapshot file is not used again, then it's OK to
        //         // delete them here. If the snapshot file will be reused when
        //         // receiving, then it will fail to pass the check again, so
        //         // missing snapshot files should not be noticed.
        //         let s = self.ctx.snap_mgr.get_snapshot_for_applying(&key)?;
        //         self.ctx.snap_mgr.delete_snapshot(&key, s.as_ref(), false);
        //         return Ok(());
        //     }
        //     Either::Right(v) => v,
        // };

        // if !self.check_request_snapshot(&msg) {
        //     return Ok(());
        // }

        // if util::is_vote_msg(&msg.get_message())
        //     || msg.get_message().get_msg_type() == MessageType::MsgTimeoutNow
        // {
        //     if self.fsm.hibernate_state.group_state() != GroupState::Chaos {
        //         self.fsm.reset_hibernate_state(GroupState::Chaos);
        //         self.register_raft_base_tick();
        //     }
        // } else if msg.get_from_peer().get_id() == self.fsm.peer.leader_id() {
        //     self.reset_raft_tick(GroupState::Ordered);
        // }

        let from_peer_id = msg.get_from_peer().get_id();
        self.fsm.peer.insert_peer_cache(msg.take_from_peer());

        let result = self.fsm.peer.step(self.ctx, msg.take_message());
        // stepped.set(result.is_ok());

        // if is_snapshot {
        //     if !self.fsm.peer.has_pending_snapshot() {
        //         // This snapshot is rejected by raft-rs.
        //         let mut meta = self.ctx.store_meta.lock().unwrap();
        //         meta.pending_snapshot_regions
        //             .retain(|r| self.fsm.region_id() != r.get_id());
        //     } else {
        //         // This snapshot may be accepted by raft-rs.
        //         // If it's rejected by raft-rs, the snapshot region in `pending_snapshot_regions`
        //         // will be removed together with the latest snapshot region after applying that snapshot.
        //         // But if `regions_to_destroy` is not empty, the pending snapshot must be this msg's snapshot
        //         // because this kind of snapshot is exclusive.
        //         self.destroy_regions_for_snapshot(regions_to_destroy);
        //     }
        // }

        if result.is_err() {
            return result;
        }

        // if self.fsm.peer.any_new_peer_catch_up(from_peer_id) {
        //     self.fsm.peer.heartbeat_pd(self.ctx);
        //     self.fsm.peer.should_wake_up = true;
        // }

        // if self.fsm.peer.should_wake_up {
        //     self.reset_raft_tick(GroupState::Ordered);
        // }

        self.fsm.has_ready = true;
        Ok(())
    }

    fn all_agree_to_hibernate(&mut self) -> bool {
        unreachable!()
    }

    fn on_hibernate_request(&mut self, from: &metapb::Peer) {
        unreachable!()
    }

    fn on_hibernate_response(&mut self, from: &metapb::Peer) {
        unreachable!()
    }

    fn on_extra_message(&mut self, mut msg: RaftMessage) {
        unreachable!()
    }

    // fn reset_raft_tick(&mut self, state: GroupState) {
    fn reset_raft_tick(&mut self,) {
        unreachable!()
        // self.fsm.reset_hibernate_state(state);
        // self.fsm.missing_ticks = 0;
        // self.fsm.peer.should_wake_up = false;
        // self.register_raft_base_tick();
    }

    // return false means the message is invalid, and can be ignored.
    fn validate_raft_msg(&mut self, msg: &RaftMessage) -> bool {
        let region_id = msg.get_region_id();
        let to = msg.get_to_peer();

        if to.get_store_id() != self.store_id() {
            warn!(
                "store not match, ignore it";
                "region_id" => region_id,
                "to_store_id" => to.get_store_id(),
                "my_store_id" => self.store_id(),
            );
            // self.ctx.raft_metrics.message_dropped.mismatch_store_id += 1;
            return false;
        }

        // if !msg.has_region_epoch() {
        //     error!(
        //         "missing epoch in raft message, ignore it";
        //         "region_id" => region_id,
        //     );
        //     self.ctx.raft_metrics.message_dropped.mismatch_region_epoch += 1;
        //     return false;
        // }

        true
    }

    /// Checks if the message is sent to the correct peer.
    ///
    /// Returns true means that the message can be dropped silently.
    fn check_msg(&mut self, msg: &RaftMessage) -> bool {
        unreachable!()
    }

    /// Check if it's necessary to gc the source merge peer.
    ///
    /// If the target merge peer won't be created on this store,
    /// then it's appropriate to destroy it immediately.
    fn need_gc_merge(&mut self, msg: &RaftMessage) -> Result<bool> {
        unreachable!()
    }

    fn handle_gc_peer_msg(&mut self, msg: &RaftMessage) {
        unreachable!()
    }

    // // Returns `Vec<(u64, bool)>` indicated (source_region_id, merge_to_this_peer) if the `msg`
    // // doesn't contain a snapshot or this snapshot doesn't conflict with any other snapshots or regions.
    // // Otherwise a `SnapKey` is returned.
    // fn check_snapshot(&mut self, msg: &RaftMessage) -> Result<Either<SnapKey, Vec<(u64, bool)>>> {
    //     unreachable!()
    // }

    fn destroy_regions_for_snapshot(&mut self, regions_to_destroy: Vec<(u64, bool)>) {
        unreachable!()
    }

    // Check if this peer can handle request_snapshot.
    fn check_request_snapshot(&mut self, msg: &RaftMessage) -> bool {
        unreachable!()
    }

    // fn handle_destroy_peer(&mut self, job: DestroyPeerJob) -> bool {
    fn handle_destroy_peer(&mut self) -> bool {
        unreachable!()
    }

    fn destroy_peer(&mut self, merged_by_target: bool) {
        unreachable!()
    }

    // Update some region infos
    fn update_region(&mut self, mut region: metapb::Region) {
        unreachable!()
    }

    // fn on_ready_change_peer(&mut self, cp: ChangePeer) {
    //     unreachable!()
    // }

    fn on_ready_compact_log(&mut self, first_index: u64, state: RaftTruncatedState) {
        // let total_cnt = self.fsm.peer.last_applying_idx - first_index;
        // // the size of current CompactLog command can be ignored.
        // let remain_cnt = self.fsm.peer.last_applying_idx - state.get_index() - 1;
        // self.fsm.peer.raft_log_size_hint =
        //     self.fsm.peer.raft_log_size_hint * remain_cnt / total_cnt;
        let compact_to = state.get_index() + 1;
        let task = RaftlogGcTask::gc(
            self.fsm.peer.get_store().get_region_id(),
            self.fsm.peer.last_compacted_idx,
            compact_to,
        );
        self.fsm.peer.last_compacted_idx = compact_to;
        // self.fsm.peer.mut_store().compact_to(compact_to);
        if let Err(e) = self.ctx.raftlog_gc_scheduler.schedule(task) {
            error!(
                "failed to schedule compact task";
                "region_id" => self.fsm.region_id(),
                "peer_id" => self.fsm.peer_id(),
                "err" => %e,
            );
        }
    }

    fn on_ready_split_region(
        &mut self,
        // derived: metapb::Region,
        // regions: Vec<metapb::Region>,
        // new_split_regions: HashMap<u64, apply::NewSplitPeer>,
    ) {
        unreachable!()
    }

    fn register_merge_check_tick(&mut self) {
        unreachable!()
        // self.schedule_tick(PeerTicks::CHECK_MERGE)
    }

    /// Check if merge target region is staler than the local one in kv engine.
    /// It should be called when target region is not in region map in memory.
    /// If everything is ok, the answer should always be true because PD should ensure all target peers exist.
    /// So if not, error log will be printed and return false.
    fn is_merge_target_region_stale(&self, target_region: &metapb::Region) -> Result<bool> {
        unreachable!()
    }

    fn validate_merge_peer(&self, target_region: &metapb::Region) -> Result<bool> {
        unreachable!()
    }

    fn schedule_merge(&mut self) -> Result<()> {
        unreachable!()
    }

    fn rollback_merge(&mut self) {
        unreachable!()
    }

    fn on_check_merge(&mut self) {
        unreachable!()
    }

    // fn on_ready_prepare_merge(&mut self, region: metapb::Region, state: MergeState) {
    fn on_ready_prepare_merge(&mut self) {
        unreachable!()
    }

    // fn on_catch_up_logs_for_merge(&mut self, mut catch_up_logs: CatchUpLogs) {
    fn on_catch_up_logs_for_merge(&mut self,) {
        unreachable!()
    }

    fn on_ready_commit_merge(
        &mut self,
        // merge_index: u64,
        // region: metapb::Region,
        // source: metapb::Region,
    ) {
        unreachable!()
    }

    /// Handle rollbacking Merge result.
    ///
    /// If commit is 0, it means that Merge is rollbacked by a snapshot; otherwise
    /// it's rollbacked by a proposal, and its value should be equal to the commit
    /// index of previous PrepareMerge.
    fn on_ready_rollback_merge(&mut self, commit: u64, region: Option<metapb::Region>) {
        unreachable!()
    }

    fn on_merge_result(
        &mut self,
        target_region_id: u64,
        target: metapb::Peer,
        // result: MergeResultKind,
    ) {
        unreachable!()
    }

    fn on_stale_merge(&mut self, target_region_id: u64) {
        unreachable!()
    }

    // fn on_ready_apply_snapshot(&mut self, apply_result: ApplySnapResult) {
    fn on_ready_apply_snapshot(&mut self) {
        unreachable!()
    }

    fn on_ready_result(
        &mut self,
        // exec_results: &mut VecDeque<ExecResult<EK::Snapshot>>,
        exec_results: &mut VecDeque<ExecResult>,
        // metrics: &ApplyMetrics,
    ) {
        // handle executing committed log results
        while let Some(result) = exec_results.pop_front() {
            match result {
                // ExecResult::ChangePeer(cp) => self.on_ready_change_peer(cp),
                ExecResult::CompactLog { first_index, state } => {
                    self.on_ready_compact_log(first_index, state)
                }
                // ExecResult::SplitRegion {
                //     derived,
                //     regions,
                //     new_split_regions,
                // } => self.on_ready_split_region(derived, regions, new_split_regions),
                // ExecResult::PrepareMerge { region, state } => {
                //     self.on_ready_prepare_merge(region, state)
                // }
                // ExecResult::CommitMerge {
                //     index,
                //     region,
                //     source,
                // } => self.on_ready_commit_merge(index, region, source),
                // ExecResult::RollbackMerge { region, commit } => {
                //     self.on_ready_rollback_merge(commit, Some(region))
                // }
                // ExecResult::ComputeHash {
                //     region,
                //     index,
                //     context,
                //     snap,
                // } => self.on_ready_compute_hash(region, index, context, snap),
                // ExecResult::VerifyHash {
                //     index,
                //     context,
                //     hash,
                // } => self.on_ready_verify_hash(index, context, hash),
                // ExecResult::DeleteRange { .. } => {
                //     // TODO: clean user properties?
                // }
                // ExecResult::IngestSst { ssts } => self.on_ingest_sst_result(ssts),
            }
        }

        // // Update metrics only when all exec_results are finished in case the metrics is counted multiple times
        // // when waiting for commit merge
        // self.ctx.store_stat.lock_cf_bytes_written += metrics.lock_cf_written_bytes;
        // self.ctx.store_stat.engine_total_bytes_written += metrics.written_bytes;
        // self.ctx.store_stat.engine_total_keys_written += metrics.written_keys;
    }

    /// Check if a request is valid if it has valid prepare_merge/commit_merge proposal.
    fn check_merge_proposal(&self, msg: &mut RaftCmdRequest) -> Result<()> {
        unreachable!()
    }

    fn pre_propose_raft_command(
        &mut self,
        msg: &RaftCmdRequest,
    ) -> Result<Option<RaftCmdResponse>> {
        // Check store_id, make sure that the msg is dispatched to the right place.
        if let Err(e) = util::check_store_id(msg, self.store_id()) {
            // self.ctx.raft_metrics.invalid_proposal.mismatch_store_id += 1;
            return Err(e);
        }
        if msg.has_status_request() {
            // For status commands, we handle it here directly.
            let resp = self.execute_status_command(msg)?;
            return Ok(Some(resp));
        }

        // Check whether the store has the right peer to handle the request.
        let region_id = self.region_id();
        let leader_id = self.fsm.peer.leader_id();
        let request = msg.get_requests();

        // // ReadIndex can be processed on the replicas.
        // let is_read_index_request =
        //     request.len() == 1 && request[0].get_cmd_type() == CmdType::ReadIndex;
        // let mut read_only = true;
        // for r in msg.get_requests() {
        //     match r.get_cmd_type() {
        //         CmdType::Get | CmdType::Snap | CmdType::ReadIndex => (),
        //         _ => read_only = false,
        //     }
        // }
        // let allow_replica_read = read_only && msg.get_header().get_replica_read();
        // let flags = WriteBatchFlags::from_bits_check(msg.get_header().get_flags());
        // let allow_stale_read = read_only && flags.contains(WriteBatchFlags::STALE_READ);
        // if !self.fsm.peer.is_leader()
        //     && !is_read_index_request
        //     && !allow_replica_read
        //     && !allow_stale_read
        // {
        //     self.ctx.raft_metrics.invalid_proposal.not_leader += 1;
        //     let leader = self.fsm.peer.get_peer_from_cache(leader_id);
        //     self.fsm.reset_hibernate_state(GroupState::Chaos);
        //     self.register_raft_base_tick();
        //     return Err(Error::NotLeader(region_id, leader));
        // }
        // peer_id must be the same as peer's.
        if let Err(e) = util::check_peer_id(msg, self.fsm.peer.peer_id()) {
            // self.ctx.raft_metrics.invalid_proposal.mismatch_peer_id += 1;
            return Err(e);
        }
        // check whether the peer is initialized.
        if !self.fsm.peer.is_initialized() {
            unreachable!()
            // self.ctx
            //     .raft_metrics
            //     .invalid_proposal
            //     .region_not_initialized += 1;
            // return Err(Error::RegionNotInitialized(region_id));
        }
        // If the peer is applying snapshot, it may drop some sending messages, that could
        // make clients wait for response until timeout.
        if self.fsm.peer.is_applying_snapshot() {
            unreachable!()
            // self.ctx.raft_metrics.invalid_proposal.is_applying_snapshot += 1;
            // // TODO: replace to a more suitable error.
            // return Err(Error::Other(box_err!(
            //     "{} peer is applying snapshot",
            //     self.fsm.peer.tag
            // )));
        }
        // Check whether the term is stale.
        if let Err(e) = util::check_term(msg, self.fsm.peer.term()) {
            // self.ctx.raft_metrics.invalid_proposal.stale_command += 1;
            return Err(e);
        }

        // match util::check_region_epoch(msg, self.fsm.peer.region(), true) {
        //     Err(Error::EpochNotMatch(m, mut new_regions)) => {
        //         // Attach the region which might be split from the current region. But it doesn't
        //         // matter if the region is not split from the current region. If the region meta
        //         // received by the TiKV driver is newer than the meta cached in the driver, the meta is
        //         // updated.
        //         let requested_version = msg.get_header().get_region_epoch().version;
        //         self.collect_sibling_region(requested_version, &mut new_regions);
        //         self.ctx.raft_metrics.invalid_proposal.epoch_not_match += 1;
        //         Err(Error::EpochNotMatch(m, new_regions))
        //     }
        //     Err(e) => Err(e),
        //     Ok(()) => Ok(None),
        // }
        Ok(None)
    }

    fn propose_raft_command(&mut self, mut msg: RaftCmdRequest, cb: Callback) {
        match self.pre_propose_raft_command(&msg) {
            Ok(Some(resp)) => {
                cb.invoke_with_response(resp);
                return;
            }
            Err(e) => {
                debug!(
                    "failed to propose";
                    "region_id" => self.region_id(),
                    "peer_id" => self.fsm.peer_id(),
                    "message" => ?msg,
                    "err" => %e,
                );
                cb.invoke_with_response(new_error(e));
                return;
            }
            _ => (),
        }

        // if self.fsm.peer.pending_remove {
        //     apply::notify_req_region_removed(self.region_id(), cb);
        //     return;
        // }

        // if let Err(e) = self.check_merge_proposal(&mut msg) {
        //     warn!(
        //         "failed to propose merge";
        //         "region_id" => self.region_id(),
        //         "peer_id" => self.fsm.peer_id(),
        //         "message" => ?msg,
        //         "err" => %e,
        //         "error_code" => %e.error_code(),
        //     );
        //     cb.invoke_with_response(new_error(e));
        //     return;
        // }

        // Note:
        // The peer that is being checked is a leader. It might step down to be a follower later. It
        // doesn't matter whether the peer is a leader or not. If it's not a leader, the proposing
        // command log entry can't be committed.

        let mut resp = RaftCmdResponse::default();
        let term = self.fsm.peer.term();
        bind_term(&mut resp, term);
        if self.fsm.peer.propose(self.ctx, cb, msg, resp) {
            self.fsm.has_ready = true;
        }

        // if self.fsm.peer.should_wake_up {
        //     self.reset_raft_tick(GroupState::Ordered);
        // }

        // self.register_pd_heartbeat_tick();

        // TODO: add timeout, if the command is not applied after timeout,
        // we will call the callback with timeout error.
    }

    fn collect_sibling_region(&self, requested_version: u64, regions: &mut Vec<Region>) {
        unreachable!()
    }

    fn register_raft_gc_log_tick(&mut self) {
        self.schedule_tick(PeerTicks::RAFT_LOG_GC)
    }

    #[allow(clippy::if_same_then_else)]
    fn on_raft_gc_log_tick(&mut self, _force_compact: bool) {
        // if !self.fsm.peer.get_store().is_cache_empty() || !self.ctx.cfg.hibernate_regions {
            self.register_raft_gc_log_tick();
        // }
        // fail_point!("on_raft_log_gc_tick_1", self.fsm.peer_id() == 1, |_| {});
        // fail_point!("on_raft_gc_log_tick", |_| {});
        // debug_assert!(!self.fsm.stopped);

        // The most simple case: compact log and cache to applied index directly.
        let applied_idx = self.fsm.peer.get_store().applied_index();
        if !self.fsm.peer.is_leader() {
            // self.fsm.peer.mut_store().compact_to(applied_idx + 1);
            return;
        }

        // // As leader, we would not keep caches for the peers that didn't response heartbeat in the
        // // last few seconds. That happens probably because another TiKV is down. In this case if we
        // // do not clean up the cache, it may keep growing.
        // let drop_cache_duration =
        //     self.ctx.cfg.raft_heartbeat_interval() + self.ctx.cfg.raft_entry_cache_life_time.0;
        // let cache_alive_limit = Instant::now() - drop_cache_duration;

        // Leader will replicate the compact log command to followers,
        // If we use current replicated_index (like 10) as the compact index,
        // when we replicate this log, the newest replicated_index will be 11,
        // but we only compact the log to 10, not 11, at that time,
        // the first index is 10, and replicated_index is 11, with an extra log,
        // and we will do compact again with compact index 11, in cycles...
        // So we introduce a threshold, if replicated index - first index > threshold,
        // we will try to compact log.
        // raft log entries[..............................................]
        //                  ^                                       ^
        //                  |-----------------threshold------------ |
        //              first_index                         replicated_index
        // `alive_cache_idx` is the smallest `replicated_index` of healthy up nodes.
        // `alive_cache_idx` is only used to gc cache.
        // let truncated_idx = self.fsm.peer.get_store().truncated_index();
        let last_idx = self.fsm.peer.get_store().last_index();
        // let (mut replicated_idx, mut alive_cache_idx) = (last_idx, last_idx);
        let mut replicated_idx = last_idx;
        for (peer_id, p) in self.fsm.peer.raft_group.raft.prs().iter() {
            if replicated_idx > p.matched {
                replicated_idx = p.matched;
            }
            // if let Some(last_heartbeat) = self.fsm.peer.peer_heartbeats.get(peer_id) {
            //     if alive_cache_idx > p.matched
            //         && p.matched >= truncated_idx
            //         && *last_heartbeat > cache_alive_limit
            //     {
            //         alive_cache_idx = p.matched;
            //     }
            // }
        }
        // When an election happened or a new peer is added, replicated_idx can be 0.
        if replicated_idx > 0 {
            assert!(
                last_idx >= replicated_idx,
                "expect last index {} >= replicated index {}",
                last_idx,
                replicated_idx
            );
            // REGION_MAX_LOG_LAG.observe((last_idx - replicated_idx) as f64);
        }
        // self.fsm
        //     .peer
        //     .mut_store()
        //     .maybe_gc_cache(alive_cache_idx, applied_idx);
        // if needs_evict_entry_cache(self.ctx.cfg.evict_cache_on_memory_ratio) {
        //     self.fsm.peer.mut_store().evict_cache(true);
        //     if !self.fsm.peer.get_store().cache_is_empty() {
        //         self.register_entry_cache_evict_tick();
        //     }
        // }

        let mut total_gc_logs = 0;

        let first_idx = self.fsm.peer.get_store().first_index();

        // let mut compact_idx = if force_compact
        //     // Too many logs between applied index and first index.
        //     || (applied_idx > first_idx && applied_idx - first_idx >= self.ctx.cfg.raft_log_gc_count_limit)
        //     // Raft log size ecceeds the limit.
        //     || (self.fsm.peer.raft_log_size_hint >= self.ctx.cfg.raft_log_gc_size_limit.0)
        // {
        //     applied_idx
        // } else if replicated_idx < first_idx || last_idx - first_idx < 3 {
        //     // In the current implementation one compaction can't delete all stale Raft logs.
        //     // There will be at least 3 entries left after one compaction:
        //     // |------------- entries needs to be compacted ----------|
        //     // [entries...][the entry at `compact_idx`][the last entry][new compaction entry]
        //     //             |-------------------- entries will be left ----------------------|
        //     return;
        // } else if replicated_idx - first_idx < self.ctx.cfg.raft_log_gc_threshold
        //     && self.fsm.skip_gc_raft_log_ticks < self.ctx.cfg.raft_log_reserve_max_ticks
        // {
        //     // Logs will only be kept `max_ticks` * `raft_log_gc_tick_interval`.
        //     self.fsm.skip_gc_raft_log_ticks += 1;
        //     self.register_raft_gc_log_tick();
        //     return;
        // } else {
        //     replicated_idx
        // };
        // assert!(compact_idx >= first_idx);
        // // Have no idea why subtract 1 here, but original code did this by magic.
        // compact_idx -= 1;
        // if compact_idx < first_idx {
        //     // In case compact_idx == first_idx before subtraction.
        //     return;
        // }

        // Picked up from L4009, currently don't need the condition ("last_idx - first_idx < 3") since
        // the current log compaction strategy is not that aggressive
        if replicated_idx < first_idx {
            warn!(
                "Replicated index {} < first index {}, this might happen when a new leader is elected",
                replicated_idx,
                first_idx,
            );
            return;
        }

        let raft_log_gc_count_limit = std::env::var("raft_log_gc_count_limit")
            .unwrap_or("50000000".to_string())
            .parse::<u64>()
            .unwrap();

        debug!("on_raft_gc_log_tick, first_idx {}, replicated_idx {}, applied_idx {}, last_idx {}, raft_log_gc_count_limit {}.",
            first_idx, replicated_idx, applied_idx, last_idx, raft_log_gc_count_limit
        );

        if replicated_idx - first_idx < raft_log_gc_count_limit {
            return;
        }
        let compact_idx = first_idx + raft_log_gc_count_limit / 10;

        info!("propose a log compact command, first_idx {}, replicated_idx {}, compact_idx {}, raft_log_gc_count_limit {}",
            first_idx, replicated_idx, compact_idx, raft_log_gc_count_limit
        );

        total_gc_logs += compact_idx - first_idx;

        // Create a compact log request and notify directly.
        let region_id = self.fsm.peer.region().get_id();
        let peer = self.fsm.peer.peer.clone();
        let term = self.fsm.peer.get_index_term(compact_idx);
        let request = new_compact_log_request(region_id, peer, compact_idx, term);
        // self.propose_raft_command(request, Callback::None, DiskFullOpt::AllowedOnAlmostFull);
        self.propose_raft_command(request, Callback::None);

        // self.fsm.skip_gc_raft_log_ticks = 0;
        self.register_raft_gc_log_tick();
        // PEER_GC_RAFT_LOG_COUNTER.inc_by(total_gc_logs);
    }

    fn register_entry_cache_evict_tick(&mut self) {
        unreachable!()
        // self.schedule_tick(PeerTicks::ENTRY_CACHE_EVICT)
    }

    fn on_entry_cache_evict_tick(&mut self) {
        unreachable!()
    }

    fn register_split_region_check_tick(&mut self) {
        unreachable!()
        // self.schedule_tick(PeerTicks::SPLIT_REGION_CHECK)
    }

    #[inline]
    fn region_split_skip_max_count(&self) -> usize {
        unreachable!()
        // fail_point!("region_split_skip_max_count", |_| { usize::max_value() });
        // REGION_SPLIT_SKIP_MAX_COUNT
    }

    fn on_split_region_check_tick(&mut self) {
        unreachable!()
    }

    fn on_prepare_split_region(
        &mut self,
        // region_epoch: metapb::RegionEpoch,
        // split_keys: Vec<Vec<u8>>,
        // cb: Callback<EK::Snapshot>,
        // source: &str,
    ) {
        unreachable!()
    }

    fn validate_split_region(
        &mut self,
        epoch: &metapb::RegionEpoch,
        split_keys: &[Vec<u8>],
    ) -> Result<()> {
        unreachable!()
    }

    fn on_approximate_region_size(&mut self, size: u64) {
        unreachable!()
    }

    fn on_approximate_region_keys(&mut self, keys: u64) {
        unreachable!()
    }

    fn on_compaction_declined_bytes(&mut self, declined_bytes: u64) {
        unreachable!()
    }

    // fn on_schedule_half_split_region(
    //     &mut self,
    //     region_epoch: &metapb::RegionEpoch,
    //     policy: CheckPolicy,
    //     source: &str,
    // ) {
    //     unreachable!()
    // }

    fn on_pd_heartbeat_tick(&mut self) {
        unreachable!()
    }

    fn register_pd_heartbeat_tick(&mut self) {
        unreachable!()
        // self.schedule_tick(PeerTicks::PD_HEARTBEAT)
    }

    fn on_check_peer_stale_state_tick(&mut self) {
        unreachable!()
    }

    fn register_check_peer_stale_state_tick(&mut self) {
        unreachable!()
        // self.schedule_tick(PeerTicks::CHECK_PEER_STALE_STATE)
    }
}

impl<'a> PeerFsmDelegate<'a>
{
    fn on_ready_compute_hash(
        &mut self,
        // region: metapb::Region,
        // index: u64,
        // context: Vec<u8>,
        // snap: EK::Snapshot,
    ) {
        unreachable!()
    }

    fn on_ready_verify_hash(
        &mut self,
        expected_index: u64,
        context: Vec<u8>,
        expected_hash: Vec<u8>,
    ) {
        unreachable!()
        // self.verify_and_store_hash(expected_index, context, expected_hash);
    }

    fn on_hash_computed(&mut self, index: u64, context: Vec<u8>, hash: Vec<u8>) {
        unreachable!()
    }

    // fn on_ingest_sst_result(&mut self, ssts: Vec<SSTMetaInfo>) {
    //     unreachable!()
    // }

    /// Verify and store the hash to state. return true means the hash has been stored successfully.
    // TODO: Consider context in the function.
    fn verify_and_store_hash(
        &mut self,
        expected_index: u64,
        _context: Vec<u8>,
        expected_hash: Vec<u8>,
    ) -> bool {
        unreachable!()
    }
}

/// Checks merge target, returns whether the source peer should be destroyed and whether the source peer is
/// merged to this target peer.
///
/// It returns (`can_destroy`, `merge_to_this_peer`).
///
/// `can_destroy` is true when there is a network isolation which leads to a follower of a merge target
/// Region's log falls behind and then receive a snapshot with epoch version after merge.
///
/// `merge_to_this_peer` is true when `can_destroy` is true and the source peer is merged to this target peer.
pub fn maybe_destroy_source(
    // meta: &StoreMeta,
    // target_region_id: u64,
    // target_peer_id: u64,
    // source_region_id: u64,
    // region_epoch: RegionEpoch,
) -> (bool, bool) {
    unreachable!()
}

pub fn new_read_index_request(
    region_id: u64,
    // region_epoch: RegionEpoch,
    peer: metapb::Peer,
) -> RaftCmdRequest {
    unreachable!()
    // let mut request = RaftCmdRequest::default();
    // request.mut_header().set_region_id(region_id);
    // request.mut_header().set_region_epoch(region_epoch);
    // request.mut_header().set_peer(peer);
    // let mut cmd = Request::default();
    // cmd.set_cmd_type(CmdType::ReadIndex);
    // request
}

pub fn new_admin_request(region_id: u64, peer: metapb::Peer) -> RaftCmdRequest {
    let mut request = RaftCmdRequest::default();
    request.mut_header().set_region_id(region_id);
    request.mut_header().set_peer(peer);
    request
}

fn new_verify_hash_request(
    region_id: u64,
    peer: metapb::Peer,
    // state: &ConsistencyState,
) -> RaftCmdRequest {
    unreachable!()
    // let mut request = new_admin_request(region_id, peer);
    //
    // let mut admin = AdminRequest::default();
    // admin.set_cmd_type(AdminCmdType::VerifyHash);
    // admin.mut_verify_hash().set_index(state.index);
    // admin.mut_verify_hash().set_context(state.context.clone());
    // admin.mut_verify_hash().set_hash(state.hash.clone());
    // request.set_admin_request(admin);
    // request
}

fn new_compact_log_request(
    region_id: u64,
    peer: metapb::Peer,
    compact_index: u64,
    compact_term: u64,
) -> RaftCmdRequest {
    let mut request = new_admin_request(region_id, peer);

    let mut admin = AdminRequest::default();
    admin.set_cmd_type(AdminCmdType::CompactLog);
    admin.mut_compact_log().set_compact_index(compact_index);
    admin.mut_compact_log().set_compact_term(compact_term);
    request.set_admin_request(admin);
    request
}

impl<'a> PeerFsmDelegate<'a>
{
    // Handle status commands here, separate the logic, maybe we can move it
    // to another file later.
    // Unlike other commands (write or admin), status commands only show current
    // store status, so no need to handle it in raft group.
    fn execute_status_command(&mut self, request: &RaftCmdRequest) -> Result<RaftCmdResponse> {
        let cmd_type = request.get_status_request().get_cmd_type();

        let mut response = match cmd_type {
            StatusCmdType::RegionLeader => self.execute_region_leader(),
            StatusCmdType::RegionDetail => self.execute_region_detail(request),
            StatusCmdType::InvalidStatus => {
                Err(box_err!("{} invalid status command!", self.fsm.peer.tag))
            }
        }?;
        response.set_cmd_type(cmd_type);

        let mut resp = RaftCmdResponse::default();
        resp.set_status_response(response);
        // Bind peer current term here.
        bind_term(&mut resp, self.fsm.peer.term());
        Ok(resp)
    }

    fn execute_region_leader(&mut self) -> Result<StatusResponse> {
        let mut resp = StatusResponse::default();
        if let Some(leader) = self.fsm.peer.get_peer_from_cache(self.fsm.peer.leader_id()) {
            resp.mut_region_leader().set_leader(leader);
        }

        Ok(resp)
    }

    fn execute_region_detail(&mut self, request: &RaftCmdRequest) -> Result<StatusResponse> {
        if !self.fsm.peer.get_store().is_initialized() {
            let region_id = request.get_header().get_region_id();
            return Err(Error::RegionNotInitialized(region_id));
        }
        let mut resp = StatusResponse::default();
        resp.mut_region_detail()
            .set_region(self.fsm.peer.region().clone());
        if let Some(leader) = self.fsm.peer.get_peer_from_cache(self.fsm.peer.leader_id()) {
            resp.mut_region_detail().set_leader(leader);
        }

        Ok(resp)
    }
}

impl AbstractPeer for PeerFsm {
    fn meta_peer(&self) -> &metapb::Peer {
        &self.peer.peer
    }
    // fn group_state(&self) -> GroupState {
    //     self.hibernate_state.group_state()
    // }
    fn region(&self) -> &metapb::Region {
        self.peer.raft_group.store().region()
    }
    fn apply_state(&self) -> &RaftApplyState {
        self.peer.raft_group.store().apply_state()
    }
    fn raft_status(&self) -> raft::Status {
        self.peer.raft_group.status()
    }
    fn raft_commit_index(&self) -> u64 {
        self.peer.raft_group.store().commit_index()
    }
    fn raft_request_snapshot(&mut self, index: u64) {
        unreachable!()
        // self.peer.raft_group.request_snapshot(index).unwrap();
    }
    // fn pending_merge_state(&self) -> Option<&MergeState> {
    //     self.peer.pending_merge_state.as_ref()
    // }
}
