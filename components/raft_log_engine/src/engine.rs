// Copyright 2019 TiKV Project Authors. Licensed under Apache-2.0.

use engine_traits::{RaftEngine, RaftLogBatch as RaftLogBatchTrait, Result};
use kvproto::raft_serverpb::RaftLocalState;
use raft::eraftpb::Entry;
use raft_engine::{EntryExt, Error as RaftEngineError, LogBatch, RaftLogEngine as RawRaftEngine};

#[derive(Clone)]
pub struct EntryExtTyped;

impl EntryExt<Entry> for EntryExtTyped {
    fn index(e: &Entry) -> u64 {
        e.index
    }
}

#[derive(Clone)]
pub struct RaftLogEngine(RawRaftEngine<Entry, EntryExtTyped>);

#[derive(Default)]
pub struct RaftLogBatch(LogBatch<Entry, EntryExtTyped>);

#[derive(Clone, Copy, Default)]
pub struct CacheStats {
    pub hit: usize,
    pub miss: usize,
    pub cache_size: usize,
}

const RAFT_LOG_STATE_KEY: &[u8] = b"R";

impl RaftLogBatchTrait for RaftLogBatch {
    fn append(&mut self, raft_group_id: u64, entries: Vec<Entry>) -> Result<()> {
        self.0.add_entries(raft_group_id, entries);
        Ok(())
    }

    fn cut_logs(&mut self, _: u64, _: u64, _: u64) {
        // It's unnecessary because overlapped entries can be handled in `append`.
    }

    fn put_raft_state(&mut self, raft_group_id: u64, state: &RaftLocalState) -> Result<()> {
        box_try!(self
            .0
            .put_msg(raft_group_id, RAFT_LOG_STATE_KEY.to_vec(), state));
        Ok(())
    }

    fn is_empty(&self) -> bool {
        self.0.items.is_empty()
    }
}

fn transfer_error(e: RaftEngineError) -> engine_traits::Error {
    match e {
        RaftEngineError::StorageCompacted => engine_traits::Error::EntriesCompacted,
        RaftEngineError::StorageUnavailable => engine_traits::Error::EntriesUnavailable,
        e => {
            let e = box_err!(e);
            engine_traits::Error::Other(e)
        }
    }
}

impl RaftEngine for RaftLogEngine {
    type LogBatch = RaftLogBatch;

    fn log_batch(&self, _capacity: usize) -> Self::LogBatch {
        RaftLogBatch::default()
    }

    fn sync(&self) -> Result<()> {
        box_try!(self.0.sync());
        Ok(())
    }

    fn get_raft_state(&self, raft_group_id: u64) -> Result<Option<RaftLocalState>> {
        let state = box_try!(self.0.get_msg(raft_group_id, RAFT_LOG_STATE_KEY));
        Ok(state)
    }

    fn get_entry(&self, raft_group_id: u64, index: u64) -> Result<Option<Entry>> {
        self.0
            .get_entry(raft_group_id, index)
            .map_err(|e| transfer_error(e))
    }

    fn fetch_entries_to(
        &self,
        raft_group_id: u64,
        begin: u64,
        end: u64,
        max_size: Option<usize>,
        to: &mut Vec<Entry>,
    ) -> Result<usize> {
        self.0
            .fetch_entries_to(raft_group_id, begin, end, max_size, to)
            .map_err(|e| transfer_error(e))
    }

    fn consume(&self, batch: &mut Self::LogBatch, sync: bool) -> Result<usize> {
        let ret = box_try!(self.0.write(&mut batch.0, sync));
        Ok(ret)
    }

    fn consume_and_shrink(
        &self,
        batch: &mut Self::LogBatch,
        sync: bool,
        _: usize,
        _: usize,
    ) -> Result<usize> {
        let ret = box_try!(self.0.write(&mut batch.0, sync));
        Ok(ret)
    }

    fn clean(
        &self,
        raft_group_id: u64,
        _: &RaftLocalState,
        batch: &mut RaftLogBatch,
    ) -> Result<()> {
        batch.0.clean_region(raft_group_id);
        Ok(())
    }

    fn append(&self, raft_group_id: u64, entries: Vec<Entry>) -> Result<usize> {
        let mut batch = Self::LogBatch::default();
        batch.0.add_entries(raft_group_id, entries);
        let ret = box_try!(self.0.write(&mut batch.0, false));
        Ok(ret)
    }

    fn put_raft_state(&self, raft_group_id: u64, state: &RaftLocalState) -> Result<()> {
        box_try!(self.0.put_msg(raft_group_id, RAFT_LOG_STATE_KEY, state));
        Ok(())
    }

    fn gc(&self, raft_group_id: u64, _from: u64, to: u64) -> Result<usize> {
        Ok(self.0.compact_to(raft_group_id, to) as usize)
    }

    fn purge_expired_files(&self) -> Result<Vec<u64>> {
        let ret = box_try!(self.0.purge_expired_files());
        Ok(ret)
    }

    fn has_builtin_entry_cache(&self) -> bool {
        true
    }

    fn gc_entry_cache(&self, raft_group_id: u64, to: u64) {
        self.0.compact_cache_to(raft_group_id, to)
    }
    /// Flush current cache stats.
    fn flush_stats(&self) -> Option<engine_traits::CacheStats> {
        let stat = self.0.flush_stats();
        Some(engine_traits::CacheStats {
            hit: stat.hit,
            miss: stat.miss,
            cache_size: stat.cache_size,
        })
    }

    fn stop(&self) {
        self.0.stop();
    }
}
