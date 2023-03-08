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

use std::error::Error as StdError;
use std::io::Error as IoError;
use std::net::AddrParseError;
use std::result;

use futures::channel::oneshot::Canceled;
use grpcio::Error as GrpcError;
// use hyper::Error as HttpError;
// use openssl::error::ErrorStack as OpenSSLError;
use protobuf::ProtobufError;
use thiserror::Error;

use kvproto::{errorpb, metapb, raft_serverpb::RaftMessage};
use crossbeam::channel::{SendError, TrySendError};
use crate::msg::PeerMsg;

// use engine_traits::Error as EngineTraitError;
// use pd_client::Error as PdError;
// use raftstore::Error as RaftServerError;
// use tikv_util::codec::Error as CodecError;
// use tikv_util::worker::ScheduleError;

// use super::snap::Task as SnapTask;
// use crate::storage::kv::Error as EngineError;
// use crate::storage::Error as StorageError;

#[derive(Debug, Error)]
pub enum Error {
    #[error("{0:?}")]
    Other(#[from] Box<dyn StdError + Sync + Send>),

    // Following is for From other errors.
    #[error("{0:?}")]
    Io(#[from] IoError),

    #[error("{0}")]
    Protobuf(#[from] ProtobufError),

    #[error("{0:?}")]
    Grpc(#[from] GrpcError),

    // #[error("{0:?}")]
    // Codec(#[from] CodecError),

    #[error("{0:?}")]
    AddrParse(#[from] AddrParseError),

    // #[error("{0:?}")]
    // RaftServer(#[from] RaftServerError),

    // #[error("{0:?}")]
    // Engine(#[from] EngineError),

    // #[error("{0:?}")]
    // EngineTrait(#[from] EngineTraitError),

    // #[error("{0:?}")]
    // Storage(#[from] StorageError),

    #[error("failed to poll from mpsc receiver")]
    Sink,

    #[error("{0:?}")]
    RecvError(#[from] Canceled),

    // #[error("{0:?}")]
    // Http(#[from] HttpError),

    // #[error("{0:?}")]
    // OpenSSL(#[from] OpenSSLError),

    #[error("{0:?}")]
    SendError(#[from] SendError<RaftMessage>),

    #[error("{0:?}")]
    TrySendError(#[from] TrySendError<RaftMessage>),

    #[error("raft entry is too large, region {}, entry size {}", .region_id, .entry_size)]
    RaftEntryTooLarge { region_id: u64, entry_size: u64 },

    #[error("to store id {}, mine {}", .to_store_id, .my_store_id)]
    StoreNotMatch { to_store_id: u64, my_store_id: u64 },

    #[error("region {0} not found")]
    RegionNotFound(u64),

    #[error("region {0} not initialized yet")]
    RegionNotInitialized(u64),

    #[error("peer is not leader for region {0}, leader may {1:?}")]
    NotLeader(u64, Option<metapb::Peer>),

    #[error("Raft {0}")]
    Raft(#[from] raft::Error),

    #[error("Timeout {0}")]
    Timeout(String),

    #[error("stale command")]
    StaleCommand,

    #[error("{}", .0.get_message())]
    RequestFailed(errorpb::Error),

    // Engine uses plain string as the error.
    #[error("Storage Engine {0}")]
    Engine(String),
}

pub type Result<T> = result::Result<T, Error>;

impl From<Error> for errorpb::Error {
    fn from(err: Error) -> errorpb::Error {
        let mut errorpb = errorpb::Error::default();
        errorpb.set_message(format!("{:?}", err));

        match err {
            Error::RegionNotFound(region_id) => {
                errorpb.mut_region_not_found().set_region_id(region_id);
            }
            Error::NotLeader(region_id, leader) => {
                if let Some(leader) = leader {
                    errorpb.mut_not_leader().set_leader(leader);
                }
                errorpb.mut_not_leader().set_region_id(region_id);
            }
            Error::RaftEntryTooLarge {
                region_id,
                entry_size,
            } => {
                errorpb.mut_raft_entry_too_large().set_region_id(region_id);
                errorpb
                    .mut_raft_entry_too_large()
                    .set_entry_size(entry_size);
            }
            Error::StoreNotMatch {
                to_store_id,
                my_store_id,
            } => {
                errorpb
                    .mut_store_not_match()
                    .set_request_store_id(to_store_id);
                errorpb
                    .mut_store_not_match()
                    .set_actual_store_id(my_store_id);
            }
            Error::StaleCommand => {
                errorpb.set_stale_command(errorpb::StaleCommand::default());
            }
            Error::RegionNotInitialized(region_id) => {
                let mut e = errorpb::RegionNotInitialized::default();
                e.set_region_id(region_id);
                errorpb.set_region_not_initialized(e);
            }
            _ => {}
        };

        errorpb
    }
}

impl From<String> for Error {
    fn from(err: String) -> Self {
        Error::Engine(err)
    }
}