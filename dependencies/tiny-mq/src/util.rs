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

use kvproto::metapb::{self, PeerRole, Region};
use kvproto::raft_cmdpb::{CmdType, RaftCmdRequest, RaftCmdResponse, Request, Response};
use kvproto::raft_serverpb::RaftMessage;
use raft::eraftpb::ConfState;
use crate::errors::{Error, Result};
use protobuf::Message;
use tikv_util::box_err;
use std::fmt::Display;
use core::fmt;

use slog::Drain;
use slog::{o};
use slog_term;

pub const INVALID_ID: u64 = 0;

pub fn find_peer(region: &metapb::Region, store_id: u64) -> Option<&metapb::Peer> {
    region
        .get_peers()
        .iter()
        .find(|&p| p.get_store_id() == store_id)
}

// a helper function to create peer easily.
pub fn new_peer(store_id: u64, peer_id: u64) -> metapb::Peer {
    let mut peer = metapb::Peer::default();
    peer.set_store_id(store_id);
    peer.set_id(peer_id);
    peer.set_role(PeerRole::Voter);
    peer
}

#[inline]
pub fn is_region_initialized(r: &metapb::Region) -> bool {
    !r.get_peers().is_empty()
}

pub fn conf_state_from_region(region: &metapb::Region) -> ConfState {
    let mut conf_state = ConfState::default();
    for p in region.get_peers() {
        match p.get_role() {
            PeerRole::Voter => conf_state.mut_voters().push(p.get_id()),
            _ => unreachable!(),
        }
    }
    conf_state
}

#[inline]
pub fn check_store_id(req: &RaftCmdRequest, store_id: u64) -> Result<()> {
    let peer = req.get_header().get_peer();
    if peer.get_store_id() == store_id {
        Ok(())
    } else {
        Err(Error::StoreNotMatch {
            to_store_id: peer.get_store_id(),
            my_store_id: store_id,
        })
    }
}

#[inline]
pub fn check_term(req: &RaftCmdRequest, term: u64) -> Result<()> {
    let header = req.get_header();
    if header.get_term() == 0 || term <= header.get_term() + 1 {
        Ok(())
    } else {
        // If header's term is 2 verions behind current term,
        // leadership may have been changed away.
        Err(Error::StaleCommand)
    }
}

#[inline]
pub fn check_peer_id(req: &RaftCmdRequest, peer_id: u64) -> Result<()> {
    let header = req.get_header();
    if header.get_peer().get_id() == peer_id {
        Ok(())
    } else {
        Err(box_err!(
            "mismatch peer id {} != {}",
            header.get_peer().get_id(),
            peer_id
        ))
    }
}

/// Parse data of entry `index`.
///
/// # Panics
///
/// If `data` is corrupted, this function will panic.
// TODO: make sure received entries are not corrupted
#[inline]
pub fn parse_data_at<T: Message + Default>(data: &[u8], index: u64, tag: &str) -> T {
    let mut result = T::default();
    result.merge_from_bytes(data).unwrap_or_else(|e| {
        panic!("{} data is corrupted at {}: {:?}", tag, index, e);
    });
    result
}

pub struct MsgType<'a>(pub &'a RaftMessage);

impl Display for MsgType<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // if !self.0.has_extra_msg() {
            write!(f, "{:?}", self.0.get_message().get_msg_type())
        // } else {
        //     write!(f, "{:?}", self.0.get_extra_msg().get_type())
        // }
    }
}

// With feature protobuf-codec, `bytes::Bytes` is generated for `bytes` in protobuf.
#[cfg(feature = "protobuf-codec")]
pub fn bytes_capacity(b: &bytes::Bytes) -> usize {
    // NOTE: For deserialized raft messages, `len` equals capacity.
    // This is used to report memory usage to metrics.
    b.len()
}

// Currently `bytes::Bytes` are not available for prost-codec.
#[cfg(feature = "prost-codec")]
pub fn bytes_capacity(b: &Vec<u8>) -> usize {
    b.capacity()
}

#[cfg(test)]
pub mod tests {
    use std::fmt::{Display, Formatter};
    use kvproto::metapb::Region;
    use crate::util::{bytes_capacity, check_peer_id, check_store_id, check_term, conf_state_from_region, find_peer, is_region_initialized, MsgType, new_peer, parse_data_at};
    use raft_proto::eraftpb::ConfState;
    use kvproto::raft_cmdpb::RaftCmdRequest;
    use kvproto::raft_serverpb::RaftMessage;

    #[test]
    fn test_base() {
        let data = b"abc";
        let res: std::result::Result<RaftCmdRequest, Box<dyn std::any::Any + std::marker::Send>> = std::panic::catch_unwind(||{parse_data_at(data.as_slice(), 0, "")});
        assert!(res.is_err());

        let cmd_req = RaftCmdRequest::default();
        let msg_data = protobuf::Message::write_to_bytes(&cmd_req).unwrap();
        let parsed_cmd = parse_data_at(msg_data.as_slice(), 0, "");
        assert_eq!(cmd_req, parsed_cmd);

        let bytes = bytes::Bytes::from(data.as_slice());
        assert_ne!(bytes_capacity(&bytes), 0);

        let msg = RaftMessage::default();
        let msg_type = MsgType(&msg);
        print!("{}", format!("{}", msg_type));
    }

    #[test]
    fn test_region() {
        let mut region = Region::default();
        assert!(!is_region_initialized(&region));

        let store_id = 1;

        let peers = region.mut_peers();
        let peer = new_peer(store_id, 0);
        peers.push(peer);

        assert!(find_peer(&region, store_id).is_some());

        let conf_state = conf_state_from_region(&region);
        assert_eq!(conf_state.get_voters().len(), 1);
    }

    #[test]
    fn test_raft_cmd_request() {
        let mut cmd_req = RaftCmdRequest::default();
        assert!(check_store_id(&cmd_req, 0).is_ok());
        assert!(check_store_id(&cmd_req, 4).is_err());

        let mock_term = 2;
        cmd_req.mut_header().set_term(mock_term);
        assert_eq!(cmd_req.get_header().get_term(), 2);
        assert!(check_term(&cmd_req, 0).is_ok());
        assert!(check_term(&cmd_req, mock_term + 2).is_err());

        assert!(check_peer_id(&cmd_req, 0).is_ok());
        assert!(check_peer_id(&cmd_req, 1).is_err());
    }
}
