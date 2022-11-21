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
use hologram_protos::firm_walletpb::internal_servicepb::{Error as ErrorPb, LogTruncated};

#[derive(Debug)]
pub enum Error {
    LogTruncated,

    OtherError { reason: String },
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn into_proto(self) -> ErrorPb {
        let mut error_pb = ErrorPb::default();
        match self {
            Error::LogTruncated => {
                error_pb.set_log_truncated(LogTruncated::default());
            }
            Error::OtherError { reason } => {
                error_pb.mut_other_error().set_reason(reason);
            }
        }
        error_pb
    }
}
