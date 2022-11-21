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
use crate::config::{Config, EventLogGCSetting};
use crate::gc::EventLogGCLoop;
use crate::message_broker::MessageBroker;
use crate::rocksdb_state::RocksDBState;
use crate::service::WalletService;
use crate::state_machine::{ApplyLoop, StateMachine};
use hologram_kv::message_queue_adapter::MessageQueueAdapter;
use hologram_kv::setup;
use hologram_kv::startup::{self, TiKVServer};
use hologram_protos::firm_walletpb::account_management_servicepb_grpc::create_account_management_service;
use hologram_protos::firm_walletpb::balance_operation_servicepb::*;
use hologram_protos::firm_walletpb::internal_servicepb::create_internal_service;
use std::sync::{mpsc, Arc, RwLock};
use std::thread;
use tikv_util::{error, info};

pub fn start_wallet(config: Config) {
    setup::initial_logger();

    // 1, init tikv
    let cluster_config = config.cluster_config;
    let mut tikv = TiKVServer::init(
        cluster_config.cluster_id,
        cluster_config.cluster_size,
        cluster_config.store_id,
    );
    tikv.init_engines();
    tikv.init_servers();

    // 2, create MessageQueueAdapter from tikv
    let region_id = 1; // hard code to 1, since it is a single raft cluster.
    let message_queue = Arc::new(MessageQueueAdapter::new(
        tikv.storage.as_ref().unwrap().clone(),
        tikv.router.clone(),
        region_id,
        tikv.store_id,
    ));

    // 2.1 Start the message broker
    let (inflight_event_tx, inflight_event_rx) = mpsc::channel();
    let (inflight_request_tx, inflight_request_rx) = mpsc::channel();
    let mut msg_broker = MessageBroker::new(inflight_request_rx, inflight_event_rx);
    thread::Builder::new()
        .name(format!("msg_broker"))
        .spawn(move || {
            msg_broker.run();
            panic!("msg_broker quit unexpectedly");
        })
        .unwrap();

    // 3, recover or create WalletStateMachine from disk
    let rocksdb_state = RocksDBState::init();
    let (state_machine, context) =
        StateMachine::recover_from_db(rocksdb_state.clone(), inflight_event_tx);
    let state_machine = Arc::new(RwLock::new(state_machine));

    // 4, start ApplyLoop
    let mut apply_loop = ApplyLoop::new(state_machine.clone(), context, message_queue.clone());
    thread::Builder::new()
        .name(format!("apply_loop_of_statemachine"))
        .spawn(move || {
            apply_loop.run();
            panic!("apply_loop_of_statemachine quit unexpectedly");
        })
        .unwrap();

    // maybe start EventLogGCLoop
    match config.event_log_gc_setting {
        EventLogGCSetting::Enabled(config) => {
            info!(
                "GC enabled! gc_poll_interval_millis={}, gc_count_limit={}, gc_batch_size={}, gc_percentage={}",
                config.poll_interval_millis,
                config.count_limit,
                config.batch_size,
                config.percentage
            );
            let mut gc_loop = EventLogGCLoop::new(rocksdb_state, config);
            thread::Builder::new()
                .name(format!("gc_loop_of_statemachine"))
                .spawn(move || {
                    gc_loop.run();
                    error!("gc_loop_of_statemachine quit unexpectedly");
                })
                .unwrap();
        }
        EventLogGCSetting::Disabled => {
            info!("GC is disabled. Please export enable_gc_of_event_log=true if need GC enabled");
        }
    }

    // 5, create WalletService
    let wallet_service = WalletService::new(
        message_queue.clone(),
        state_machine.clone(),
        config.wallet_service_config,
        inflight_request_tx,
    );
    tikv.register_service(create_balance_operation_service(wallet_service.clone()));
    tikv.register_service(create_internal_service(wallet_service.clone()));
    tikv.register_service(create_account_management_service(wallet_service));

    // 6, run tikv
    tikv.run_server();
    tikv.run_status_server();

    startup::wait_for_signal();
}
