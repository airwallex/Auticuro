#![feature(map_first_last)]
#![feature(array_from_fn)]
#![feature(backtrace)]
extern crate core;

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
mod account;
mod balance_calculator;
mod config;
mod errors;
mod gc;
mod internal;
mod message_broker;
mod metrics;
mod rocksdb_state;
mod service;
mod start_up;
mod state_machine;
mod test;
mod utils;

use crate::config::{ClusterConfig, Config};
use start_up::start_wallet;
use tikv_util::info;

fn main() {
    let config = Config::default();

    let cluster_config: &ClusterConfig = &config.cluster_config;
    info!(
        "startup node, cluster_id: {}, cluster_size: {}, store_id: {}.",
        cluster_config.cluster_id, cluster_config.cluster_size, cluster_config.store_id
    );

    start_wallet(config);
}
