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
use crate::account::Account;
use crate::metrics::*;
use byteorder::{BigEndian, ByteOrder};
use hologram_kv::keys::{self, DATA_MAX_KEY, DATA_MIN_KEY};
use hologram_kv::rocks_engine::{new_engine, RocksEngine};
use hologram_protos::firm_walletpb::accountpb::Account as AccountPb;
use hologram_protos::firm_walletpb::internal_servicepb::Event;
use protobuf::Message;
use std::collections::HashMap;
use std::env;
use std::path::Path;
use std::sync::Arc;
use tikv_util::time::{duration_to_sec, Instant};
use tikv_util::{debug, info};

/**
 * Hard state kind persisted in RocksDB
 */
pub struct HardStateKind<'a> {
    pub(crate) default_value: u64,
    pub(crate) key: &'a str,
}

pub const APPLIED_INDEX: HardStateKind<'static> = HardStateKind {
    default_value: 5, // raft log index starts from 5, a magic number from raft-rs.
    key: "applied_index",
};

pub const PERSISTED_LAST_SEQ_NUM: HardStateKind<'static> = HardStateKind {
    // the persisted last seq num will be 0 on start with no events, and the real last event's
    // seq_num starts from 1
    default_value: 0,
    key: "last_seq_num",
};

pub const PERSISTED_FIRST_SEQ_NUM: HardStateKind<'static> = HardStateKind {
    // the persisted first seq num will be 0 on start with no events, and the real first event's
    // seq_num starts from 1
    default_value: 0,
    key: "first_seq_num",
};

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct SequenceNumber(pub u64);

impl SequenceNumber {
    pub(crate) fn to_bytes(&self) -> [u8; 8] {
        let mut bytes = [0; 8];
        BigEndian::write_u64(&mut bytes[..], self.0);
        bytes
    }

    pub(crate) fn from_bytes(bytes: &[u8]) -> Self {
        assert_eq!(bytes.len(), 8);
        let seq_num = BigEndian::read_u64(&bytes[..]);
        Self(seq_num)
    }
}

#[derive(Clone)]
pub struct RocksDBState {
    db: RocksEngine,
}

impl RocksDBState {
    pub fn init() -> Self {
        let db_path = env::var("wallet_db_path").unwrap();
        let db_path = Path::new(&db_path);
        let rocksdb_engine =
            RocksEngine::from_db(Arc::new(new_engine(db_path.to_str().unwrap(), &[])));
        RocksDBState { db: rocksdb_engine }
    }

    pub fn get_db(&self) -> RocksEngine {
        self.db.clone()
    }

    pub fn read_hard_state_from_db(&self, hard_state_kind: HardStateKind) -> u64 {
        let val = match self.db.get_value(hard_state_kind.key.as_bytes()) {
            Some(bytes) => BigEndian::read_u64(&bytes),
            None => {
                debug!(
                    "Hard state: {} does not exist, assume it is an empty DB, using default value: {}",
                    hard_state_kind.key, hard_state_kind.default_value
                );
                hard_state_kind.default_value
            }
        };
        debug!("Read {}={} from db.", hard_state_kind.key, val);
        val
    }

    pub fn read_event_from_db(&self, seq_num: SequenceNumber) -> Option<Event> {
        let begin_instant = Instant::now_coarse();

        let bytes = seq_num.to_bytes();
        let event_key = keys::event_key(&bytes);
        let event_opt = self.db.get_msg::<Event>(&event_key);

        CRITICAL_EVENT_HISTOGRAM_STATIC
            .read_event_from_db
            .observe(duration_to_sec(begin_instant.elapsed()));
        event_opt
    }

    pub fn read_seq_num_from_db(&self, command_id: &str) -> Option<SequenceNumber> {
        let begin_instant = Instant::now_coarse();
        let dedup_key = keys::dedup_key(command_id.as_bytes());
        let seq_num_opt = if let Some(bytes) = self.db.get_value(&dedup_key) {
            Some(SequenceNumber::from_bytes(&bytes))
        } else {
            None
        };

        CRITICAL_EVENT_HISTOGRAM_STATIC
            .read_seq_num_from_db
            .observe(duration_to_sec(begin_instant.elapsed()));
        seq_num_opt
    }

    pub fn read_accounts_from_db(&self) -> HashMap<String, Account> {
        let begin_instant = Instant::now_coarse();
        let mut accounts = HashMap::new();

        self.db
            .scan(DATA_MIN_KEY, DATA_MAX_KEY, false, |key, value| {
                let account_id = String::from_utf8(keys::origin_data_key(key).to_owned()).unwrap();
                let account = Account::from_proto(AccountPb::parse_from_bytes(value).unwrap());
                accounts.insert(account_id, account);
                Ok(true)
            })
            .unwrap();

        info!(
            "Recover {} accounts from db, cost {:?}.",
            accounts.len(),
            begin_instant.elapsed()
        );
        accounts
    }
}
