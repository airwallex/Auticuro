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

use kvproto::metapb::{self, Region};
use kvproto::raft_serverpb::{RaftApplyState, RaftLocalState};

use crate::rocks_engine::{Engines, RocksWriteBatch, CF_RAFT};
use crate::{keys, util};

use raft::eraftpb::{ConfState, Entry, HardState, Snapshot};
use raft::{self, RaftState, Ready, Storage};
use std::mem;
use std::ops::Range;
use std::sync::{Arc, Mutex};
use tikv_util::debug;

// When we create a region peer, we should initialize its log term/index > 0,
// so that we can force the follower peer to sync the snapshot first.
pub const RAFT_INIT_LOG_TERM: u64 = 5;
pub const RAFT_INIT_LOG_INDEX: u64 = 5;

/// The initial region epoch version.
pub const INIT_EPOCH_VER: u64 = 1;
/// The initial region epoch conf_version.
pub const INIT_EPOCH_CONF_VER: u64 = 1;

pub trait HandleRaftReadyContext {
    /// Returns the mutable references of WriteBatch for both KvDB and RaftDB in one interface.
    fn wb_mut(&mut self) -> (&mut RocksWriteBatch, &mut RocksWriteBatch);
    fn kv_wb_mut(&mut self) -> &mut RocksWriteBatch;
    fn raft_wb_mut(&mut self) -> &mut RocksWriteBatch;
    fn sync_log(&self) -> bool;
    fn set_sync_log(&mut self, sync: bool);
}

pub struct PeerStorage {
    pub engines: Engines,

    peer_id: u64,
    region: Region,
    raft_state: RaftLocalState,
    apply_state: RaftApplyState,
    applied_index_term: u64,
    last_term: u64,

    pub tag: String,
}

impl Storage for PeerStorage {
    fn initial_state(&self) -> raft::Result<RaftState> {
        self.initial_state()
    }

    fn entries(
        &self,
        low: u64,
        high: u64,
        max_size: impl Into<Option<u64>>,
    ) -> raft::Result<Vec<Entry>> {
        self.entries(low, high, max_size.into().unwrap_or(u64::MAX))
    }

    fn term(&self, idx: u64) -> raft::Result<u64> {
        self.term(idx)
    }

    fn first_index(&self) -> raft::Result<u64> {
        Ok(self.first_index())
    }

    fn last_index(&self) -> raft::Result<u64> {
        Ok(self.last_index())
    }

    fn snapshot(&self, _request_index: u64) -> raft::Result<Snapshot> {
        todo!()
    }
}

impl PeerStorage {
    pub fn new(
        engines: Engines,
        region: &metapb::Region,
        peer_id: u64,
        tag: String,
    ) -> PeerStorage {
        debug!(
            "creating storage on specified path";
            "region_id" => region.get_id(),
            "peer_id" => peer_id,
            "path" => ?engines.kv.path(),
        );

        let mut raft_state = init_raft_state(&engines, region);
        let apply_state = init_apply_state(&engines, region);
        validate_states(region.get_id(), &engines, &mut raft_state, &apply_state);

        let last_term = init_last_term(&engines, region, &raft_state, &apply_state);
        let applied_index_term = init_applied_index_term(&engines, region, &apply_state);

        PeerStorage {
            engines,
            peer_id,
            region: region.clone(),
            raft_state,
            apply_state,
            tag,
            applied_index_term,
            last_term,
        }
    }

    pub fn is_initialized(&self) -> bool {
        util::is_region_initialized(self.region())
    }

    fn check_range(&self, low: u64, high: u64) {
        if low > high {
            panic!("low > high");
        } else if low <= self.truncated_index() {
            panic!("low <= self.truncated_index()");
        } else if high > self.last_index() + 1 {
            panic!(
                "high > self.last_index() + 1, low {}, high {}, last_index {}",
                low,
                high,
                self.last_index()
            );
        }
    }

    pub fn initial_state(&self) -> raft::Result<RaftState> {
        let hard_state = self.raft_state.get_hard_state().clone();
        if hard_state == HardState::default() {
            return Ok(RaftState::new(hard_state, ConfState::default()));
        }
        Ok(RaftState::new(
            hard_state,
            util::conf_state_from_region(self.region()),
        ))
    }

    pub fn entries(&self, low: u64, high: u64, max_size: u64) -> raft::Result<Vec<Entry>> {
        self.check_range(low, high);
        let mut ents = Vec::with_capacity((high - low) as usize);
        if low == high {
            return Ok(ents);
        }

        let region_id = self.get_region_id();
        self.engines.raft.fetch_entries_to(
            region_id,
            low,
            high,
            Some(max_size as usize),
            &mut ents,
        );

        Ok(ents)
    }

    pub fn term(&self, idx: u64) -> raft::Result<u64> {
        if idx == self.truncated_index() {
            return Ok(self.truncated_term());
        }
        self.check_range(idx, idx + 1);
        if self.truncated_term() == self.last_term || idx == self.last_index() {
            return Ok(self.last_term);
        }
        let entries = self.entries(idx, idx + 1, raft::NO_LIMIT)?;
        Ok(entries[0].get_term())
    }

    #[inline]
    pub fn first_index(&self) -> u64 {
        first_index(&self.apply_state)
    }

    #[inline]
    pub fn last_index(&self) -> u64 {
        last_index(&self.raft_state)
    }

    #[inline]
    pub fn last_term(&self) -> u64 {
        self.last_term
    }

    #[inline]
    pub fn applied_index(&self) -> u64 {
        self.apply_state.get_applied_index()
    }

    #[inline]
    pub fn set_applied_state(&mut self, apply_state: RaftApplyState) {
        self.apply_state = apply_state;
    }

    #[inline]
    pub fn apply_state(&self) -> &RaftApplyState {
        &self.apply_state
    }

    #[inline]
    pub fn set_applied_term(&mut self, applied_index_term: u64) {
        self.applied_index_term = applied_index_term;
    }

    #[inline]
    pub fn applied_index_term(&self) -> u64 {
        self.applied_index_term
    }

    #[inline]
    pub fn commit_index(&self) -> u64 {
        self.raft_state.get_hard_state().get_commit()
    }

    #[inline]
    pub fn set_commit_index(&mut self, commit: u64) {
        assert!(commit >= self.commit_index());
        self.raft_state.mut_hard_state().set_commit(commit);
    }

    #[inline]
    pub fn hard_state(&self) -> &HardState {
        self.raft_state.get_hard_state()
    }

    #[inline]
    pub fn truncated_index(&self) -> u64 {
        self.apply_state.get_truncated_state().get_index()
    }

    #[inline]
    pub fn truncated_term(&self) -> u64 {
        self.apply_state.get_truncated_state().get_term()
    }

    #[inline]
    pub fn region(&self) -> &metapb::Region {
        &self.region
    }

    #[inline]
    pub fn set_region(&mut self, region: metapb::Region) {
        self.region = region;
    }

    #[inline]
    pub fn get_region_id(&self) -> u64 {
        self.region().get_id()
    }
}

/// Returned by `PeerStorage::handle_raft_ready`, used for recording changed status of
/// `RaftLocalState` and `RaftApplyState`.
pub struct InvokeContext {
    pub region_id: u64,
    /// Changed RaftLocalState is stored into `raft_state`.
    pub raft_state: RaftLocalState,
    /// Changed RaftApplyState is stored into `apply_state`.
    pub apply_state: RaftApplyState,
    last_term: u64,
    /// If the ready has new entries.
    pub has_new_entries: bool,
}

impl InvokeContext {
    pub fn new(store: &PeerStorage) -> InvokeContext {
        InvokeContext {
            region_id: store.get_region_id(),
            raft_state: store.raft_state.clone(),
            apply_state: store.apply_state.clone(),
            last_term: store.last_term,
            has_new_entries: false,
        }
    }

    #[inline]
    pub fn save_raft_state_to(&self, raft_wb: &mut RocksWriteBatch) {
        raft_wb.put_raft_state(self.region_id, &self.raft_state);
    }

    #[inline]
    pub fn save_apply_state_to(&self, kv_wb: &mut RocksWriteBatch) {
        kv_wb.put_msg_cf(
            CF_RAFT,
            &keys::apply_state_key(self.region_id),
            &self.apply_state,
        );
    }
}

impl PeerStorage {
    /// Save memory states to disk.
    ///
    /// This function only write data to `ready_ctx`'s `WriteBatch`. It's caller's duty to write
    /// it explicitly to disk. If it's flushed to disk successfully, `post_ready` should be called
    /// to update the memory states properly.
    /// WARNING: If this function returns error, the caller must panic(details in `append` function).
    pub fn handle_raft_ready<H: HandleRaftReadyContext>(
        &mut self,
        ready_ctx: &mut H,
        ready: &mut Ready,
    ) -> InvokeContext {
        let mut ctx = InvokeContext::new(self);
        // let snapshot_index = if ready.snapshot().is_empty() {
        //     0
        // } else {
        //     fail_point!("raft_before_apply_snap");
        //     let (kv_wb, raft_wb) = ready_ctx.wb_mut();
        //     self.apply_snapshot(&mut ctx, ready.snapshot(), kv_wb, raft_wb, &destroy_regions)?;
        //     fail_point!("raft_after_apply_snap");
        //
        //     ctx.destroyed_regions = destroy_regions;
        //
        //     last_index(&ctx.raft_state)
        // };

        // todo(glengeng): revisit the code above later
        let snapshot_index = 0;

        if !ready.entries().is_empty() {
            self.append(&mut ctx, ready.take_entries(), ready_ctx);
        }

        // Last index is 0 means the peer is created from raft message
        // and has not applied snapshot yet, so skip persistent hard state.
        if ctx.raft_state.get_last_index() > 0 {
            if let Some(hs) = ready.hs() {
                ctx.raft_state.set_hard_state(hs.clone());
            }
        }

        // Save raft state if it has changed or there is a snapshot.
        if ctx.raft_state != self.raft_state || snapshot_index > 0 {
            ctx.save_raft_state_to(ready_ctx.raft_wb_mut());
            // if snapshot_index > 0 {
            //     // in case of restart happen when we just write region state to Applying,
            //     // but not write raft_local_state to raft rocksdb in time.
            //     // we write raft state to default rocksdb, with last index set to snap index,
            //     // in case of recv raft log after snapshot.
            //     ctx.save_snapshot_raft_state_to(snapshot_index, ready_ctx.kv_wb_mut())?;
            // }
        }

        // // only when apply snapshot
        // if snapshot_index > 0 {
        //     ctx.save_apply_state_to(ready_ctx.kv_wb_mut())?;
        // }

        ctx
    }

    /// Update the memory state after ready changes are flushed to disk successfully.
    pub fn post_ready(&mut self, ctx: InvokeContext) {
        self.raft_state = ctx.raft_state;
        self.apply_state = ctx.apply_state;
        self.last_term = ctx.last_term;

        // todo(glengeng): revisit, since I removed quite a few snapshot related code
    }

    // Append the given entries to the raft log using previous last index or self.last_index.
    // Return the new last index for later update. After we commit in engine, we can set last_index
    // to the return one.
    // WARNING: If this function returns error, the caller must panic otherwise the entry cache may
    // be wrong and break correctness.
    pub fn append<H: HandleRaftReadyContext>(
        &mut self,
        invoke_ctx: &mut InvokeContext,
        entries: Vec<Entry>,
        ready_ctx: &mut H,
    ) -> u64 {
        let region_id = self.get_region_id();
        debug!(
            "append entries";
            "region_id" => region_id,
            "peer_id" => self.peer_id,
            "count" => entries.len(),
        );
        let prev_last_index = invoke_ctx.raft_state.get_last_index();
        if entries.is_empty() {
            return prev_last_index;
        }

        invoke_ctx.has_new_entries = true;

        let (last_index, last_term) = {
            let e = entries.last().unwrap();
            (e.get_index(), e.get_term())
        };

        ready_ctx.raft_wb_mut().append(region_id, entries);

        // Delete any previously appended log entries which never committed.
        // TODO: Wrap it as an engine::Error.
        ready_ctx
            .raft_wb_mut()
            .cut_logs(region_id, last_index + 1, prev_last_index + 1);

        invoke_ctx.raft_state.set_last_index(last_index);
        invoke_ctx.last_term = last_term;

        last_index
    }
}

#[inline]
pub fn first_index(state: &RaftApplyState) -> u64 {
    state.get_truncated_state().get_index() + 1
}

#[inline]
pub fn last_index(state: &RaftLocalState) -> u64 {
    state.get_last_index()
}

fn init_raft_state(engines: &Engines, region: &Region) -> RaftLocalState {
    if let Some(state) = engines.raft.get_raft_state(region.get_id()) {
        state
    } else {
        unreachable!()
    }
}

pub fn init_apply_state(engines: &Engines, region: &Region) -> RaftApplyState {
    if let Some(state) = engines
        .kv
        .get_msg_cf(CF_RAFT, &keys::apply_state_key(region.get_id()))
    {
        state
    } else {
        unreachable!()
    }
}

fn init_last_term(
    engines: &Engines,
    region: &Region,
    raft_state: &RaftLocalState,
    apply_state: &RaftApplyState,
) -> u64 {
    let last_idx = raft_state.get_last_index();
    if last_idx == 0 {
        return 0;
    } else if last_idx == RAFT_INIT_LOG_INDEX {
        return RAFT_INIT_LOG_TERM;
    } else if last_idx == apply_state.get_truncated_state().get_index() {
        return apply_state.get_truncated_state().get_term();
    } else {
        assert!(last_idx > RAFT_INIT_LOG_INDEX);
    }
    if let Some(entry) = engines.raft.get_entry(region.get_id(), last_idx) {
        entry.get_term()
    } else {
        panic!(
            "[region {}] entry at {} doesn't exist, may lose data.",
            region.get_id(),
            last_idx
        )
    }
}

fn init_applied_index_term(
    engines: &Engines,
    region: &Region,
    apply_state: &RaftApplyState,
) -> u64 {
    if apply_state.applied_index == RAFT_INIT_LOG_INDEX {
        return RAFT_INIT_LOG_TERM;
    }
    let truncated_state = apply_state.get_truncated_state();
    if apply_state.applied_index == truncated_state.get_index() {
        return truncated_state.get_term();
    }

    if let Some(entry) = engines
        .raft
        .get_entry(region.get_id(), apply_state.applied_index)
    {
        entry.get_term()
    } else {
        panic!(
            "[region {}] entry at apply index {} doesn't exist, may lose data.",
            region.get_id(),
            apply_state.applied_index
        )
    }
}

fn validate_states(
    region_id: u64,
    engines: &Engines,
    raft_state: &mut RaftLocalState,
    apply_state: &RaftApplyState,
) {
    let last_index = raft_state.get_last_index();
    let mut commit_index = raft_state.get_hard_state().get_commit();
    let recorded_commit_index = apply_state.get_commit_index();
    let state_str = || -> String {
        format!(
            "region {}, raft state {:?}, apply state {:?}",
            region_id, raft_state, apply_state
        )
    };
    // The commit index of raft state may be less than the recorded commit index.
    // If so, forward the commit index.
    if commit_index < recorded_commit_index {
        let entry = engines.raft.get_entry(region_id, recorded_commit_index);
        if entry.map_or(true, |e| e.get_term() != apply_state.get_commit_term()) {
            panic!(
                "log at recorded commit index [{}] {} doesn't exist, may lose data, {}",
                apply_state.get_commit_term(),
                recorded_commit_index,
                state_str()
            )
        }
        println!(
            "updating commit index, region_id {}, old {}, new {}",
            region_id, commit_index, recorded_commit_index
        );
        commit_index = recorded_commit_index;
    }
    // Invariant: applied index <= max(commit index, recorded commit index)
    if apply_state.get_applied_index() > commit_index {
        panic!(
            "applied index > max(commit index, recorded commit index), {}",
            state_str()
        );
    }
    // Invariant: max(commit index, recorded commit index) <= last index
    if commit_index > last_index {
        panic!(
            "max(commit index, recorded commit index) > last index, {}",
            state_str()
        );
    }
    // Since the entries must be persisted before applying, the term of raft state should also
    // be persisted. So it should be greater than the commit term of apply state.
    if raft_state.get_hard_state().get_term() < apply_state.get_commit_term() {
        panic!(
            "term of raft state < commit term of apply state, {}",
            state_str()
        );
    }

    raft_state.mut_hard_state().set_commit(commit_index);
}

// When we bootstrap the region we must call this to initialize region local state first.
pub fn write_initial_raft_state(raft_wb: &mut RocksWriteBatch, region_id: u64) {
    let mut raft_state = RaftLocalState {
        last_index: RAFT_INIT_LOG_INDEX,
        ..Default::default()
    };
    raft_state.mut_hard_state().set_term(RAFT_INIT_LOG_TERM);
    raft_state.mut_hard_state().set_commit(RAFT_INIT_LOG_INDEX);
    raft_wb.put_raft_state(region_id, &raft_state);
}

// When we bootstrap the region or handling split new region, we must
// call this to initialize region apply state first.
pub fn write_initial_apply_state(kv_wb: &mut RocksWriteBatch, region_id: u64) {
    let mut apply_state = RaftApplyState::default();
    apply_state.set_applied_index(RAFT_INIT_LOG_INDEX);
    apply_state
        .mut_truncated_state()
        .set_index(RAFT_INIT_LOG_INDEX);
    apply_state
        .mut_truncated_state()
        .set_term(RAFT_INIT_LOG_TERM);

    kv_wb.put_msg_cf(CF_RAFT, &keys::apply_state_key(region_id), &apply_state);
}

/// Committed entries sent to apply threads.
#[derive(Clone)]
pub struct CachedEntries {
    pub range: Range<u64>,
    // Entries and dangle size for them. `dangle` means not in entry cache.
    entries: Arc<Mutex<(Vec<Entry>, usize)>>,
}

impl CachedEntries {
    pub fn new(entries: Vec<Entry>) -> Self {
        assert!(!entries.is_empty());
        let start = entries.first().map(|x| x.index).unwrap();
        let end = entries.last().map(|x| x.index).unwrap() + 1;
        let range = Range { start, end };
        CachedEntries {
            entries: Arc::new(Mutex::new((entries, 0))),
            range,
        }
    }

    /// Take cached entries and dangle size for them. `dangle` means not in entry cache.
    pub fn take_entries(&self) -> (Vec<Entry>, usize) {
        mem::take(&mut *self.entries.lock().unwrap())
    }
}

#[cfg(test)]
mod tests {
    use tempfile::Builder;

    use super::*;
    use crate::bootstrap::{bootstrap_store, initial_region, prepare_bootstrap_cluster};
    use crate::rocks_engine::{self, RocksEngine, ALL_CFS};
    use std::sync::Arc;

    #[test]
    fn test_bootstrap() {
        let path = Builder::new().prefix("test_bootstrap").tempdir().unwrap();
        let raft_path = path.path().join("raft");


        let kv_db = rocks_engine::new_engine(path.path().to_str().unwrap(), ALL_CFS);
        let kv_engine = RocksEngine::from_db(Arc::new(kv_db));

        let raft_db = rocks_engine::new_engine(raft_path.as_path().to_str().unwrap(), ALL_CFS);
        let raft_engine = RocksEngine::from_db(Arc::new(raft_db));

        let engines = Engines::new(kv_engine.clone(), raft_engine.clone());

        let region = initial_region(1, &[(1, 1), (2, 2), (3, 3), (4, 4), (5, 5)]);

        assert!(prepare_bootstrap_cluster(&engines, &region).is_ok());

        assert!(bootstrap_store(&engines, 1, 1).is_ok());

        assert!(engines
            .kv
            .get_value_cf(CF_RAFT, &keys::apply_state_key(1))
            .is_some());

        assert!(engines.raft.get_value(&keys::raft_state_key(1)).is_some());
    }
}
