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
use crate::raft_cluster::RaftCluster;

#[derive(Clone)]
#[fundamental]
pub struct Gateway<Node> {
    pub raft_cluster: RaftCluster<Node>,
}

impl<Node> Gateway<Node> {
    pub fn new() -> Self {
        Gateway {
            raft_cluster: RaftCluster::new(),
        }
    }
}

#[macro_export]
macro_rules! impl_gateway {
    ($service_name:ident, $node_type:ty, $service_entrypoint:ident,
    $(($fn_name:ident, $request_type:ty, $response_wrapper:ident <$response_type:ty>)),*) => {
        use std::time::Instant;
        use async_trait::async_trait;
        use tokio::time::{sleep, Duration};
        use tonic::{Request, Response, Status};
        use tracing::{info, warn};
        use $crate::gateway::Gateway;
        use $crate::AbstractResponse;

        #[async_trait]
        impl $service_name for Gateway<$node_type> {
            $(
                async fn $fn_name(
                    &self,
                    request: Request<$request_type>,
                ) -> Result<Response<$response_type>, Status> {
                    let request = request.into_inner();
                    let response: Result<Response<$response_type>, Status>;

                    let mut reset_leader_counter = 0;
                    let mut leader_node;

                    loop {
                        (leader_node, reset_leader_counter) =
                        self.raft_cluster.get_or_reset_leader(reset_leader_counter).await;

                        let start = Instant::now();
                        let grpc_response =
                            leader_node.$service_entrypoint.$fn_name(request.clone()).await;
                        if let Ok(ref res) = grpc_response {
                            // Gateway service should directly return response to the caller if:
                            // 1. No error in the response
                            // 2. Response has business related error, which is not the NotLeader error
                            let response_wrapper = $response_wrapper(res.get_ref());
                            if response_wrapper.success() || !response_wrapper.has_not_leader_error()
                            {
                                 info!(
                                     "Gateway service send request: {:?}, and received response:\
                                     {:?}, await time: {:?}",
                                     request,
                                     res,
                                     start.elapsed()
                                 );

                                 response = grpc_response;
                                 break;
                            }
                        }

                        warn!(
                            "Gateway will re-send request: {:?} after 2 secs, since response: {:?} \
                            received, this may imply temporary grpc error or downstream service is \
                            during leader change, time elapsed: {:?}",
                            request,
                            grpc_response,
                            start.elapsed()
                        );

                        reset_leader_counter += 1;
                        sleep(Duration::from_secs(1)).await;
                    }
                    response
                }
            )*
        }
    }
}
