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

use super::peer_storage::{
    write_initial_apply_state, write_initial_raft_state, INIT_EPOCH_CONF_VER, INIT_EPOCH_VER,
};
use super::util::new_peer;
use crate::errors::Result;

use kvproto::metapb;
use kvproto::raft_serverpb::{RaftLocalState, RegionLocalState, StoreIdent};
use tikv_util::{box_err};
use crate::rocks_engine::{Engines, CF_RAFT};
use crate::keys;

// pub fn initial_region(store_id: u64, region_id: u64, peer_id: u64) -> metapb::Region {
//     let mut region = metapb::Region::default();
//     region.set_id(region_id);
//     region.set_start_key(keys::EMPTY_KEY.to_vec());
//     region.set_end_key(keys::EMPTY_KEY.to_vec());
//     region.mut_region_epoch().set_version(INIT_EPOCH_VER);
//     region.mut_region_epoch().set_conf_ver(INIT_EPOCH_CONF_VER);
//     region.mut_peers().push(new_peer(store_id, peer_id));
//     region
// }

pub fn initial_region(region_id: u64, peers: &[(u64, u64)]) -> metapb::Region {
    let mut region = metapb::Region::default();
    region.set_id(region_id);
    region.set_start_key(keys::EMPTY_KEY.to_vec());
    region.set_end_key(keys::EMPTY_KEY.to_vec());
    region.mut_region_epoch().set_version(INIT_EPOCH_VER);
    region.mut_region_epoch().set_conf_ver(INIT_EPOCH_CONF_VER);
    // (store_id, peer_id) like (1,1), (2,2), (3,3), (4,4), (5,5)
    for (store_id, peer_id) in peers {
        region.mut_peers().push(new_peer(*store_id, *peer_id));
    }
    region
}

// Bootstrap the store, the DB for this store must be empty and has no data.
//
// FIXME: ER typaram should just be impl KvEngine, but RaftEngine doesn't support
// the `is_range_empty` query yet.
pub fn bootstrap_store(
    engines: &Engines,
    cluster_id: u64,
    store_id: u64,
) -> Result<()>
{
    let mut ident = StoreIdent::default();

    // if !is_range_empty(&engines.kv, CF_DEFAULT, keys::MIN_KEY, keys::MAX_KEY)? {
    //     return Err(box_err!("kv store is not empty and has already had data."));
    // }

    ident.set_cluster_id(cluster_id);
    ident.set_store_id(store_id);

    engines.kv.put_msg(keys::STORE_IDENT_KEY, &ident);
    engines.sync_kv();
    Ok(())
}

/// The first phase of bootstrap cluster
///
/// Write the first region meta and prepare state.
pub fn prepare_bootstrap_cluster(
    engines: &Engines,
    region: &metapb::Region,
) -> Result<()> {
    let mut state = RegionLocalState::default();
    state.set_region(region.clone());

    let mut wb = engines.kv.write_batch();
    // wb.put_msg(keys::PREPARE_BOOTSTRAP_KEY, region);
    wb.put_msg_cf(CF_RAFT, &keys::region_state_key(region.get_id()), &state);
    write_initial_apply_state(&mut wb, region.get_id());
    wb.write_opt(false);
    engines.sync_kv();

    let mut raft_wb = engines.raft.log_batch(1024);
    write_initial_raft_state(&mut raft_wb, region.get_id());
    engines.raft.consume(&mut raft_wb, true);
    Ok(())
}
