//! The local state: `state.db`, the `sync` lock and `hook.log`.
use rusqlite::Connection;
use spendbranch::infra::{hook_log, lock, store::Store};
use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

fn user_version(dir: &Path) -> i64 {
    Connection::open(dir.join("state.db"))
        .unwrap()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap()
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn a_new_database_has_the_schema_wal_and_private_permissions() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("data");
    let store = Store::open(&dir).unwrap();
    assert_eq!(user_version(&dir), 1);
    let conn = Connection::open(dir.join("state.db")).unwrap();
    for table in [
        "meta",
        "sync_state",
        "rejected",
        "source",
        "cursor",
        "unsupported_file",
        "seen",
        "spool",
        "pending_coverage",
    ] {
        conn.prepare(&format!("SELECT * FROM {table}"))
            .unwrap_or_else(|e| panic!("{table}: {e}"));
    }
    let journal: String = conn
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(journal, "wal");
    store.meta_set("k", "v").unwrap(); // makes sure the -wal file exists
    #[cfg(unix)]
    {
        assert_eq!(mode(&dir), 0o700);
        for name in ["state.db", "state.db-wal", "state.db-shm"] {
            let p = dir.join(name);
            if p.exists() {
                assert_eq!(mode(&p), 0o600, "{name}");
            }
        }
    }
}

#[test]
fn reopening_keeps_the_data_and_does_not_migrate_again() {
    let dir = tempfile::tempdir().unwrap();
    Store::open(dir.path())
        .unwrap()
        .meta_set("server", "http://localhost:3000")
        .unwrap();
    let again = Store::open(dir.path()).unwrap();
    assert_eq!(
        again.meta_get("server").unwrap().as_deref(),
        Some("http://localhost:3000")
    );
    assert_eq!(again.meta_get("missing").unwrap(), None);
    assert_eq!(user_version(dir.path()), 1);
}

#[test]
fn meta_set_overwrites() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(dir.path()).unwrap();
    s.meta_set("k", "1").unwrap();
    s.meta_set("k", "2").unwrap();
    assert_eq!(s.meta_get("k").unwrap().as_deref(), Some("2"));
}

#[test]
fn a_database_from_a_newer_build_is_refused_untouched() {
    let dir = tempfile::tempdir().unwrap();
    Store::open(dir.path()).unwrap();
    Connection::open(dir.path().join("state.db"))
        .unwrap()
        .pragma_update(None, "user_version", 99)
        .unwrap();
    let err = Store::open(dir.path()).unwrap_err().to_string();
    assert!(
        err.contains("newer spendbranch") && err.contains("99"),
        "{err}"
    );
    assert_eq!(user_version(dir.path()), 99);
}

#[test]
fn installation_id_is_a_stable_uuid_v4() {
    let dir = tempfile::tempdir().unwrap();
    let first = Store::open(dir.path()).unwrap().installation_id().unwrap();
    let id = uuid::Uuid::parse_str(&first).unwrap();
    assert_eq!(id.get_version_num(), 4);
    assert_eq!(
        Store::open(dir.path()).unwrap().installation_id().unwrap(),
        first
    );
    let other = tempfile::tempdir().unwrap();
    assert_ne!(
        Store::open(other.path())
            .unwrap()
            .installation_id()
            .unwrap(),
        first
    );
}

#[test]
fn connected_means_a_device_id_is_stored() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(dir.path()).unwrap();
    assert!(!s.is_connected().unwrap());
    s.meta_set("device_id", "dev_synthetic").unwrap();
    assert!(s.is_connected().unwrap());
}

#[test]
fn two_holders_cannot_share_the_lock_and_release_frees_it() {
    let dir = tempfile::tempdir().unwrap();
    let first = lock::try_acquire(dir.path()).unwrap().expect("free lock");
    assert!(lock::try_acquire(dir.path()).unwrap().is_none());
    drop(first);
    assert!(lock::try_acquire(dir.path()).unwrap().is_some());
}

/// Not a real test: it is the "other process" of the test below, and does nothing otherwise.
#[test]
fn lock_holder_child() {
    let Ok(dir) = std::env::var("SPENDBRANCH_TEST_LOCK_DIR") else {
        return;
    };
    let _held = lock::try_acquire(Path::new(&dir))
        .unwrap()
        .expect("the child must get the lock");
    fs::write(Path::new(&dir).join("ready"), "").unwrap();
    thread::sleep(Duration::from_secs(120));
}

#[test]
fn another_process_holds_the_lock_and_its_death_releases_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["lock_holder_child", "--exact", "--nocapture"])
        .env("SPENDBRANCH_TEST_LOCK_DIR", dir.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !dir.path().join("ready").exists() {
        assert!(
            child.try_wait().unwrap().is_none(),
            "child exited before locking"
        );
        assert!(Instant::now() < deadline, "child never locked");
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        lock::try_acquire(dir.path()).unwrap().is_none(),
        "held by the child"
    );

    child.kill().unwrap(); // no chance to clean up: like a crash or SIGKILL
    child.wait().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while lock::try_acquire(dir.path()).unwrap().is_none() {
        assert!(
            Instant::now() < deadline,
            "the OS did not release the lock of a dead process"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn hook_log_appends_lines_and_rotates_above_1_mib() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("hook.log");
    hook_log::append(dir.path(), r#"{"event":"one"}"#).unwrap();
    hook_log::append(dir.path(), r#"{"event":"two"}"#).unwrap();
    assert_eq!(
        fs::read_to_string(&log).unwrap(),
        "{\"event\":\"one\"}\n{\"event\":\"two\"}\n"
    );
    #[cfg(unix)]
    assert_eq!(mode(&log), 0o600);

    fs::write(&log, vec![b'x'; hook_log::MAX_BYTES as usize + 1]).unwrap();
    hook_log::append(dir.path(), "fresh").unwrap();
    assert_eq!(fs::read_to_string(&log).unwrap(), "fresh\n");
    assert_eq!(
        fs::metadata(dir.path().join("hook.log.1")).unwrap().len(),
        hook_log::MAX_BYTES + 1
    );

    fs::write(&log, vec![b'y'; hook_log::MAX_BYTES as usize + 1]).unwrap(); // a second rotation replaces .1
    hook_log::append(dir.path(), "again").unwrap();
    assert_eq!(fs::read(dir.path().join("hook.log.1")).unwrap()[0], b'y');
}

#[test]
fn many_processes_opening_a_new_database_at_once_all_succeed() {
    // A hook and the scheduler can start together on a fresh machine. Each opens its own
    // connection, like separate processes would.
    for round in 0..8 {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data");
        let handles: Vec<_> = (0..12)
            .map(|_| {
                let path = path.clone();
                thread::spawn(move || Store::open(&path).map(|_| ()).map_err(|e| e.to_string()))
            })
            .collect();
        for h in handles {
            h.join()
                .unwrap()
                .unwrap_or_else(|e| panic!("round {round}: every opener must succeed: {e}"));
        }
        assert_eq!(user_version(&path), 1);
    }
}

#[test]
fn a_newer_database_is_a_typed_error() {
    let dir = tempfile::tempdir().unwrap();
    Store::open(dir.path()).unwrap();
    Connection::open(dir.path().join("state.db"))
        .unwrap()
        .pragma_update(None, "user_version", 7)
        .unwrap();
    assert!(matches!(
        Store::open(dir.path()),
        Err(spendbranch::infra::store::Error::NewerSchema { found: 7, known: 1 })
    ));
}

#[cfg(unix)]
mod folders {
    use super::*;
    use spendbranch::infra::private_fs;
    use std::os::unix::fs::PermissionsExt;

    fn set(path: &Path, mode: u32) {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn a_new_folder_is_private_and_so_are_the_parents_we_create() {
        let tmp = tempfile::tempdir().unwrap();
        let leaf = tmp.path().join("a/b/data");
        private_fs::create_dir(&leaf).unwrap();
        assert_eq!(mode(&leaf), 0o700);
        assert_eq!(mode(&tmp.path().join("a")), 0o700);
    }

    #[test]
    fn a_folder_that_already_exists_is_never_changed() {
        // SPENDBRANCH_HOME may point at the user's home or a shared folder by mistake.
        let tmp = tempfile::tempdir().unwrap();
        for existing in [0o755, 0o775, 0o770, 0o750, 0o700] {
            let dir = tmp.path().join(format!("d{existing:o}"));
            fs::create_dir(&dir).unwrap();
            set(&dir, existing);
            private_fs::create_dir(&dir).unwrap();
            assert_eq!(mode(&dir), existing, "must stay {existing:o}");
        }
    }

    #[test]
    fn a_folder_anybody_can_write_to_is_refused_and_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        for open in [0o777, 0o757, 0o702, 0o1777] {
            let dir = tmp.path().join(format!("w{open:o}"));
            fs::create_dir(&dir).unwrap();
            set(&dir, open);
            let err = private_fs::create_dir(&dir).unwrap_err();
            assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
            assert!(
                err.to_string().contains("SPENDBRANCH_HOME"),
                "tells what to do: {err}"
            );
            assert_eq!(
                mode(&dir),
                open & 0o777,
                "refusing must not change it either"
            );
            assert!(
                Store::open(&dir).is_err(),
                "and no database is created in it"
            );
            assert!(!dir.join("state.db").exists());
        }
    }

    #[test]
    fn a_file_where_the_folder_should_be_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("not-a-folder");
        fs::write(&file, "x").unwrap();
        assert!(private_fs::create_dir(&file).is_err());
    }
}

fn observation(id: &str) -> spendbranch::contract::UsageObservation {
    serde_json::from_value(serde_json::json!({
        "source": "codex", "externalSessionId": "s", "externalEventId": id,
        "occurredAt": "2026-06-01T10:00:00.000Z", "model": "m", "agentRole": "main",
        "counters": {"inputTokens": 1, "outputTokens": 1, "cacheReadTokens": 0,
                     "cacheWriteTokens": 0, "reasoningTokens": 0},
        "counterOrigin": "reported_delta", "parserVersion": "codex/v1"
    }))
    .unwrap()
}

#[test]
fn several_writers_at_once_neither_fail_nor_lose_anything() {
    use spendbranch::contract::{Iso, Source};
    use spendbranch::infra::store::{Commit, SpoolLimits};
    use spendbranch::sources::outcome::ReadOutcome;

    const WRITERS: u64 = 6;
    const ROUNDS: u64 = 15;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data");
    Store::open(&path).unwrap();
    let handles: Vec<_> = (0..WRITERS)
        .map(|w| {
            let path = path.clone();
            thread::spawn(move || {
                let mut store = Store::open(&path).map_err(|e| e.to_string())?;
                let at = Iso::parse("2026-06-01T10:00:00Z").unwrap();
                for round in 0..ROUNDS {
                    let mut out = ReadOutcome::empty(Source::Codex, at);
                    out.observations = (0..3)
                        .map(|k| observation(&format!("e{w}_{round}_{k}")))
                        .collect();
                    out.seen = (0..5).map(|k| ((w << 32) | (round << 8) | k, at)).collect();
                    // the commit counts the buffer first (a read) and then writes: it must take
                    // the write lock up front, or a writer that got in between makes it fail
                    match store.commit_read(&out, SpoolLimits::default()) {
                        Ok(Commit::Stored) => {}
                        other => return Err(format!("writer {w} round {round}: {other:?}")),
                    }
                }
                Ok(())
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap().expect("no writer may fail");
    }
    let store = Store::open(&path).unwrap();
    assert_eq!(
        store.pending_counts().unwrap().0,
        WRITERS * ROUNDS * 3,
        "every observation was buffered once"
    );
    assert_eq!(
        store
            .seen_count(spendbranch::contract::Source::Codex)
            .unwrap(),
        WRITERS * ROUNDS * 5
    );
}
