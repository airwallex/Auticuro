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
use crate::AbstractNode;
use kvproto_tonic::kvrpcpb::ProbeRequest;
use kvproto_tonic::tikvpb::tikv_client::TikvClient;
use std::borrow::Borrow;
use std::env;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::{Mutex, MutexGuard};
use tokio::time::{sleep, Duration};
use tonic::transport::Channel;
use tower::timeout::Timeout;
use tracing::{info, warn};

const INITIAL_LEADER: usize = 1;

#[derive(Clone)]
pub struct RaftCluster<Node> {
    // ip address for raft peers, ordered by node id
    node_addresses: Vec<String>,

    inner: Arc<Mutex<Inner<Node>>>,
}

pub struct Inner<Node> {
    pub reset_leader_counter: usize,

    leader_node: Option<Node>,
    // node id of raft leader
    leader_id: usize,
    // next node to probe
    next_probe_node: usize,
}

impl<Node> RaftCluster<Node> {
    pub fn new() -> Self {
        let downstream_service =
            env::var("DOWNSTREAM_SERVICE").expect("DOWNSTREAM_SERVICE must be set");

        let downstream_services_address = Self::parse_address(&downstream_service);

        info!(
            "DOWNSTREAM_SERVICE: {:?}, initial_leader: {}",
            downstream_services_address, INITIAL_LEADER
        );

        let inner = Arc::new(Mutex::new(Inner {
            reset_leader_counter: 0,
            leader_node: None,
            leader_id: INITIAL_LEADER,
            next_probe_node: INITIAL_LEADER,
        }));

        RaftCluster {
            node_addresses: downstream_services_address,
            inner,
        }
    }
}

impl<Node> RaftCluster<Node>
where
    Node: Clone + AbstractNode,
{
    pub async fn get_or_reset_leader(&self, reset_leader_counter: usize) -> (Node, usize) {
        let mut guard = self.inner.lock().await;

        if reset_leader_counter <= guard.reset_leader_counter {
            if let Some(node) = guard.leader_node.borrow() {
                return ((*node).clone(), guard.reset_leader_counter);
            }
        }

        guard.reset_leader_counter += 1;

        self.reset_leader(guard).await
    }

    pub async fn reset_leader(
        &self,
        mut inner_guard: MutexGuard<'_, Inner<Node>>,
    ) -> (Node, usize) {
        let node_count = self.node_addresses.len();
        let mut node_id = inner_guard.next_probe_node;

        loop {
            match self.probe_node(node_id, inner_guard.leader_id).await {
                Err(e) => {
                    // node/peer id start from 1 while RaftCluster.node_addresses's index start from 0
                    node_id = node_id % node_count + 1;
                    warn!(
                        "Leader not found with error: {:?}, retrying with node_id={}",
                        e, node_id
                    );
                    sleep(Duration::from_secs(1)).await;
                }
                Ok(leader_id) => {
                    let address = self.node_addresses.get(leader_id - 1).unwrap();
                    let leader_node =
                        Node::new(Self::get_timeout_channel_until_success(address.clone()).await);

                    let _ = inner_guard.leader_node.insert(leader_node.clone());
                    inner_guard.leader_id = leader_id;
                    inner_guard.next_probe_node = node_id;
                    return (leader_node, inner_guard.reset_leader_counter);
                }
            }
        }
    }

    async fn probe_node(&self, node_id: usize, current_leader_id: usize) -> GeneralResult<usize> {
        let node_count = self.node_addresses.len();

        let address = self.node_addresses.get(node_id - 1).unwrap();

        let leader_id = Self::probe(&mut TikvClient::new(
            Self::get_timeout_channel(address.clone()).await?,
        ))
        .await?;

        if leader_id > node_count || leader_id == 0 {
            panic!("leader id {} out of index.", leader_id);
        }

        if leader_id != current_leader_id {
            info!("Found new leader id={}", leader_id);
        }
        Ok(leader_id)
    }
}

impl<T> RaftCluster<T> {
    fn parse_address(address_str: &str) -> Vec<String> {
        address_str
            .trim()
            .split(',')
            .map(|addr| addr.trim().to_string())
            .collect()
    }

    async fn probe(tikv_client: &mut TikvClient<Timeout<Channel>>) -> GeneralResult<usize> {
        let err = Box::new(Error::NotLeaderError);
        tikv_client
            .probe(ProbeRequest {})
            .await
            .ok()
            .and_then(|resp| resp.into_inner().leader)
            .map(|peer| peer.id as usize)
            .filter(|peer_id| *peer_id > 0_usize)
            .ok_or(err)
    }

    async fn get_timeout_channel(address: String) -> GeneralResult<Timeout<Channel>> {
        let endpoint = Channel::from_shared(address)?;
        let channel = endpoint.connect().await?;
        Ok(Timeout::new(channel, Duration::from_secs(2)))
    }

    async fn get_timeout_channel_until_success(address: String) -> Timeout<Channel> {
        loop {
            match Self::get_timeout_channel(address.clone()).await {
                Err(e) => {
                    warn!("Failed to create timeout channel with error: {}", e);
                }
                Ok(res) => return res,
            }
        }
    }
}

type GeneralResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("NotLeaderError")]
    NotLeaderError,
}
