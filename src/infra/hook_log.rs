//! `hook.log`: a plain append-only file, apart from the database so a hook never opens it.
//! Beside it, `last-sync-attempt`: when `sync` last started, so a hook that is not due decides
//! without opening the database (which stays the record `sync` itself goes by).
use super::private_fs;
use std::{
    fs, io,
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub const MAX_BYTES: u64 = 1024 * 1024;

/// Where the log is: the detached `sync` writes its output there too.
pub fn path(dir: &Path) -> PathBuf {
    dir.join("hook.log")
}

fn stamp_path(dir: &Path) -> PathBuf {
    dir.join("last-sync-attempt")
}

/// Saves when a `sync` started (ISO text). A failure is not worth stopping the sync for: the
/// hook then asks the database.
pub fn write_attempt(dir: &Path, at: &str) -> io::Result<()> {
    let mut file = private_fs::open(&stamp_path(dir), false)?;
    file.set_len(0)?;
    file.write_all(at.as_bytes())
}

/// When a `sync` last started, as saved; `None` without the file. A half-written file does not
/// parse, and the hook then asks the database.
pub fn read_attempt(dir: &Path) -> Option<String> {
    let mut text = String::new();
    fs::File::open(stamp_path(dir))
        .ok()?
        .take(64)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

/// The whole log, or `None` when there is none yet.
pub fn read(dir: &Path) -> Option<String> {
    fs::read_to_string(path(dir)).ok()
}

/// Appends one line. Above 1 MiB the current log first becomes `hook.log.1`.
pub fn append(dir: &Path, line: &str) -> io::Result<()> {
    private_fs::create_dir(dir)?;
    let log = path(dir);
    if fs::metadata(&log).is_ok_and(|m| m.len() > MAX_BYTES) {
        let old = dir.join("hook.log.1");
        let _ = fs::remove_file(&old); // rename does not replace an existing file on Windows
        fs::rename(&log, &old)?;
    }
    let mut file = private_fs::open(&log, true)?;
    writeln!(file, "{line}")
}
