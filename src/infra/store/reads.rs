//! What readers leave behind: the buffer, pending coverage, seen keys and cursors, all
//! committed together.
use super::{Result, Store};
use crate::contract::{CoverageReport, Source};
use crate::sources::outcome::{ReadOutcome, ReaderState};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

/// How long an event key is remembered.
const SEEN_KEEP_MS: i64 = 45 * 24 * 60 * 60 * 1000;

/// Buffer ceiling. Past it nothing is discarded: the reader records a `buffer_full` gap.
#[derive(Clone, Copy, Debug)]
pub struct SpoolLimits {
    pub items: u64,
    pub bytes: u64,
}

impl Default for SpoolLimits {
    fn default() -> Self {
        Self {
            items: 100_000,
            bytes: 50 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Commit {
    Stored,
    /// The buffer has no room: nothing was written, so the same data is read again later.
    Full,
}

impl Store {
    /// What a reader starts from. The seen keys are not loaded: they can be hundreds of
    /// thousands, and most reads touch only a few, so the reader asks one at a time.
    pub fn reader_state(&self, source: Source) -> Result<ReaderState<'_>> {
        let name = source.name();
        let mut cursors = std::collections::HashMap::new();
        let mut q = self
            .conn
            .prepare_cached("SELECT key, data FROM cursor WHERE source = ?1")?;
        for row in q.query_map([name], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })? {
            let (key, data) = row?;
            cursors.insert(key, data);
        }
        let mut unsupported = std::collections::HashSet::new();
        let mut q = self
            .conn
            .prepare_cached("SELECT path FROM unsupported_file WHERE source = ?1")?;
        for row in q.query_map([name], |r| r.get::<_, String>(0))? {
            unsupported.insert(row?);
        }
        let untested = self
            .conn
            .query_row(
                "SELECT untested_version FROM source WHERE name = ?1",
                [name],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        let conn = &self.conn;
        let is_seen = move |hash: u64| {
            // A failed lookup counts as "not seen": the worst case is sending an event twice,
            // which the server drops. The opposite mistake would lose it.
            conn.prepare_cached("SELECT 1 FROM seen WHERE source = ?1 AND key_hash = ?2")
                .and_then(|mut q| q.exists(params![name, hash as i64]))
                .unwrap_or(false)
        };
        Ok(ReaderState::new(cursors, unsupported, untested, is_seen))
    }

    /// How many event keys are remembered for a source.
    pub fn seen_count(&self, source: Source) -> Result<u64> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM seen WHERE source = ?1",
            [source.name()],
            |r| r.get(0),
        )?;
        Ok(n as u64)
    }

    /// Buffer, pending coverage, seen keys and cursors in ONE transaction. Either all of it
    /// is saved or none: a crash before the commit repeats the read (the server drops the
    /// repeat), a crash after it finds the cursor already moved.
    pub fn commit_read(&mut self, outcome: &ReadOutcome, limits: SpoolLimits) -> Result<Commit> {
        let name = outcome.source.name();
        let payloads = outcome
            .observations
            .iter()
            .map(serde_json::to_string)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        // Immediate: take the write lock now, waiting for it. A transaction that starts as a
        // read and later wants to write fails at once if another process wrote in between.
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !payloads.is_empty() {
            let (items, bytes): (i64, i64) = tx.query_row(
                "SELECT COUNT(*), COALESCE(SUM(LENGTH(payload) + 1), 0) FROM spool",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let added: u64 = payloads.iter().map(|p| p.len() as u64 + 1).sum();
            if items as u64 + payloads.len() as u64 > limits.items
                || bytes as u64 + added > limits.bytes
            {
                return Ok(Commit::Full); // the transaction is dropped: nothing is written
            }
        }
        {
            let mut insert = tx.prepare_cached("INSERT INTO spool(payload) VALUES(?1)")?;
            for payload in &payloads {
                insert.execute([payload])?;
            }
            let mut insert =
                tx.prepare_cached("INSERT INTO pending_coverage(payload) VALUES(?1)")?;
            for report in &outcome.coverage {
                insert.execute([serde_json::to_string(report)?])?;
            }
        }
        // Keys older than the window are not worth keeping: skip them instead of inserting
        // and deleting.
        let cutoff = (outcome.read_at.millis() - SEEN_KEEP_MS).max(0);
        {
            let mut insert = tx.prepare_cached(
                "INSERT OR IGNORE INTO seen(source, key_hash, occurred_ms) VALUES(?1, ?2, ?3)",
            )?;
            for (hash, at) in outcome.seen.iter().filter(|(_, at)| at.millis() >= cutoff) {
                insert.execute(params![name, *hash as i64, at.millis()])?;
            }
        }
        tx.execute(
            "DELETE FROM seen WHERE source = ?1 AND occurred_ms < ?2",
            params![name, cutoff],
        )?;
        {
            let mut delete =
                tx.prepare_cached("DELETE FROM cursor WHERE source = ?1 AND key = ?2")?;
            for key in &outcome.cursor_remove {
                delete.execute(params![name, key])?;
            }
            let mut upsert = tx.prepare_cached(
                "INSERT INTO cursor(source, key, data) VALUES(?1, ?2, ?3)
                 ON CONFLICT(source, key) DO UPDATE SET data = excluded.data",
            )?;
            for (key, data) in &outcome.cursor_set {
                upsert.execute(params![name, key, data])?;
            }
        }
        tx.execute("DELETE FROM unsupported_file WHERE source = ?1", [name])?;
        {
            let mut insert = tx.prepare_cached(
                "INSERT OR IGNORE INTO unsupported_file(source, path) VALUES(?1, ?2)",
            )?;
            for path in &outcome.unsupported_files {
                insert.execute(params![name, path])?;
            }
        }
        tx.execute(
            "INSERT INTO source(name, last_read_at, untested_version) VALUES(?1, ?2, ?3)
             ON CONFLICT(name) DO UPDATE SET last_read_at = excluded.last_read_at,
                                             untested_version = excluded.untested_version",
            params![name, outcome.read_at.to_string(), outcome.untested_version],
        )?;
        tx.commit()?;
        Ok(Commit::Stored)
    }

    /// Records gaps without touching cursors, e.g. `buffer_full` after a refused commit.
    pub fn add_pending_coverage(&mut self, reports: &[CoverageReport]) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        {
            let mut insert =
                tx.prepare_cached("INSERT INTO pending_coverage(payload) VALUES(?1)")?;
            for report in reports {
                insert.execute([serde_json::to_string(report)?])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Items waiting in the buffer and the coverage reports waiting to be sent.
    pub fn pending_counts(&self) -> Result<(u64, u64)> {
        let one = |sql: &str| -> Result<u64> {
            Ok(self.conn.query_row(sql, [], |r| r.get::<_, i64>(0))? as u64)
        };
        Ok((
            one("SELECT COUNT(*) FROM spool")?,
            one("SELECT COUNT(*) FROM pending_coverage")?,
        ))
    }
}
