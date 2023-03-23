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

use crate::errors::Result;
use crate::kv::Service;
use crate::raft_client::{AddressMap, ConnectionBuilder, RaftClient};
use crate::storage::Storage;
use crate::store::RaftRouter;
use crossbeam::channel;
use grpcio::{
    ChannelBuilder, EnvBuilder, Environment, ResourceQuota, Server as GrpcServer, ServerBuilder,
};
use kvproto::raft_serverpb::RaftMessage;
use kvproto::tikvpb::*;
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tikv_util::{info, thd_name, Either};

pub const GRPC_THREAD_PREFIX: &str = "grpc-server";

/// The TiKV server
///
/// It hosts various internal components, including gRPC, the raftstore router
/// and a snapshot worker.
pub struct Server {
    env: Arc<Environment>,
    /// A GrpcServer builder or a GrpcServer.
    ///
    /// If the listening port is configured, the server will be started lazily.
    server: Option<GrpcServer>,
    local_addr: SocketAddr,
    pub raft_client: RaftClient,
    services: Vec<grpcio::Service>,
}

impl Server {
    pub fn new(
        store_id: u64,
        storage: Storage,
        resolver: AddressMap,
        // sender: Arc<channel::Sender<RaftMessage>>,
        router: RaftRouter,
        env: Arc<Environment>,
    ) -> Result<Self> {
        let kv_service = Service::new(store_id, storage, router);

        let hostname = resolver.resolve(store_id);
        let parts: Vec<&str> = hostname
            .split(':')
            .filter(|s| !s.is_empty()) // "Host:" or ":Port" are invalid.
            .collect();
        // ["Host", "Port"]
        assert_eq!(parts.len(), 2);
        // Server is listening on "0.0.0.0:Port"
        let addr = SocketAddr::from_str(&format!("0.0.0.0:{}", parts[1]))?;

        let conn_builder = ConnectionBuilder::new(env.clone(), resolver);
        let raft_client = RaftClient::new(conn_builder);

        let svr = Server {
            env: env.clone(),
            server: None,
            local_addr: addr,
            raft_client,
            services: vec![create_tikv(kv_service)]
        };

        Ok(svr)
    }

    /// Register a gRPC service.
    /// Register after starting, it fails and returns the service.
    pub fn register_service(&mut self, svc: grpcio::Service) -> Option<grpcio::Service> {
        if self.server.is_none() {
            self.services.push(svc);
            None
        } else {
            Some(svc)
        }
    }

    /// Build gRPC server and bind to address.
    pub fn build_and_bind(&mut self) -> Result<SocketAddr> {
        let channel_args = ChannelBuilder::new(self.env.clone())
            .stream_initial_window_size(2 * 1024 * 1024 as i32)
            .max_concurrent_stream(1024)
            .max_receive_message_len(-1)
            .max_send_message_len(-1)
            .http2_max_ping_strikes(i32::MAX) // For pings without data from clients.
            .keepalive_time(Duration::from_secs(10))
            .keepalive_timeout(Duration::from_secs(3))
            .build_args();

        let mut sb = ServerBuilder::new(self.env.clone())
            .channel_args(channel_args);
        let ip = format!("{}", self.local_addr.ip());
        sb = sb.bind(ip, self.local_addr.port());
        for service in self.services.drain(..) {
            sb = sb.register_service(service);
        }

        let server = sb.build()?;
        let (host, port) = server.bind_addrs().next().unwrap();
        let addr = SocketAddr::new(IpAddr::from_str(host)?, port);
        self.local_addr = addr;
        self.server = Some(server);
        Ok(addr)
    }

    /// Starts the TiKV server.
    /// Notice: Make sure call `build_and_bind` first.
    pub fn start(&mut self) -> Result<()> {
        let mut grpc_server = self.server.take().unwrap();
        info!("listening on addr"; "addr" => &self.local_addr);
        grpc_server.start();
        self.server = Some(grpc_server);

        info!("TiKV is ready to serve");
        Ok(())
    }

    /// Stops the TiKV server.
    pub fn stop(&mut self) -> Result<()> {
        if let Some(mut server) = self.server.take() {
            server.shutdown();
        }
        Ok(())
    }
}

#[cfg(test)]
pub mod tests {
    use std::sync::{Arc, Mutex};
    use tempfile::Builder;
    use std::time::Duration;
    use crossbeam::channel;
    use grpcio::{EnvBuilder, Environment};
    use raft::Config;
    use crate::node::create_raft_storage;
    use crate::raft_client::AddressMap;
    use crate::raftkv::RaftKv;
    use crate::rocks_engine::{self, ALL_CFS, Engines, RocksEngine};
    use crate::server::{Server, GRPC_THREAD_PREFIX};
    use crate::storage::Storage;
    use crate::store::RaftRouter;
    use kvproto::raft_serverpb::RaftMessage;
    use tikv_util::thd_name;
    use crate::msg::{InspectedRaftMessage, PeerMsg};

    pub fn mock_storage(router: RaftRouter, raft_path: &str, kv_path: &str) -> Storage {
        let raft_db = rocks_engine::new_engine(raft_path, ALL_CFS);
        let kv_db = rocks_engine::new_engine(kv_path, ALL_CFS);

        let kv_engine = RocksEngine::from_db(Arc::new(kv_db));
        let raft_engine = RocksEngine::from_db(Arc::new(raft_db));

        let engines = Engines::new(kv_engine, raft_engine);
        let raftkv = RaftKv::new(router.clone(),engines.clone());

        let storage = create_raft_storage(raftkv).unwrap();
        storage
    }

    #[test]
    #[ignore]
    fn test_peer_resolve() {

        // let mock_raft_db = Builder::new().prefix("/tmp/mock_raft_db").tempdir().unwrap();
        // let mock_kv_db = Builder::new().prefix("/tmp/mock_db").tempdir().unwrap();
        let mock_raft_db = "/tmp/mock_raft_db";
        let mock_kv_db = "/tmp/mock_db";

        let env = Arc::new(
            EnvBuilder::new()
                // .cq_count(config.server.grpc_concurrency)
                .cq_count(30)
                .name_prefix(thd_name!(GRPC_THREAD_PREFIX))
                .build(),
        );
        let (tx, rx) = channel::unbounded();
        let router = RaftRouter { router: tx};

        // let storage = mock_storage(router.clone(), mock_raft_db.path().to_str().unwrap(), mock_kv_db.path().to_str().unwrap());
        let storage = mock_storage(router.clone(), mock_raft_db, mock_kv_db);
        let mock_store_id = 0;

        let mut resolver = AddressMap::default();

        resolver.insert(0, format!("0.0.0.0:2016{}", 0));

        let mut server  = Server::new(mock_store_id, storage, resolver, router.clone(), env).unwrap();

        server.build_and_bind();

        assert!(server.start().is_ok());

        let msg = Default::default();
        let _ = server.raft_client.clone();
        server.raft_client.send(msg);

        if server.raft_client.need_flush() {
            server.raft_client.flush();
        }

        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok());
        assert!(server.stop().is_ok());
    }
}
