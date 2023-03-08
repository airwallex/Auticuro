#![allow(warnings)]

//! This module startups all the components of a TiKV server.
//!
//! It is responsible for reading from configs, starting up the various server components,
//! and handling errors (mostly by aborting and reporting to the user).
//!
//! The entry point is `run_tikv`.
//!
//! Components are often used to initialize other components, and/or must be explicitly stopped.
//! We keep these components in the `TiKVServer` struct.

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
    cmp,
    convert::TryFrom,
    env, fmt,
    fs::{self, File},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
    u64,
};

use crate::kv::MQService;
use crate::node::{create_raft_storage, Node};
use crate::raft_client::AddressMap;
use crate::raftkv::RaftKv;
use crate::rocks_engine::{self, Engines, RocksEngine, ALL_CFS};
use crate::server::{Server, GRPC_THREAD_PREFIX};
use crate::status_server::StatusServer;
use crate::storage::Storage;
use crate::store::{self, RaftRouter, RaftSystem};
use grpcio::{EnvBuilder, Environment};
use kvproto::mqpb::*;
use libc::c_int;
use nix::sys::signal::{SIGHUP, SIGINT, SIGTERM, SIGUSR1, SIGUSR2};
use signal::trap::Trap;
use tikv_util::{
    info, thd_name,
    worker::{Builder as WorkerBuilder, LazyWorker, Worker},
};

/// Run a TiKV server. Returns when the server is shutdown by the user, in which
/// case the server will be properly stopped.
pub fn run_tikv(cluster_id: u64, cluster_size: u64, store_id: u64) -> TiKVServer {
    let mut tikv = TiKVServer::init(cluster_id, cluster_size, store_id);

    tikv.init_engines();
    tikv.init_servers();

    tikv.register_service(create_mq(MQService::new(tikv.storage.clone().unwrap())));

    tikv.run_server();
    tikv.run_status_server();

    tikv
}

/// A complete TiKV server.
pub struct TiKVServer {
    pub cluster_id: u64,
    pub cluster_size: u64,
    pub store_id: u64,

    pub router: RaftRouter,
    pub storage: Option<Storage>,

    system: Option<RaftSystem>,
    resolver: AddressMap,
    engines: Option<TiKVEngines>,
    servers: Option<Servers>,
    status_server: Option<StatusServer>,
    env: Arc<Environment>,
    background_worker: Worker,
}

struct TiKVEngines {
    engines: Engines,
    engine: RaftKv,
}

struct Servers {
    server: Server,
    node: Node,
}

impl TiKVServer {
    pub fn init(cluster_id: u64, cluster_size: u64, store_id: u64) -> TiKVServer {
        let mut resolver = AddressMap::default();
        for i in 1..=cluster_size {
            resolver.insert(i, std::env::var(format!("peer_{}", i)).unwrap());
        }

        let env = Arc::new(
            EnvBuilder::new()
                // .cq_count(config.server.grpc_concurrency)
                .cq_count(30)
                .name_prefix(thd_name!(GRPC_THREAD_PREFIX))
                .build(),
        );

        let (router, system) = store::create_raft_system();

        let thread_count = 2;
        let background_worker = WorkerBuilder::new("background")
            .thread_count(thread_count)
            .create();

        TiKVServer {
            cluster_id,
            cluster_size,
            store_id,
            router,
            system: Some(system),
            resolver,
            engines: None,
            servers: None,
            status_server: None,
            env,
            storage: None,
            background_worker,
        }
    }

    pub fn init_engines(&mut self) {
        // Create raft engine.
        let raft_db_path = env::var("raft_db_path").unwrap();
        let raft_db_path = Path::new(&raft_db_path);
        let raft_db = rocks_engine::new_engine(raft_db_path.to_str().unwrap(), ALL_CFS);

        let db_path = env::var("db_path").unwrap();
        let db_path = Path::new(&db_path);
        let kv_db = rocks_engine::new_engine(db_path.to_str().unwrap(), ALL_CFS);

        let kv_engine = RocksEngine::from_db(Arc::new(kv_db));
        let raft_engine = RocksEngine::from_db(Arc::new(raft_db));

        let engines = Engines::new(kv_engine, raft_engine);
        let engine = RaftKv::new(self.router.clone(), engines.clone());

        self.engines = Some(TiKVEngines { engines, engine });
    }

    pub fn init_servers(&mut self) {
        let engines = self.engines.as_ref().unwrap();

        let storage = create_raft_storage(engines.engine.clone()).unwrap();
        // .unwrap_or_else(|e| fatal!("failed to create raft storage: {}", e));

        let mut node = Node::new(
            self.cluster_id,
            self.cluster_size,
            self.store_id,
            self.system.take().unwrap(),
            self.background_worker.clone(),
        );
        node.try_bootstrap_store(engines.engines.clone()).unwrap();

        // Create server
        let server = Server::new(
            node.id(),
            storage.clone(),
            self.resolver.clone(),
            self.router.clone(),
            self.env.clone(),
        )
        .unwrap();
        self.storage = Some(storage);

        node.start(engines.engines.clone(), server.raft_client.clone())
            .unwrap();

        assert!(node.id() > 0); // Node id should never be 0.

        self.servers = Some(Servers { server, node });
    }

    // This is where applications inject their own GrpcService.
    pub fn register_service(&mut self, svc: grpcio::Service) {
        assert!(self
            .servers
            .as_mut()
            .unwrap()
            .server
            .register_service(svc)
            .is_none());
    }

    pub fn run_server(&mut self) {
        let server = self.servers.as_mut().unwrap();
        server.server.build_and_bind().unwrap();
        server.server.start().unwrap();
    }

    pub fn run_status_server(&mut self) {
        // Create a status server.
        let mut status_server = StatusServer::new(1).unwrap();
        // Start the status server.
        let status_address = std::env::var(format!("status_address_{}", self.store_id))
            .unwrap_or("0.0.0.0:20211".to_string());
        info!("run status server on {}", status_address);
        status_server.start(status_address);
        self.status_server = Some(status_server);
    }

    fn stop(self) {
        unreachable!()
    }
}

#[allow(dead_code)]
pub fn wait_for_signal() {
    let trap = Trap::trap(&[SIGTERM, SIGINT, SIGHUP, SIGUSR1, SIGUSR2]);
    for sig in trap {
        match sig {
            SIGTERM | SIGINT | SIGHUP => {
                info!("receive signal {}, stopping server...", sig as c_int);
                break;
            }
            // TODO: handle more signal
            _ => unreachable!(),
        }
    }
}
