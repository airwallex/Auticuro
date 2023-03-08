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

use crate::errors::Result;
use crate::keys;
use kvproto::raft_serverpb::RaftLocalState;
use protobuf::Message;
use raft::eraftpb::Entry;
use rocksdb::{
    CFHandle, ColumnFamilyOptions, DBIterator, DBOptions, DBVector, ReadOptions, SeekKey, Writable,
    WriteBatch, WriteOptions, DB,
};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use tikv_util::keybuilder::KeyBuilder;
use tikv_util::time::Instant;
use tikv_util::{debug, trace, warn};

const WRITE_BATCH_MAX_KEYS: usize = 256;
const RAFT_LOG_MULTI_GET_CNT: u64 = 8;

// FIXME: This should live somewhere else
pub const DATA_KEY_PREFIX_LEN: usize = 1;

pub type CfName = &'static str;

pub const CF_DEFAULT: CfName = "default";
pub const CF_RAFT: CfName = "raft";
pub const ALL_CFS: &[CfName] = &[CF_DEFAULT, CF_RAFT];

#[derive(Clone, Debug)]
pub struct RocksEngine {
    db: Arc<DB>,
    shared_block_cache: bool,
}

impl RocksEngine {
    pub fn from_db(db: Arc<DB>) -> Self {
        RocksEngine {
            db,
            shared_block_cache: false,
        }
    }

    pub fn as_inner(&self) -> &Arc<DB> {
        &self.db
    }

    pub fn set_shared_block_cache(&mut self, enable: bool) {
        self.shared_block_cache = enable;
    }

    pub fn sync(&self) {
        self.db.sync_wal().unwrap();
    }

    pub fn path(&self) -> &str {
        self.db.path()
    }
}

impl RocksEngine {
    pub fn get_value(&self, key: &[u8]) -> Option<DBVector> {
        let mut opt = ReadOptions::default();
        self.db.get_opt(key, &opt).unwrap()
    }

    pub fn get_value_cf(&self, cf: &str, key: &[u8]) -> Option<DBVector> {
        let mut opt = ReadOptions::default();
        let handle = get_cf_handle(&self.db, cf);
        self.db.get_cf_opt(handle, key, &opt).unwrap()
    }

    pub fn get_msg<M: protobuf::Message + Default>(&self, key: &[u8]) -> Option<M> {
        if let Some(value) = self.get_value(key) {
            let mut m = M::default();
            m.merge_from_bytes(&value).unwrap();
            Some(m)
        } else {
            None
        }
    }

    pub fn get_msg_cf<M: protobuf::Message + Default>(&self, cf: &str, key: &[u8]) -> Option<M> {
        if let Some(value) = self.get_value_cf(cf, key) {
            let mut m = M::default();
            m.merge_from_bytes(&value).unwrap();
            Some(m)
        } else {
            None
        }
    }
}

impl RocksEngine {
    pub fn put(&self, key: &[u8], value: &[u8]) {
        self.db.put(key, value).unwrap();
    }

    pub fn put_cf(&self, cf: &str, key: &[u8], value: &[u8]) {
        let handle = get_cf_handle(&self.db, cf);
        self.db.put_cf(handle, key, value).unwrap();
    }

    pub fn put_msg<M: protobuf::Message>(&self, key: &[u8], m: &M) {
        self.put(key, &m.write_to_bytes().unwrap());
    }

    pub fn put_msg_cf<M: protobuf::Message>(&self, cf: &str, key: &[u8], m: &M) {
        self.put_cf(cf, key, &m.write_to_bytes().unwrap());
    }

    pub fn write_batch(&self) -> RocksWriteBatch {
        RocksWriteBatch::new(Arc::clone(&self.as_inner()))
    }

    pub fn write_batch_with_cap(&self, cap: usize) -> RocksWriteBatch {
        RocksWriteBatch::with_capacity(Arc::clone(self.as_inner()), cap)
    }
}

#[derive(Clone)]
pub struct IterOptions {
    lower_bound: Option<KeyBuilder>,
    upper_bound: Option<KeyBuilder>,
}

impl IterOptions {
    pub fn new(lower_bound: Option<KeyBuilder>, upper_bound: Option<KeyBuilder>) -> IterOptions {
        IterOptions {
            lower_bound,
            upper_bound,
        }
    }

    #[inline]
    pub fn build_bounds(self) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
        let lower = self.lower_bound.map(KeyBuilder::build);
        let upper = self.upper_bound.map(KeyBuilder::build);
        (lower, upper)
    }
}

impl Default for IterOptions {
    fn default() -> IterOptions {
        IterOptions {
            lower_bound: None,
            upper_bound: None,
        }
    }
}

fn build_read_opts(iter_opts: IterOptions) -> ReadOptions {
    let mut opts = ReadOptions::new();

    opts.fill_cache(true);
    opts.set_total_order_seek(true);

    let (lower, upper) = iter_opts.build_bounds();
    if let Some(lower) = lower {
        opts.set_iterate_lower_bound(lower);
    }
    if let Some(upper) = upper {
        opts.set_iterate_upper_bound(upper);
    }

    opts
}

impl RocksEngine {
    fn iterator_opt(&self, opts: IterOptions) -> DBIterator<Arc<DB>> {
        let opt = build_read_opts(opts);
        DBIterator::new(self.db.clone(), opt)
    }

    fn iterator_cf_opt(&self, cf: &str, opts: IterOptions) -> DBIterator<Arc<DB>> {
        let handle = get_cf_handle(&self.db, cf);
        let opt = build_read_opts(opts);
        DBIterator::new_cf(self.db.clone(), handle, opt)
    }

    fn iterator(&self) -> DBIterator<Arc<DB>> {
        self.iterator_opt(IterOptions::default())
    }

    fn iterator_cf(&self, cf: &str) -> DBIterator<Arc<DB>> {
        self.iterator_cf_opt(cf, IterOptions::default())
    }

    pub fn scan<F>(&self, start_key: &[u8], end_key: &[u8], _fill_cache: bool, f: F) -> Result<()>
        where
            F: FnMut(&[u8], &[u8]) -> Result<bool>,
    {
        let start = KeyBuilder::from_slice(start_key, DATA_KEY_PREFIX_LEN, 0);
        let end = KeyBuilder::from_slice(end_key, DATA_KEY_PREFIX_LEN, 0);
        let iter_opt = IterOptions::new(Some(start), Some(end));
        scan_impl(self.iterator_opt(iter_opt), start_key, f)
    }

    // like `scan`, only on a specific column family.
    fn scan_cf<F>(
        &self,
        cf: &str,
        start_key: &[u8],
        end_key: &[u8],
        fill_cache: bool,
        f: F,
    ) -> Result<()>
        where
            F: FnMut(&[u8], &[u8]) -> Result<bool>,
    {
        let start = KeyBuilder::from_slice(start_key, DATA_KEY_PREFIX_LEN, 0);
        let end = KeyBuilder::from_slice(end_key, DATA_KEY_PREFIX_LEN, 0);
        let iter_opt = IterOptions::new(Some(start), Some(end));
        scan_impl(self.iterator_cf_opt(cf, iter_opt), start_key, f)
    }

    // Seek the first key >= given key, if not found, return None.
    fn seek(&self, key: &[u8]) -> Result<Option<(Vec<u8>, Vec<u8>)>> {
        // let mut iter = self.iterator()?;
        let mut iter = self.iterator();
        if iter.seek(SeekKey::Key(key))? {
            let (k, v) = (iter.key().to_vec(), iter.value().to_vec());
            return Ok(Some((k, v)));
        }
        Ok(None)
    }
}

fn scan_impl<F>(mut it: DBIterator<Arc<DB>>, start_key: &[u8], mut f: F) -> Result<()>
    where
        F: FnMut(&[u8], &[u8]) -> Result<bool>,
{
    let mut remained = it.seek(SeekKey::Key(start_key))?;
    while remained {
        remained = f(it.key(), it.value())? && it.next()?;
    }
    Ok(())
}

impl RocksEngine {
    pub fn get_raft_state(&self, raft_group_id: u64) -> Option<RaftLocalState> {
        let key = keys::raft_state_key(raft_group_id);
        self.get_msg_cf(CF_DEFAULT, &key)
    }

    pub fn get_entry(&self, raft_group_id: u64, index: u64) -> Option<Entry> {
        let key = keys::raft_log_key(raft_group_id, index);
        self.get_msg_cf(CF_DEFAULT, &key)
    }

    pub fn fetch_entries_to(
        &self,
        region_id: u64,
        low: u64,
        high: u64,
        max_size: Option<usize>,
        buf: &mut Vec<Entry>,
    ) -> usize {
        // Entries between [low, high) might exist only if high >= low + 1
        if high <= low {
            warn!("RocksEngine::fetch_entries_to() received illegal request: high({}) <= low({})", high, low);
            return 0;
        }

        let t = Instant::now_coarse();
        let (max_size, mut total_size, mut count) = (max_size.unwrap_or(usize::MAX), 0, 0);

        if high - low <= RAFT_LOG_MULTI_GET_CNT {
            // If election happens in inactive regions, they will just try to fetch one empty log.
            for i in low..high {
                if total_size > 0 && total_size >= max_size {
                    break;
                }
                let key = keys::raft_log_key(region_id, i);
                match self.get_value(&key) {
                    Some(v) => {
                        let mut entry = Entry::default();
                        entry.merge_from_bytes(&v).unwrap();
                        assert_eq!(entry.get_index(), i);
                        buf.push(entry);
                        total_size += v.len();
                        count += 1;
                    }
                    _ => {
                        panic!("failed to fetch raft log index {}", i);
                    }
                }
            }
            debug!(
                "RocksEngine::fetch_entries_to(), by multi-get, elapsed {:?}, entry count {}, total size {}KB",
                t.elapsed(),
                count,
                total_size as f32 / 1024.0
            );
            return count;
        }

        let (mut check_compacted, mut next_index) = (true, low);
        let start_key = keys::raft_log_key(region_id, low);
        let end_key = keys::raft_log_key(region_id, high);
        self.scan(
            &start_key,
            &end_key,
            true, // fill_cache
            |_, value| {
                let mut entry = Entry::default();
                entry.merge_from_bytes(value)?;

                if check_compacted {
                    if entry.get_index() != low {
                        // May meet gap or has been compacted.
                        // return Ok(false);
                        panic!(
                            "failed to fetch raft log index {}, get {}",
                            low,
                            entry.get_index()
                        );
                    }
                    check_compacted = false;
                } else {
                    assert_eq!(entry.get_index(), next_index);
                }
                next_index += 1;

                buf.push(entry);
                total_size += value.len();
                count += 1;
                Ok(total_size < max_size)
            },
        )
            .unwrap();

        // If we get the correct number of entries, returns.
        // Or the total size almost exceeds max_size, returns.
        if count == (high - low) as usize || total_size >= max_size {
            debug!(
                "RocksEngine::fetch_entries_to(), by scan, elapsed {:?}, entry count {}, total size {}KB",
                t.elapsed(),
                count,
                total_size as f32 / 1024.0
            );
            return count;
        }

        // Here means we don't fetch enough entries.
        warn!(
            "failed to fetch raft log index, count {}, high {}, low {}, total_size {}, max_size {}",
            count, high, low, total_size, max_size
        );

        // Although buf is unlikely to be non-zero, clear it to 0 in case inconsistency between non-empty buf and returned value 0
        // Ideally, with proper error handling in the future, Result<usize> should be the return type as in tikv and below code can be removed
        if buf.len() != 0 {
            warn!("Clear the buf(size={}) to 0 upon illegal request.", buf.len());
            buf.clear();
        }
        return 0;
    }
}

impl RocksEngine {
    pub fn log_batch(&self, capacity: usize) -> RocksWriteBatch {
        RocksWriteBatch::with_capacity(self.as_inner().clone(), capacity)
    }

    pub fn consume(&self, batch: &mut RocksWriteBatch, sync_log: bool) -> usize {
        let bytes = batch.data_size();
        batch.write_opt(sync_log);
        batch.clear();
        bytes
    }

    pub fn append(&self, raft_group_id: u64, entries: Vec<Entry>) -> usize {
        let mut wb = RocksWriteBatch::new(self.as_inner().clone());
        let buf = Vec::with_capacity(1024);
        wb.append_impl(raft_group_id, &entries, buf);
        self.consume(&mut wb, false)
    }

    pub fn put_raft_state(&self, raft_group_id: u64, state: &RaftLocalState) {
        self.put_msg(&keys::raft_state_key(raft_group_id), state)
    }

    pub fn gc(&self, raft_group_id: u64, mut from: u64, to: u64) -> Result<usize> {
        if from >= to {
            return Ok(0);
        }
        if from == 0 {
            let start_key = keys::raft_log_key(raft_group_id, 0);
            let prefix = keys::raft_log_prefix(raft_group_id);
            match self.seek(&start_key)? {
                // Some((k, _)) if k.starts_with(&prefix) => from = box_try!(keys::raft_log_index(&k)),
                Some((k, _)) if k.starts_with(&prefix) => from = keys::raft_log_index(&k),
                // No need to gc.
                _ => return Ok(0),
            }
        }

        let mut raft_wb = self.write_batch_with_cap(4 * 1024);
        for idx in from..to {
            let key = keys::raft_log_key(raft_group_id, idx);
            // raft_wb.delete(&key)?;
            raft_wb.delete(&key);
            // if raft_wb.count() >= Self::WRITE_BATCH_MAX_KEYS {
            if raft_wb.count() >= 256 {
                // raft_wb.write()?;
                raft_wb.write_opt(false);
                raft_wb.clear();
            }
        }

        // TODO: disable WAL here.
        // if !WriteBatch::is_empty(&raft_wb) {
        if !RocksWriteBatch::is_empty(&raft_wb) {
            // raft_wb.write()?;
            raft_wb.write_opt(false);
        }
        Ok((to - from) as usize)
    }
}

pub fn get_cf_handle<'a>(db: &'a DB, cf: &str) -> &'a CFHandle {
    db.cf_handle(cf).unwrap()
}

pub struct RocksWriteBatch {
    db: Arc<DB>,
    wb: WriteBatch,
}

impl RocksWriteBatch {
    pub fn new(db: Arc<DB>) -> RocksWriteBatch {
        RocksWriteBatch {
            db,
            wb: WriteBatch::default(),
        }
    }

    pub fn as_inner(&self) -> &WriteBatch {
        &self.wb
    }

    pub fn with_capacity(db: Arc<DB>, cap: usize) -> RocksWriteBatch {
        let wb = if cap == 0 {
            WriteBatch::default()
        } else {
            WriteBatch::with_capacity(cap)
        };
        RocksWriteBatch { db, wb }
    }

    pub fn get_db(&self) -> &DB {
        self.db.as_ref()
    }

    pub fn write_opt(&self, sync: bool) {
        // self.get_db().write(self.as_inner()).unwrap();
        let mut opt = WriteOptions::default();
        opt.set_sync(sync);
        let t = Instant::now_coarse();
        self.get_db().write_opt(self.as_inner(), &opt).unwrap();
        debug!(
            "RocksWriteBatch::write() elapsed {:?}, entry count {}, total size {} KB, sync {}",
            t.elapsed(),
            self.wb.count(),
            self.wb.data_size() as f32 / 1024.0,
            sync
        );
    }

    pub fn data_size(&self) -> usize {
        self.wb.data_size()
    }

    pub fn count(&self) -> usize {
        self.wb.count()
    }

    pub fn is_empty(&self) -> bool {
        self.wb.is_empty()
    }

    pub fn should_write_to_engine(&self) -> bool {
        self.wb.count() > WRITE_BATCH_MAX_KEYS
    }

    pub fn clear(&mut self) {
        self.wb.clear();
    }

    pub fn set_save_point(&mut self) {
        self.wb.set_save_point();
    }

    pub fn pop_save_point(&mut self) {
        self.wb.pop_save_point().unwrap();
    }

    pub fn rollback_to_save_point(&mut self) {
        self.wb.rollback_to_save_point().unwrap()
    }
}

impl RocksWriteBatch {
    pub fn put(&mut self, key: &[u8], value: &[u8]) {
        self.wb.put(key, value).unwrap();
    }

    pub fn put_cf(&mut self, cf: &str, key: &[u8], value: &[u8]) {
        let handle = get_cf_handle(self.db.as_ref(), cf);
        self.wb.put_cf(handle, key, value).unwrap();
    }

    pub fn delete(&mut self, key: &[u8]) {
        self.wb.delete(key).unwrap();
    }

    pub fn delete_cf(&mut self, cf: &str, key: &[u8]) {
        let handle = get_cf_handle(self.db.as_ref(), cf);
        self.wb.delete_cf(handle, key).unwrap();
    }

    pub fn put_msg<M: protobuf::Message>(&mut self, key: &[u8], m: &M) {
        self.put(key, &m.write_to_bytes().unwrap())
    }

    pub fn put_msg_cf<M: protobuf::Message>(&mut self, cf: &str, key: &[u8], m: &M) {
        self.put_cf(cf, key, &m.write_to_bytes().unwrap())
    }
}

impl RocksWriteBatch {
    pub fn append(&mut self, raft_group_id: u64, entries: Vec<Entry>) {
        if let Some(max_size) = entries.iter().map(|e| e.compute_size()).max() {
            let ser_buf = Vec::with_capacity(max_size as usize);
            self.append_impl(raft_group_id, &entries, ser_buf);
        }
    }

    pub fn cut_logs(&mut self, raft_group_id: u64, from: u64, to: u64) {
        for index in from..to {
            let key = keys::raft_log_key(raft_group_id, index);
            self.delete(&key);
        }
    }

    pub fn put_raft_state(&mut self, raft_group_id: u64, state: &RaftLocalState) {
        self.put_msg(&keys::raft_state_key(raft_group_id), state)
    }

    fn append_impl(&mut self, raft_group_id: u64, entries: &[Entry], mut ser_buf: Vec<u8>) {
        for entry in entries {
            let key = keys::raft_log_key(raft_group_id, entry.get_index());
            ser_buf.clear();
            entry.write_to_vec(&mut ser_buf).unwrap();
            self.put(&key, &ser_buf);

            trace!("append entry";
                "index" => entry.get_index(),
                "term" => entry.get_term(),
                "db_path" => self.get_db().path()); // debug
        }
    }
}

pub struct CFOptions<'a> {
    cf: &'a str,
    options: ColumnFamilyOptions,
}

impl<'a> CFOptions<'a> {
    pub fn new(cf: &'a str, options: ColumnFamilyOptions) -> CFOptions<'a> {
        CFOptions { cf, options }
    }
}

pub fn new_engine(path: &str, cfs: &[&str]) -> DB {
    let mut db_opts = DBOptions::new();
    let mut cf_opts = Vec::with_capacity(cfs.len());

    for cf in cfs {
        cf_opts.push(CFOptions::new(*cf, ColumnFamilyOptions::new()));
    }
    new_engine_opt(path, db_opts, cf_opts)
}

pub fn new_engine_opt(path: &str, mut db_opt: DBOptions, cfs_opts: Vec<CFOptions<'_>>) -> DB {
    // Creates a new db if it doesn't exist.
    if !db_exist(path) {
        db_opt.create_if_missing(true);

        let mut cfs_v = vec![];
        let mut cf_opts_v = vec![];
        if let Some(x) = cfs_opts.iter().find(|x| x.cf == CF_DEFAULT) {
            cfs_v.push(x.cf);
            cf_opts_v.push(x.options.clone());
        }
        let mut db = DB::open_cf(db_opt, path, cfs_v.into_iter().zip(cf_opts_v).collect()).unwrap();
        for x in cfs_opts {
            if x.cf == CF_DEFAULT {
                continue;
            }
            db.create_cf((x.cf, x.options)).unwrap();
        }

        return db;
    }

    db_opt.create_if_missing(false);

    let mut cfs_v = vec![];
    let mut cfs_opts_v = vec![];
    for mut x in cfs_opts {
        cfs_v.push(x.cf);
        cfs_opts_v.push(x.options);
    }

    DB::open_cf(db_opt, path, cfs_v.into_iter().zip(cfs_opts_v).collect()).unwrap()
}

pub fn db_exist(path: &str) -> bool {
    let path = Path::new(path);
    if !path.exists() || !path.is_dir() {
        return false;
    }
    let current_file_path = path.join("CURRENT");
    if !current_file_path.exists() || !current_file_path.is_file() {
        return false;
    }

    // If path is not an empty directory, and current file exists, we say db exists. If path is not an empty directory
    // but db has not been created, `DB::list_column_families` fails and we can clean up
    // the directory by this indication.
    fs::read_dir(&path).unwrap().next().is_some()
}

#[derive(Clone, Debug)]
pub struct Engines {
    pub kv: RocksEngine,
    pub raft: RocksEngine,
}

impl Engines {
    pub fn new(kv_engine: RocksEngine, raft_engine: RocksEngine) -> Self {
        Engines {
            kv: kv_engine,
            raft: raft_engine,
        }
    }

    // pub fn write_kv(&self, wb: &RocksWriteBatch) {
    //     wb.write();
    // }
    //
    // pub fn write_kv_opt(&self, wb: &RocksWriteBatch, opts: &WriteOptions) {
    //     wb.write_opt(opts);
    // }

    pub fn sync_kv(&self) {
        self.kv.sync();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use tempfile::Builder;
    use kvproto::metapb::Region;
    use kvproto::raft_serverpb::RaftLocalState;

    fn mock_rocks_engine(cf: &str, dir: &str) -> RocksEngine {
        let dir = Builder::new().prefix(dir).tempdir().unwrap();
        let engine = new_engine(dir.path().to_str().unwrap(), &[cf]);
        RocksEngine::from_db(Arc::new(engine))
    }

    #[test]
    fn test_engine_base() {
        let cf = "cf";
        let dir = Builder::new().prefix("test-engine-base").tempdir().unwrap();
        let engine = new_engine(dir.path().to_str().unwrap(), &[cf]);
        let mut rocks_engine = RocksEngine::from_db(Arc::new(engine));

        assert_eq!(rocks_engine.path(), dir.path().to_str().unwrap());

        rocks_engine.set_shared_block_cache(true);
        assert!(rocks_engine.shared_block_cache);

        let mut r = Region::default();
        r.set_id(10);

        let key = b"key";
        rocks_engine.put_msg(key, &r);
        rocks_engine.put_msg_cf(cf, key, &r);

        let mut r1: Region = rocks_engine.get_msg(key).unwrap();
        assert_eq!(r, r1);
        let r1_cf: Region = rocks_engine.get_msg_cf(cf, key).unwrap();
        assert_eq!(r, r1_cf);

        r.set_id(11);
        rocks_engine.put_msg(key, &r);
        r1 = rocks_engine.get_msg(key).unwrap();
        assert_eq!(r1.get_id(), 11);

        let b: Option<Region> = rocks_engine.get_msg(b"missing_key");
        assert!(b.is_none());

        let result: std::thread::Result<Option<Region>> = std::panic::catch_unwind(|| { rocks_engine.get_msg_cf("missing_cfg", key) });
        assert!(result.is_err());
    }

    #[test]
    fn test_write_batch_base() {
        let cf = "cf";
        let mut write_batch = mock_rocks_engine(cf, "test-write-batch-base").write_batch();

        assert!(write_batch.is_empty());
        assert!(!write_batch.should_write_to_engine());

        write_batch.set_save_point();

        let key = b"a1";
        let value = b"v1";
        write_batch.put(key, value);
        write_batch.put_cf(cf, key, value);
        assert_eq!(write_batch.count(), 2);

        write_batch.rollback_to_save_point();

        write_batch.delete(key);
        write_batch.delete_cf(cf, key);
        assert_eq!(write_batch.count(), 2);

        write_batch.set_save_point();
        write_batch.pop_save_point();
    }

    #[test]
    fn test_scan() {
        let cf = "cf";
        let rocks_engine = mock_rocks_engine(cf, "scan");

        rocks_engine.put(b"a1", b"v1");
        rocks_engine.put(b"a2", b"v2");
        rocks_engine.put_cf(cf, b"a1", b"v1");
        rocks_engine.put_cf(cf, b"a2", b"v22");

        let mut data = vec![];
        rocks_engine
            .scan(b"", &[0xFF, 0xFF], false, |key, value| {
                data.push((key.to_vec(), value.to_vec()));
                Ok(true)
            })
            .unwrap();

        assert_eq!(
            data,
            vec![
                (b"a1".to_vec(), b"v1".to_vec()),
                (b"a2".to_vec(), b"v2".to_vec()),
            ]
        );
        data.clear();

        rocks_engine
            .scan_cf(cf, b"", &[0xFF, 0xFF], false, |key, value| {
                data.push((key.to_vec(), value.to_vec()));
                Ok(true)
            })
            .unwrap();
        assert_eq!(
            data,
            vec![
                (b"a1".to_vec(), b"v1".to_vec()),
                (b"a2".to_vec(), b"v22".to_vec()),
            ]
        );
        data.clear();
    }

    #[test]
    fn test_engine_log() {
        let rocks_engine = mock_rocks_engine("cf", "test-engine-state");

        let mock_group = 5;
        let low = 0;
        let high = 3;
        let mut entries = vec![];

        for i in low..high {
            let mut entry = Entry::default();
            entry.set_index(i);
            entry.set_data(Bytes::from(b"abc".as_slice()));
            entries.push(entry);
        }

        rocks_engine.append(mock_group, entries.clone());

        for i in 0..high {
            assert_eq!(rocks_engine.get_entry(mock_group, i).as_ref(), entries.get(i as usize));
        }

        let size = rocks_engine.fetch_entries_to(mock_group, low, high, None,  &mut vec![]);
        assert_eq!(size, high as usize);

        // case of fetched size larger than max size
        let size = rocks_engine.fetch_entries_to(mock_group, low, high, Some(0), &mut vec![]);
        assert_eq!(size, 1);

        let size = rocks_engine.fetch_entries_to(mock_group, low, high + RAFT_LOG_MULTI_GET_CNT + 1, Some(0), &mut vec![]);
        assert_eq!(size, 1);

        // No entries will be fetched if high <= low
        let size = rocks_engine.fetch_entries_to(mock_group, low, low, Some(0), &mut vec![]);
        assert_eq!(size, 0);

        // expect panic if request non-exist group id
        let res = std::panic::catch_unwind(|| { rocks_engine.fetch_entries_to(mock_group + 1, low, high, None, &mut vec![]) });
        assert!(res.is_err());

        let raft_state = RaftLocalState::default();
        assert!(rocks_engine.get_raft_state(mock_group).is_none());
        rocks_engine.put_raft_state(mock_group, &raft_state);
        assert!(rocks_engine.get_raft_state(mock_group).is_some());
    }

    #[test]
    fn test_write_batch_log() {
        let mut write_batch = mock_rocks_engine("cf", "test-write-batch-state").write_batch();

        let mock_group = 5;
        let low = 0;
        let high = 3;
        let mut entries = vec![];

        for i in low..high {
            let mut entry = Entry::default();
            entry.set_index(i);
            entry.set_data(Bytes::from(b"abc".as_slice()));
            entries.push(entry);
        }

        write_batch.append(mock_group, entries.clone());

        write_batch.cut_logs(mock_group, low, low + 1);
        assert_eq!(write_batch.count(), (high - low + 1) as usize);
    }
}
