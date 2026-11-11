//! `state.db`: the single local SQLite file that holds all device state.
use super::private_fs;
use crate::contract::{CoverageReport, Iso, Source};
use crate::sources::outcome::{ReadOutcome, ReaderState};
use rusqlite::{Connection, OptionalExtension, params};
use std::{path::Path, time::Duration};

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

/// Index = schema version - 1. Never edit an applied entry: append a new one.
const MIGRATIONS: &[&str] = &["
    CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT) WITHOUT ROWID;
    CREATE TABLE sync_state(key TEXT PRIMARY KEY, value TEXT) WITHOUT ROWID;
    CREATE TABLE rejected(code TEXT PRIMARY KEY, count INTEGER NOT NULL) WITHOUT ROWID;
    CREATE TABLE source(
        name TEXT PRIMARY KEY, enabled_since TEXT, observed_until TEXT,
        last_read_at TEXT, untested_version TEXT) WITHOUT ROWID;
    CREATE TABLE cursor(source TEXT, key TEXT, data TEXT, PRIMARY KEY(source, key)) WITHOUT ROWID;
    CREATE TABLE unsupported_file(source TEXT, path TEXT, PRIMARY KEY(source, path)) WITHOUT ROWID;
    CREATE TABLE seen(
        source TEXT, key_hash INTEGER, occurred_at TEXT,
        PRIMARY KEY(source, key_hash)) WITHOUT ROWID;
    CREATE TABLE spool(id INTEGER PRIMARY KEY, payload TEXT NOT NULL);
    CREATE TABLE pending_coverage(id INTEGER PRIMARY KEY, payload TEXT NOT NULL);
"];

#[derive(Debug)]
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Opens (creating and migrating if needed) `<dir>/state.db`, owner-only, WAL.
    pub fn open(dir: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        private_fs::create_dir(dir)?;
        let path = dir.join("state.db");
        // Create it 0600 first: SQLite gives the -wal and -shm files the same mode.
        private_fs::open(&path, false)?;
        let mut conn = Connection::open(&path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
        migrate(&mut conn)?;
        Ok(Self { conn })
    }

    pub fn meta_get(&self, key: &str) -> rusqlite::Result<Option<String>> {
        self.conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()
    }

    pub fn meta_set(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES(?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Stable per installation: a UUID v4 created on first use.
    pub fn installation_id(&self) -> rusqlite::Result<String> {
        if let Some(id) = self.meta_get("installation_id")? {
            return Ok(id);
        }
        // INSERT OR IGNORE then re-read: two processes racing both end up with the winner's id.
        self.conn.execute(
            "INSERT OR IGNORE INTO meta(key, value) VALUES('installation_id', ?1)",
            [uuid::Uuid::new_v4().to_string()],
        )?;
        Ok(self.meta_get("installation_id")?.expect("just inserted"))
    }

    /// Turns collection on for a source. Keeps the cursor if it was on before.
    pub fn enable_source(&self, source: Source, since: Iso) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO source(name, enabled_since) VALUES(?1, ?2)
             ON CONFLICT(name) DO UPDATE SET enabled_since = excluded.enabled_since",
            params![source.name(), since.to_string()],
        )?;
        Ok(())
    }

    pub fn source_enabled(&self, source: Source) -> rusqlite::Result<bool> {
        let since: Option<Option<String>> = self
            .conn
            .query_row(
                "SELECT enabled_since FROM source WHERE name = ?1",
                [source.name()],
                |r| r.get(0),
            )
            .optional()?;
        Ok(since.flatten().is_some())
    }

    /// What a reader starts from: its cursors, the events already read and the files it
    /// could not read.
    pub fn reader_state(&self, source: Source) -> rusqlite::Result<ReaderState> {
        let name = source.name();
        let mut state = ReaderState::default();
        let mut q = self
            .conn
            .prepare("SELECT key, data FROM cursor WHERE source = ?1")?;
        for row in q.query_map([name], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })? {
            let (key, data) = row?;
            state.cursors.insert(key, data);
        }
        let mut q = self
            .conn
            .prepare("SELECT key_hash FROM seen WHERE source = ?1")?;
        for row in q.query_map([name], |r| r.get::<_, i64>(0))? {
            state.seen.insert(row? as u64);
        }
        let mut q = self
            .conn
            .prepare("SELECT path FROM unsupported_file WHERE source = ?1")?;
        for row in q.query_map([name], |r| r.get::<_, String>(0))? {
            state.unsupported_files.insert(row?);
        }
        state.untested_version = self
            .conn
            .query_row(
                "SELECT untested_version FROM source WHERE name = ?1",
                [name],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        Ok(state)
    }

    /// Buffer, pending coverage, seen keys and cursors in ONE transaction. Either all of it
    /// is saved or none: a crash before the commit repeats the read (the server drops the
    /// repeat), a crash after it finds the cursor already moved.
    pub fn commit_read(
        &mut self,
        outcome: &ReadOutcome,
        limits: SpoolLimits,
    ) -> Result<Commit, Box<dyn std::error::Error>> {
        let name = outcome.source.name();
        let tx = self.conn.transaction()?;
        let payloads = outcome
            .observations
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()?;
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
                return Ok(Commit::Full);
            }
        }
        for payload in &payloads {
            tx.execute("INSERT INTO spool(payload) VALUES(?1)", [payload])?;
        }
        for report in &outcome.coverage {
            tx.execute(
                "INSERT INTO pending_coverage(payload) VALUES(?1)",
                [serde_json::to_string(report)?],
            )?;
        }
        for (hash, at) in &outcome.seen {
            tx.execute(
                "INSERT OR IGNORE INTO seen(source, key_hash, occurred_at) VALUES(?1, ?2, ?3)",
                params![name, *hash as i64, at.to_string()],
            )?;
        }
        let cutoff =
            Iso::from_millis((outcome.read_at.millis() - SEEN_KEEP_MS).max(0)).expect("in range");
        tx.execute(
            "DELETE FROM seen WHERE source = ?1 AND occurred_at < ?2",
            params![name, cutoff.to_string()],
        )?;
        for key in &outcome.cursor_remove {
            tx.execute(
                "DELETE FROM cursor WHERE source = ?1 AND key = ?2",
                params![name, key],
            )?;
        }
        for (key, data) in &outcome.cursor_set {
            tx.execute(
                "INSERT INTO cursor(source, key, data) VALUES(?1, ?2, ?3)
                 ON CONFLICT(source, key) DO UPDATE SET data = excluded.data",
                params![name, key, data],
            )?;
        }
        tx.execute("DELETE FROM unsupported_file WHERE source = ?1", [name])?;
        for path in &outcome.unsupported_files {
            tx.execute(
                "INSERT OR IGNORE INTO unsupported_file(source, path) VALUES(?1, ?2)",
                params![name, path],
            )?;
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
    pub fn add_pending_coverage(
        &mut self,
        reports: &[CoverageReport],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let tx = self.conn.transaction()?;
        for report in reports {
            tx.execute(
                "INSERT INTO pending_coverage(payload) VALUES(?1)",
                [serde_json::to_string(report)?],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Items waiting in the buffer and the coverage reports waiting to be sent.
    pub fn pending_counts(&self) -> rusqlite::Result<(u64, u64)> {
        let one = |sql: &str| {
            self.conn
                .query_row(sql, [], |r| r.get::<_, i64>(0))
                .map(|n| n as u64)
        };
        Ok((
            one("SELECT COUNT(*) FROM spool")?,
            one("SELECT COUNT(*) FROM pending_coverage")?,
        ))
    }

    /// A device is connected once pairing stored its `device_id`.
    pub fn is_connected(&self) -> rusqlite::Result<bool> {
        Ok(self.meta_get("device_id")?.is_some())
    }
}

fn migrate(conn: &mut Connection) -> Result<(), Box<dyn std::error::Error>> {
    let current: usize = conn
        .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?
        .try_into()?;
    if current > MIGRATIONS.len() {
        return Err(format!(
            "state.db is from a newer spendbranch (schema {current}, this build knows {}); update the CLI",
            MIGRATIONS.len()
        )
        .into());
    }
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (i + 1) as i64)?;
        tx.commit()?;
    }
    Ok(())
}
