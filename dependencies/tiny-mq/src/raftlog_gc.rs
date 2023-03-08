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
use std::fmt::{self, Display, Formatter};
use std::sync::mpsc::Sender;

use thiserror::Error;

use crate::rocks_engine::Engines;
use crate::keys;
// use engine_traits::{Engines, KvEngine, RaftEngine};
// use file_system::{IOType, WithIOType};
use tikv_util::time::Duration;
use tikv_util::worker::{Runnable, RunnableWithTimer};
use tikv_util::{box_try, debug, error, warn};

// use crate::store::{CasualMessage, CasualRouter};

const MAX_GC_REGION_BATCH: usize = 128;
const COMPACT_LOG_INTERVAL: Duration = Duration::from_secs(60);

pub enum Task {
    Gc {
        region_id: u64,
        start_idx: u64,
        end_idx: u64,
    },
    // Purge,
}

impl Task {
    pub fn gc(region_id: u64, start: u64, end: u64) -> Self {
        Task::Gc {
            region_id,
            start_idx: start,
            end_idx: end,
        }
    }
}

impl Display for Task {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Task::Gc {
                region_id,
                start_idx,
                end_idx,
            } => write!(
                f,
                "GC Raft Logs [region: {}, from: {}, to: {}]",
                region_id, start_idx, end_idx
            ),
            // Task::Purge => write!(f, "Purge Expired Files",),
        }
    }
}

#[derive(Debug, Error)]
enum Error {
    #[error("raftlog gc failed {0:?}")]
    Other(#[from] Box<dyn StdError + Sync + Send>),
}


pub struct Runner {
// pub struct Runner<EK: KvEngine, ER: RaftEngine, R: CasualRouter<EK>> {
    // ch: R,
    tasks: Vec<Task>,
    engines: Engines,
    gc_entries: Option<Sender<usize>>,
}

// impl<EK: KvEngine, ER: RaftEngine, R: CasualRouter<EK>> Runner<EK, ER, R> {
impl Runner {
    // pub fn new(ch: R, engines: Engines<EK, ER>) -> Runner<EK, ER, R> {
    pub fn new(engines: Engines) -> Runner {
        Runner {
            // ch,
            engines,
            tasks: vec![],
            gc_entries: None,
        }
    }

    /// Does the GC job and returns the count of logs collected.
    fn gc_raft_log(
        &mut self,
        region_id: u64,
        start_idx: u64,
        end_idx: u64,
    ) -> Result<usize, Error> {
        let deleted = box_try!(self.engines.raft.gc(region_id, start_idx, end_idx));
        Ok(deleted)
    }

    fn report_collected(&self, collected: usize) {
        if let Some(ref ch) = self.gc_entries {
            ch.send(collected).unwrap();
        }
    }

    fn flush(&mut self) {
        // Sync wal of kv_db to make sure the data before apply_index has been persisted to disk.
        // self.engines.kv.sync().unwrap_or_else(|e| {
        //     panic!("failed to sync kv_engine in raft_log_gc: {:?}", e);
        // });
        self.engines.kv.sync();
        let tasks = std::mem::take(&mut self.tasks);
        for t in tasks {
            match t {
                Task::Gc {
                    region_id,
                    start_idx,
                    end_idx,
                } => {
                    debug!("gc raft log"; "region_id" => region_id, "end_index" => end_idx);
                    match self.gc_raft_log(region_id, start_idx, end_idx) {
                        Err(e) => {
                            error!("failed to gc"; "region_id" => region_id, "err" => %e);
                            self.report_collected(0);
                        }
                        Ok(n) => {
                            debug!("gc log entries"; "region_id" => region_id, "entry_count" => n);
                            self.report_collected(n);
                        }
                    }
                }
                // Task::Purge => {
                //     let regions = match self.engines.raft.purge_expired_files() {
                //         Ok(regions) => regions,
                //         Err(e) => {
                //             warn!("purge expired files"; "err" => %e);
                //             return;
                //         }
                //     };
                //     for region_id in regions {
                //         let _ = self.ch.send(region_id, CasualMessage::ForceCompactRaftLogs);
                //     }
                // }
            }
        }
    }
}

// impl<EK, ER, R> Runnable for Runner<EK, ER, R>
//     where
//         EK: KvEngine,
//         ER: RaftEngine,
//         R: CasualRouter<EK>,
impl Runnable for Runner {
    type Task = Task;

    fn run(&mut self, task: Task) {
        // let _io_type_guard = WithIOType::new(IOType::ForegroundWrite);
        self.tasks.push(task);
        if self.tasks.len() < MAX_GC_REGION_BATCH {
            return;
        }
        self.flush();
    }

    fn shutdown(&mut self) {
        self.flush();
    }
}

// impl<EK, ER, R> RunnableWithTimer for Runner<EK, ER, R>
//     where
//         EK: KvEngine,
//         ER: RaftEngine,
//         R: CasualRouter<EK>,
impl RunnableWithTimer for Runner {
    fn on_timeout(&mut self) {
        self.flush();
    }

    fn get_interval(&self) -> Duration {
        COMPACT_LOG_INTERVAL
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // use engine_traits::{KvEngine, Mutable, WriteBatch, WriteBatchExt, ALL_CFS, CF_DEFAULT};
    use crate::rocks_engine::*;
    use std::sync::{mpsc, Arc};
    use std::time::Duration;
    use tempfile::Builder;

    #[test]
    fn test_gc_raft_log() {
        let dir = Builder::new().prefix("gc-raft-log-test").tempdir().unwrap();
        let path_raft = dir.path().join("raft");
        let path_kv = dir.path().join("kv");
        // let raft_db =
        //     engine_test::raft::new_engine(path_kv.to_str().unwrap(), None, CF_DEFAULT, None)
        //         .unwrap();
        // let kv_db =
        //     engine_test::kv::new_engine(path_raft.to_str().unwrap(), None, ALL_CFS, None).unwrap();
        let raft_db = new_engine(path_kv.to_str().unwrap(), &[CF_DEFAULT]);
        let raft_db = RocksEngine::from_db(Arc::new(raft_db));

        let kv_db = new_engine(path_raft.to_str().unwrap(),  ALL_CFS);
        let kv_db = RocksEngine::from_db(Arc::new(kv_db));

        let engines = Engines::new(kv_db, raft_db.clone());

        let (tx, rx) = mpsc::channel();
        // let (r, _) = mpsc::sync_channel(1);
        let mut runner = Runner {
            gc_entries: Some(tx),
            engines,
            // ch: r,
            tasks: vec![],
        };

        // generate raft logs
        let region_id = 1;
        let mut raft_wb = raft_db.write_batch();
        for i in 0..100 {
            let k = keys::raft_log_key(region_id, i);
            // raft_wb.put(&k, b"entry").unwrap();
            raft_wb.put(&k, b"entry");
        }
        // raft_wb.write().unwrap();
        raft_wb.write_opt(true);

        let tbls = vec![
            (Task::gc(region_id, 0, 10), 10, (0, 10), (10, 100)),
            (Task::gc(region_id, 0, 50), 40, (0, 50), (50, 100)),
            (Task::gc(region_id, 50, 50), 0, (0, 50), (50, 100)),
            (Task::gc(region_id, 50, 60), 10, (0, 60), (60, 100)),
        ];

        for (task, expected_collectd, not_exist_range, exist_range) in tbls {
            runner.run(task);
            runner.flush();
            let res = rx.recv_timeout(Duration::from_secs(3)).unwrap();
            assert_eq!(res, expected_collectd);
            raft_log_must_not_exist(&raft_db, 1, not_exist_range.0, not_exist_range.1);
            raft_log_must_exist(&raft_db, 1, exist_range.0, exist_range.1);
        }
    }

    fn raft_log_must_not_exist(
        // raft_engine: &impl KvEngine,
        raft_engine: &RocksEngine,
        region_id: u64,
        start_idx: u64,
        end_idx: u64,
    ) {
        for i in start_idx..end_idx {
            let k = keys::raft_log_key(region_id, i);
            // assert!(raft_engine.get_value(&k).unwrap().is_none());
            assert!(raft_engine.get_value(&k).is_none());
        }
    }

    fn raft_log_must_exist(
        // raft_engine: &impl KvEngine,
        raft_engine: &RocksEngine,
        region_id: u64,
        start_idx: u64,
        end_idx: u64,
    ) {
        for i in start_idx..end_idx {
            let k = keys::raft_log_key(region_id, i);
            // assert!(raft_engine.get_value(&k).unwrap().is_some());
            assert!(raft_engine.get_value(&k).is_some());
        }
    }
}
