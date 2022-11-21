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
use hologram_protos::firm_walletpb::errorpb::Error as ErrorPb;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Timeout")]
    Timeout,

    #[error("ConflictDedupId dedup_id={}", .0)]
    ConflictDedupId(String),

    #[error("DataNotReady requested_seq_num={}, available_seq_num={}", .requested_seq_num, .available_seq_num)]
    DataNotReady {
        requested_seq_num: u64,
        available_seq_num: u64,
    },

    #[error("MessageQueueError details:{}", .0)]
    MessageQueueError(String),
}

impl Error {
    pub fn into_proto(self) -> ErrorPb {
        let mut error_pb = ErrorPb::default();
        match self {
            Error::MessageQueueError(err) => {
                if is_not_leader_error(&err) {
                    error_pb.mut_not_leader().set_message(err.to_string());
                } else {
                    error_pb.mut_other_error().set_reason(format!("{:?}", err));
                }
            }
            _ => {
                error_pb.mut_other_error().set_reason(format!("{:?}", self));
            }
        }
        error_pb
    }
}

pub fn is_not_leader_error(err: &str) -> bool {
    err.contains("not_leader: Some(NotLeader")
}

#[cfg(test)]
mod test {
    use super::is_not_leader_error;

    #[test]
    fn test_extract_leader() {
        // Error from consensus send_payload
        let error = "RequestFailed(Error { message: \"NotLeader(1, None)\", not_leader: Some(NotLeader \
        { region_id: 1, leader: None, unknown_fields: UnknownFields { fields: None }, cached_size:\
         CachedSize { size: 0 } }), region_not_found: None, server_is_busy: None, stale_command: None,\
          store_not_match: None, raft_entry_too_large: None, region_not_initialized: None, \
          unknown_fields: UnknownFields { fields: None }, cached_size: CachedSize { size: 0 } })";

        let result = is_not_leader_error(error);
        assert!(result)
    }
}
