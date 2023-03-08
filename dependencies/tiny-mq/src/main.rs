// We use `default` method a lot to be support prost and rust-protobuf at the
// same time. And reassignment can be optimized by compiler.
#![allow(clippy::field_reassign_with_default)]
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

mod apply;
mod bootstrap;
mod cmd_resp;
mod errors;
mod keys;
mod kv;
mod metrics;
mod msg;
mod node;
mod peer;

mod peer_storage;
mod raft_client;
mod raftkv;
mod rocks_engine;
mod server;
mod setup;
mod startup;
mod status_server;
mod storage;
mod store;
mod util;
mod raftlog_gc;

use tikv_util::info;

fn main() {
    setup::initial_logger();

    let cluster_id = std::env::var("cluster_id").unwrap().parse::<u64>().unwrap();
    let cluster_size = std::env::var("cluster_size")
        .unwrap_or("5".to_string())
        .parse::<u64>()
        .unwrap();
    let store_id = std::env::var("store_id").unwrap().parse::<u64>().unwrap();

    info!(
        "startup node, cluster_id: {}, cluster_size: {}, store_id: {}.",
        cluster_id, cluster_size, store_id
    );

    let tikv = startup::run_tikv(cluster_id, cluster_size, store_id);
    startup::wait_for_signal();
}
