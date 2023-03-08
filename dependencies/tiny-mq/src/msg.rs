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

use std::fmt;
use std::time::Instant;

use bitflags::bitflags;
use kvproto::raft_cmdpb::{RaftCmdRequest, RaftCmdResponse};
use kvproto::raft_serverpb::RaftMessage;

use crate::apply::TaskRes as ApplyTaskRes;

#[derive(Debug)]
pub struct ReadResponse {
    pub response: RaftCmdResponse,
}

#[derive(Debug)]
pub struct WriteResponse {
    pub response: RaftCmdResponse,
}

pub type ReadCallback = Box<dyn FnOnce(ReadResponse) + Send>;
pub type WriteCallback = Box<dyn FnOnce(WriteResponse) + Send>;
pub type ExtCallback = Box<dyn FnOnce() + Send>;

/// Variants of callbacks for `Msg`.
///  - `Read`: a callback for read only requests including `StatusRequest`,
///         `GetRequest` and `SnapRequest`
///  - `Write`: a callback for write only requests including `AdminRequest`
///          `PutRequest`, `DeleteRequest` and `DeleteRangeRequest`.
pub enum Callback {
    /// No callback.
    None,
    /// Read callback.
    // Read(ReadCallback<S>),
    Read(ReadCallback),
    /// Write callback.
    Write {
        cb: WriteCallback,
        /// `proposed_cb` is called after a request is proposed to the raft group successfully.
        /// It's used to notify the caller to move on early because it's very likely the request
        /// will be applied to the raftstore.
        proposed_cb: Option<ExtCallback>,
        /// `committed_cb` is called after a request is committed and before it's being applied, and
        /// it's guaranteed that the request will be successfully applied soon.
        committed_cb: Option<ExtCallback>,
    },
}

impl Callback {
    pub fn write(cb: WriteCallback) -> Self {
        Self::write_ext(cb, None, None)
    }

    pub fn write_ext(
        cb: WriteCallback,
        proposed_cb: Option<ExtCallback>,
        committed_cb: Option<ExtCallback>,
    ) -> Self {
        Callback::Write {
            cb,
            proposed_cb,
            committed_cb,
        }
    }

    pub fn invoke_with_response(self, resp: RaftCmdResponse) {
        match self {
            Callback::None => (),
            Callback::Read(read) => {
                let resp = ReadResponse { response: resp };
                read(resp);
            }
            Callback::Write { cb, .. } => {
                let resp = WriteResponse { response: resp };
                cb(resp);
            }
        }
    }

    pub fn invoke_proposed(&mut self) {
        if let Callback::Write { proposed_cb, .. } = self {
            if let Some(cb) = proposed_cb.take() {
                cb()
            }
        }
    }

    pub fn invoke_committed(&mut self) {
        if let Callback::Write { committed_cb, .. } = self {
            if let Some(cb) = committed_cb.take() {
                cb()
            }
        }
    }

    pub fn is_none(&self) -> bool {
        matches!(self, Callback::None)
    }
}

impl fmt::Debug for Callback {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Callback::None => write!(fmt, "Callback::None"),
            Callback::Read(_) => write!(fmt, "Callback::Read(..)"),
            Callback::Write { .. } => write!(fmt, "Callback::Write(..)"),
        }
    }
}

bitflags! {
    pub struct PeerTicks: u8 {
        const RAFT                   = 0b00000001;
        const RAFT_LOG_GC            = 0b00000010;
    }
}

impl PeerTicks {
    #[inline]
    pub fn tag(self) -> &'static str {
        match self {
            PeerTicks::RAFT => "raft",
            PeerTicks::RAFT_LOG_GC => "raft_log_gc",
            _ => unreachable!(),
        }
    }
    pub fn get_all_ticks() -> &'static [PeerTicks] {
        const TICKS: &[PeerTicks] = &[PeerTicks::RAFT, PeerTicks::RAFT_LOG_GC];
        TICKS
    }
}

/// Raft command is the command that is expected to be proposed by the
/// leader of the target raft group.
#[derive(Debug)]
pub struct RaftCommand {
    pub send_time: Instant,
    pub request: RaftCmdRequest,
    pub callback: Callback,
    // pub deadline: Option<Deadline>,
}

impl RaftCommand {
    #[inline]
    pub fn new(request: RaftCmdRequest, callback: Callback) -> RaftCommand {
        RaftCommand {
            request,
            callback,
            send_time: Instant::now(),
        }
    }
}

pub struct InspectedRaftMessage {
    pub heap_size: usize,
    pub msg: RaftMessage,
}

/// Message that can be sent to a peer.
pub enum PeerMsg {
    /// Raft message is the message sent between raft nodes in the same
    /// raft group. Messages need to be redirected to raftstore if target
    /// peer doesn't exist.
    RaftMessage(InspectedRaftMessage),
    /// Raft command is the command that is expected to be proposed by the
    /// leader of the target raft group. If it's failed to be sent, callback
    /// usually needs to be called before dropping in case of resource leak.
    RaftCommand(RaftCommand),
    /// Tick is periodical task. If target peer doesn't exist there is a potential
    /// that the raft node will not work anymore.
    Tick(PeerTicks),
    /// Result of applying committed entries. The message can't be lost.
    ApplyRes { res: ApplyTaskRes },
    /// Start the FSM.
    Start,
}

impl fmt::Debug for PeerMsg {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PeerMsg::RaftMessage(_) => write!(fmt, "Raft Message"),
            PeerMsg::RaftCommand(_) => write!(fmt, "Raft Command"),
            PeerMsg::Tick(tick) => write! {
                fmt,
                "{:?}",
                tick
            },
            PeerMsg::ApplyRes { res } => write!(fmt, "ApplyRes {:?}", res),
            _ => unreachable!(),
        }
    }
}
