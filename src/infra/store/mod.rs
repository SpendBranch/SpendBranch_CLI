//! `state.db`: the single local SQLite file that holds all device state.
//! This file opens and migrates the database and keeps small facts (`meta`, sources);
//! what readers leave behind (buffer, cursors, seen keys) is in `reads`.
mod account;
mod outbox;
mod reads;

use super::private_fs;
use crate::contract::{Iso, Source};
use rusqlite::{Connection, ErrorCode, OptionalExtension, TransactionBehavior, params};
use std::{
    fmt, io,
    path::Path,
    thread,
    time::{Duration, Instant},
};

pub use outbox::{Batch, Change, Queued};
pub use reads::{Commit, SpoolLimits};

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Sqlite(rusqlite::Error),
    Json(serde_json::Error),
    /// `state.db` was written by a newer build; opening it could corrupt it.
    NewerSchema {
        found: i64,
        known: usize,
    },
    /// `user_version` is negative: the file was edited by hand.
    BadSchemaVersion(i64),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Sqlite(e) => write!(f, "{e}"),
            Error::Json(e) => write!(f, "{e}"),
            Error::NewerSchema { found, known } => write!(
                f,
                "state.db is from a newer spendbranch (schema {found}, this build knows {known}); update the CLI"
            ),
            Error::BadSchemaVersion(found) => {
                write!(f, "state.db has an invalid schema version ({found})")
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            Error::Sqlite(e) => Some(e),
            Error::Json(e) => Some(e),
            Error::NewerSchema { .. } | Error::BadSchemaVersion(_) => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}
impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error::Sqlite(e)
    }
}
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Json(e)
    }
}

/// Index = schema version - 1. Never edit an applied entry: append a new one.
const MIGRATIONS: &[&str] = &[
    "
    CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT) WITHOUT ROWID;
    CREATE TABLE sync_state(key TEXT PRIMARY KEY, value TEXT) WITHOUT ROWID;
    CREATE TABLE rejected(code TEXT PRIMARY KEY, count INTEGER NOT NULL) WITHOUT ROWID;
    CREATE TABLE source(
        name TEXT PRIMARY KEY, enabled_since TEXT, observed_until TEXT,
        last_read_at TEXT, untested_version TEXT) WITHOUT ROWID;
    CREATE TABLE cursor(source TEXT, key TEXT, data TEXT, PRIMARY KEY(source, key)) WITHOUT ROWID;
    CREATE TABLE unsupported_file(source TEXT, path TEXT, PRIMARY KEY(source, path)) WITHOUT ROWID;
    CREATE TABLE seen(
        source TEXT, key_hash INTEGER, occurred_ms INTEGER NOT NULL,
        PRIMARY KEY(source, key_hash)) WITHOUT ROWID;
    CREATE TABLE spool(id INTEGER PRIMARY KEY, payload TEXT NOT NULL);
    CREATE TABLE pending_coverage(id INTEGER PRIMARY KEY, payload TEXT NOT NULL);
",
    "
    -- The age of a buffered item (unix ms of when it entered): the 24 h cap on CLOCK_SKEW.
    -- What was already there counts as entering now.
    -- AUTOINCREMENT: an id is never used twice, so an item put back lands after everything
    -- this run found, even if the table was emptied first.
    CREATE TABLE spool_new(id INTEGER PRIMARY KEY AUTOINCREMENT, payload TEXT NOT NULL,
        queued_at INTEGER NOT NULL);
    INSERT INTO spool_new(id, payload, queued_at)
        SELECT id, payload, CAST(strftime('%s', 'now') AS INTEGER) * 1000 FROM spool;
    DROP TABLE spool;
    ALTER TABLE spool_new RENAME TO spool;
",
];

const DB_FILE: &str = "state.db";

#[derive(Debug)]
pub struct Store {
    pub(super) conn: Connection,
}

impl Store {
    /// Whether `<dir>/state.db` exists yet, without creating it.
    pub fn exists(dir: &Path) -> bool {
        dir.join(DB_FILE).exists()
    }

    /// Opens (creating and migrating if needed) `<dir>/state.db`, owner-only, WAL.
    /// Safe to call from several processes at once (a hook and the scheduler, say).
    pub fn open(dir: &Path) -> Result<Self> {
        private_fs::create_dir(dir)?;
        let path = dir.join(DB_FILE);
        // Create it 0600 first: SQLite gives the -wal and -shm files the same mode.
        private_fs::open(&path, false)?;
        let mut conn = Connection::open(&path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        // The file lives in the user's folder but could still be tampered with: do not let
        // a crafted schema run functions, and double-check cell sizes while reading.
        conn.execute_batch("PRAGMA trusted_schema = OFF; PRAGMA cell_size_check = ON;")?;
        // Switching to WAL and migrating can fail with "busy" without ever consulting the busy
        // timeout when several processes start together (Windows shows it first), so both
        // are retried for a few seconds.
        retry_busy(|| Ok(conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?))?;
        // With WAL, NORMAL cannot corrupt the file; at worst the last commit is repeated,
        // and every write here is idempotent.
        conn.execute_batch("PRAGMA synchronous = NORMAL; PRAGMA cache_size = -8192;")?;
        retry_busy(|| migrate(&mut conn))?;
        Ok(Self { conn })
    }

    pub fn meta_get(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn meta_set(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES(?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Stable per installation: a UUID v4 created on first use.
    pub fn installation_id(&self) -> Result<String> {
        if let Some(id) = self.meta_get("installation_id")? {
            return Ok(id);
        }
        // INSERT OR IGNORE then re-read: two processes racing both end up with the winner's id.
        self.conn.execute(
            "INSERT OR IGNORE INTO meta(key, value) VALUES('installation_id', ?1)",
            [uuid::Uuid::new_v4().to_string()],
        )?;
        self.meta_get("installation_id")?
            .ok_or(Error::Sqlite(rusqlite::Error::QueryReturnedNoRows))
    }

    /// Turns collection on for a source. Keeps the cursor if it was on before.
    pub fn enable_source(&self, source: Source, since: Iso) -> Result<()> {
        self.conn.execute(
            "INSERT INTO source(name, enabled_since) VALUES(?1, ?2)
             ON CONFLICT(name) DO UPDATE SET enabled_since = excluded.enabled_since",
            params![source.name(), since.to_string()],
        )?;
        Ok(())
    }

    pub fn source_enabled(&self, source: Source) -> Result<bool> {
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

    /// A device is connected once pairing stored its `device_id`.
    pub fn is_connected(&self) -> Result<bool> {
        Ok(self.meta_get("device_id")?.is_some())
    }
}

/// Runs `op` again while SQLite says the database is busy, for up to 5 seconds.
fn retry_busy<T>(mut op: impl FnMut() -> Result<T>) -> Result<T> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match op() {
            Err(Error::Sqlite(rusqlite::Error::SqliteFailure(e, _)))
                if matches!(e.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
                    && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(15));
            }
            other => return other,
        }
    }
}

fn version(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}

fn migrate(conn: &mut Connection) -> Result<()> {
    let known = MIGRATIONS.len();
    let check = |found: i64| -> Result<bool> {
        if found < 0 {
            return Err(Error::BadSchemaVersion(found));
        }
        if found > known as i64 {
            return Err(Error::NewerSchema { found, known });
        }
        Ok(found == known as i64)
    };
    if check(version(conn)?)? {
        return Ok(()); // the common case: nothing to do, and no write lock taken
    }
    // Take the write lock first and read the version again inside it: another process may
    // have migrated while we waited. All pending steps are one transaction.
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current = version(&tx)?;
    if check(current)? {
        return Ok(());
    }
    for sql in &MIGRATIONS[current as usize..] {
        tx.execute_batch(sql)?;
    }
    tx.pragma_update(None, "user_version", known as i64)?;
    tx.commit()?;
    Ok(())
}
