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
use crate::config::EventLogGCConfig;
use crate::rocksdb_state::{
    RocksDBState, SequenceNumber, PERSISTED_FIRST_SEQ_NUM, PERSISTED_LAST_SEQ_NUM,
};
use byteorder::{BigEndian, ByteOrder};
use hologram_kv::keys;
use hologram_kv::rocks_engine::{RocksEngine, RocksWriteBatch};
use hologram_protos::firm_walletpb::internal_servicepb::Event;
use protobuf::Message;
use std::cmp::{max, min};
use std::thread;
use std::time::Duration;
use tikv_util::time::Instant;
use tikv_util::{debug, info};

pub struct EventLogGCLoop {
    rocksdb_state: RocksDBState,
    config: EventLogGCConfig,
}

impl EventLogGCLoop {
    pub fn new(rocksdb_state: RocksDBState, config: EventLogGCConfig) -> Self {
        EventLogGCLoop {
            rocksdb_state,
            config,
        }
    }

    pub fn run(&mut self) {
        loop {
            let first_seq_num = self
                .rocksdb_state
                .read_hard_state_from_db(PERSISTED_FIRST_SEQ_NUM);
            let last_seq_num = self
                .rocksdb_state
                .read_hard_state_from_db(PERSISTED_LAST_SEQ_NUM);

            if let Some(next_first_seq_num) = self.should_gc(first_seq_num, last_seq_num) {
                self.gc(first_seq_num, next_first_seq_num);
            } else {
                thread::sleep(Duration::from_millis(self.config.poll_interval_millis));
            }
        }
    }

    // Return Some(next_first_seq_num) if need gc, and after gc, the persisted first_seq_num
    // should be equal to next_first_seq_num
    fn should_gc(&self, first_seq_num: u64, last_seq_num: u64) -> Option<u64> {
        assert!(last_seq_num >= first_seq_num);
        if last_seq_num - first_seq_num < self.config.count_limit {
            return None;
        }
        let gc_count = (self.config.count_limit as f64 * self.config.percentage) as u64;
        let next_first_seq_num = first_seq_num + gc_count;
        debug!(
            "Current entries count={} between persisted_first_seq_num={}, \
            persisted_last_seq_num={} exceeds the gc_count_limit={}. Events within [{}, {}) \
            will be truncated, gc_count={} calculated by gc_count_limit({})  * gc_percentage({})",
            last_seq_num - first_seq_num,
            first_seq_num,
            last_seq_num,
            self.config.count_limit,
            first_seq_num,
            next_first_seq_num,
            gc_count,
            self.config.count_limit,
            self.config.percentage
        );

        return Some(next_first_seq_num);
    }

    fn gc(&mut self, first_seq_num: u64, next_first_seq_num: u64) {
        // Real entry's seq_num will start from PERSISTED_FIRST_SEQ_NUM.default_val + 1
        let mut from = max(first_seq_num, PERSISTED_FIRST_SEQ_NUM.default_value + 1);
        let mut to = min(from + self.config.batch_size, next_first_seq_num);

        // Todo Considerations
        // 1. Consider porting RocksEngine::write_batch_with_cap from tikv to consensus, tikv is using
        // write_batch_with_cap, not sure if it is for more efficient memory allocation.
        let mut gc_wb = self.rocksdb_state.get_db().write_batch();

        while from < next_first_seq_num {
            EventLogGCLoop::gc_batch(from, to, &self.rocksdb_state.get_db(), &mut gc_wb);
            from = to;
            to = min(from + self.config.batch_size, next_first_seq_num);
        }
    }

    // Delete 1). seq_num -> event; 2), command_id -> seq_num; entries in rocksDB where seq_num
    // is within [from, to). Caller should guarantee the validity of from & to
    fn gc_batch(from: u64, to: u64, db: &RocksEngine, gc_wb: &mut RocksWriteBatch) {
        let begin_instant = Instant::now_coarse();

        info!(
            "EventLogGCLoop.gc_batch() was called, starting to truncate entries whose seq_num is \
            within [{},{})",
            from, to
        );

        let start_seq_num_bytes = SequenceNumber(from).to_bytes(); // inclusive
        let end_seq_num_bytes = SequenceNumber(to).to_bytes(); // exclusive
        let start_event_key = keys::event_key(&start_seq_num_bytes);
        let end_event_key = keys::event_key(&end_seq_num_bytes);

        // scan all the command_ids to be deleted from event
        db.scan(
            &start_event_key,
            &end_event_key,
            false,
            |event_key, event_value| {
                let mut event = Event::default();
                event.merge_from_bytes(event_value)?;

                // 1. Delete seq_num -> event entries;
                gc_wb.delete(event_key);

                // 2. Delete command_id -> seq_num entries;
                let command_key = keys::dedup_key(event.get_header().get_command_id().as_bytes());
                gc_wb.delete(&command_key);
                Ok(true)
            },
        )
        .unwrap();

        debug!("{} event logs will be deleted", to - from);

        // 3. update the PERSISTED_FIRST_SEQ_NUM
        let mut buf = [0; 8];

        BigEndian::write_u64(&mut buf, to);
        gc_wb.put(PERSISTED_FIRST_SEQ_NUM.key.as_bytes(), &buf);

        gc_wb.write_opt(false);
        gc_wb.clear();
        info!(
            "GC finished, entries whose seq_num within [{}, {}) deleted, elapsed: {:?}",
            from,
            to,
            begin_instant.elapsed()
        )
    }
}
