//! The `sync` lock. The OS releases an advisory file lock when the process dies,
//! so there is no stale-lock logic: no PID, no age, no break file.
use super::private_fs;
use std::{fs::File, io, path::Path};

/// Held until dropped (or until the process ends, however it ends).
#[derive(Debug)]
pub struct SyncLock(#[allow(dead_code)] File);

/// `Ok(None)` means another process holds the lock.
pub fn try_acquire(dir: &Path) -> io::Result<Option<SyncLock>> {
    private_fs::create_dir(dir)?;
    let file = private_fs::open(&dir.join("sync.lock"), false)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(SyncLock(file))),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(e),
    }
}
