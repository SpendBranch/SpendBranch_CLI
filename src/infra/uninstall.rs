//! The files `uninstall` changes or deletes: the shell startup files the install script added a
//! PATH line to, and the data folder. The rules are in `crate::uninstall`.
use crate::env::Env;
use crate::uninstall::{folder_is_ours, without_path_lines};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// The startup files install.sh may have written to (macOS and Linux).
pub fn shell_files(env: Env) -> Vec<PathBuf> {
    let get = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let Some(home) = get("HOME") else {
        return Vec::new();
    };
    let zdot = get("ZDOTDIR").unwrap_or_else(|| home.clone());
    vec![
        home.join(".profile"),
        home.join(".bashrc"),
        home.join(".bash_profile"),
        zdot.join(".zshrc"),
        home.join(".config/fish/conf.d/spendbranch.fish"),
    ]
}

/// Takes our PATH lines out of `file`. A file that held nothing else (the fish file is ours
/// alone) is deleted. `Ok(false)`: no such file, or nothing of ours in it.
pub fn strip_path_lines(file: &Path) -> io::Result<bool> {
    let text = match fs::read_to_string(file) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    let Some(kept) = without_path_lines(&text) else {
        return Ok(false);
    };
    if kept.trim().is_empty() && file.extension().is_some_and(|e| e == "fish") {
        fs::remove_file(file)?;
    } else {
        // Written in place: the file keeps its owner, mode and any link pointing at it.
        fs::write(file, kept)?;
    }
    Ok(true)
}

/// What became of the data folder.
#[derive(Debug, PartialEq, Eq)]
pub enum DataDir {
    Removed,
    /// It was not there.
    Absent,
    /// Not a folder named `spendbranch` (`SPENDBRANCH_HOME` set elsewhere): left alone.
    NotOurs,
}

pub fn exists(dir: &Path) -> bool {
    dir.exists()
}

/// Deletes the data folder with everything in it (`remove_dir_all` deletes a link, never what
/// it points to).
pub fn remove_data_dir(dir: &Path) -> io::Result<DataDir> {
    if !folder_is_ours(dir.file_name().and_then(|n| n.to_str())) {
        return Ok(DataDir::NotOurs);
    }
    match fs::symlink_metadata(dir) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(DataDir::Absent),
        Err(e) => Err(e),
        // A link to a folder is a file on Unix and a directory on Windows.
        Ok(meta) if meta.file_type().is_symlink() => fs::remove_file(dir)
            .or_else(|_| fs::remove_dir(dir))
            .map(|()| DataDir::Removed),
        Ok(_) => fs::remove_dir_all(dir).map(|()| DataDir::Removed),
    }
}
