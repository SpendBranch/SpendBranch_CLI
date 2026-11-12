//! `hook.log`: a plain append-only file, apart from the database so a hook never opens it.
use super::private_fs;
use std::{
    fs, io,
    io::Write,
    path::{Path, PathBuf},
};

pub const MAX_BYTES: u64 = 1024 * 1024;

/// Where the log is: the detached `sync` writes its output there too.
pub fn path(dir: &Path) -> PathBuf {
    dir.join("hook.log")
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
