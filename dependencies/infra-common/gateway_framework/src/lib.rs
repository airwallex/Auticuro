#![allow(clippy::new_without_default)]
#![feature(fundamental)]
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
#[macro_use]
pub mod gateway;
pub mod raft_cluster;

use tonic::transport::Channel;
use tower::timeout::Timeout;

pub trait AbstractNode {
    fn new(channel: Timeout<Channel>) -> Self;
}

pub trait AbstractResponse {
    fn success(&self) -> bool;

    fn has_not_leader_error(&self) -> bool;
}
