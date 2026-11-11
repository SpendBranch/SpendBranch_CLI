//! `hook.log`: a plain append-only file, apart from the database so a hook never opens it.
use super::private_fs;
use std::{fs, io, io::Write, path::Path};

pub const MAX_BYTES: u64 = 1024 * 1024;

/// Appends one line. Above 1 MiB the current log first becomes `hook.log.1`.
pub fn append(dir: &Path, line: &str) -> io::Result<()> {
    private_fs::create_dir(dir)?;
    let log = dir.join("hook.log");
    if fs::metadata(&log).is_ok_and(|m| m.len() > MAX_BYTES) {
        let old = dir.join("hook.log.1");
        let _ = fs::remove_file(&old); // rename does not replace an existing file on Windows
        fs::rename(&log, &old)?;
    }
    let mut file = private_fs::open(&log, true)?;
    writeln!(file, "{line}")
}
