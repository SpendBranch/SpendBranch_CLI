//! What `login`, `logout`, `status` and `disable` need from `state.db`.
use super::{Result, Store};
use crate::contract::Source;
use crate::sources::outcome::SourceFacts;
use rusqlite::{OptionalExtension, TransactionBehavior, params};

impl Store {
    pub fn meta_delete(&self, key: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM meta WHERE key = ?1", [key])?;
        Ok(())
    }

    /// Turns collection off for a source. The cursor stays, so turning it on again does not
    /// read the same files from the start.
    pub fn disable_source(&self, source: Source) -> Result<()> {
        self.conn.execute(
            "UPDATE source SET enabled_since = NULL WHERE name = ?1",
            [source.name()],
        )?;
        Ok(())
    }

    pub fn disable_all_sources(&self) -> Result<()> {
        self.conn
            .execute("UPDATE source SET enabled_since = NULL", [])?;
        Ok(())
    }

    /// Everything waiting to be sent, and the send bookkeeping, goes: it belonged to another
    /// organization. Cursors and seen keys stay, so that data is not read again and sent to
    /// the new one.
    pub fn discard_buffer(&mut self) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for table in ["spool", "pending_coverage", "sync_state", "rejected"] {
            tx.execute(&format!("DELETE FROM {table}"), [])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn source_facts(&self, source: Source) -> Result<SourceFacts> {
        let name = source.name();
        let (enabled_since, last_read_at, untested_version) = self
            .conn
            .query_row(
                "SELECT enabled_since, last_read_at, untested_version FROM source WHERE name = ?1",
                [name],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?
            .unwrap_or_default();
        let count = |sql: &str| -> Result<u64> {
            let n: i64 = self.conn.query_row(sql, params![name], |r| r.get(0))?;
            Ok(u64::try_from(n).unwrap_or(0))
        };
        Ok(SourceFacts {
            enabled_since,
            last_read_at,
            untested_version,
            // Codex also keeps one cursor per thread: only the file cursors are files.
            tracked_files: count(
                "SELECT COUNT(*) FROM cursor WHERE source = ?1 AND key NOT LIKE 'thread:%'",
            )?,
            unreadable_files: count("SELECT COUNT(*) FROM unsupported_file WHERE source = ?1")?,
        })
    }

    /// Items the server refused, by code, since the last time that was cleared.
    pub fn rejected_counts(&self) -> Result<Vec<(String, u64)>> {
        let mut q = self
            .conn
            .prepare("SELECT code, count FROM rejected ORDER BY code")?;
        let rows = q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (code, n) = row?;
            out.push((code, u64::try_from(n).unwrap_or(0)));
        }
        Ok(out)
    }

    /// One send bookkeeping value (`lastSentAt`, `lastError`, `nextAttemptAt`...), written by `sync`.
    pub fn sync_get(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM sync_state WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()?)
    }
}
