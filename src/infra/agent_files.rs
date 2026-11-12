//! The agents' own files (`settings.json`, `hooks.json`): read as they are, written with a
//! backup and an atomic rename.
use super::private_fs;
use std::{
    ffi::OsString,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

/// `None` when the file does not exist.
pub fn read(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

fn beside(path: &Path, suffix: &str) -> PathBuf {
    let mut name: OsString = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Writes `text`. An existing file is copied to `<file>.spendbranch.bak` first and keeps its
/// permissions; a new one is owner-only. A symlink (a dotfiles manager) is followed, so the
/// link stays a link.
pub fn write(path: &Path, text: &str) -> io::Result<()> {
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if let Some(dir) = target.parent() {
        fs::create_dir_all(dir)?;
    }
    let existing = fs::metadata(&target).ok();
    if existing.is_some() {
        fs::copy(&target, beside(&target, ".spendbranch.bak"))?;
    }
    let tmp = beside(&target, ".spendbranch.tmp");
    let _ = fs::remove_file(&tmp); // a leftover from a crash must not leave its tail behind
    let mut file = private_fs::open(&tmp, false)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    drop(file);
    if let Some(meta) = existing {
        fs::set_permissions(&tmp, meta.permissions())?;
    }
    fs::rename(&tmp, &target).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}
