//! `state.db`: the single local SQLite file (schema in the planning, section 9).
use super::private_fs;
use rusqlite::{Connection, OptionalExtension, params};
use std::{path::Path, time::Duration};

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
