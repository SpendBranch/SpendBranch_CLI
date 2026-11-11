//! Readers: find the session files, read what is new since the last time, and hand the
//! store a `ReadOutcome`. They only read; nothing is saved until the store commits.
//! This is the impure shell around the pure parsers in `sources`.
pub mod account;
pub mod claude_code;
pub mod codex;
pub mod discover;
pub mod git;
pub mod lines;

use std::path::PathBuf;

/// Reads one environment variable. A parameter so tests never touch the real environment.
pub type Env<'a> = &'a dyn Fn(&str) -> Option<String>;

pub fn user_home(env: Env) -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    env(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// FNV-1a over bytes. Stable on purpose: it is stored in the cursors.
pub(crate) fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// (device, file id): tells a replaced file from the same file grown. Windows has no stable
/// file id in std, so its creation time stands in: a replaced file gets a new one.
pub(crate) fn identity(meta: &std::fs::Metadata) -> (u64, u64) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (meta.dev(), meta.ino())
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        (0, meta.creation_time())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = meta;
        (0, 0)
    }
}

pub(crate) fn mtime_ms(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64)
}

/// Fingerprint of the first 64 bytes and of the (up to) 64 bytes before `offset`. If the file
/// was replaced or rewritten in place, one of them changed and the saved offset no longer
/// means anything.
pub(crate) fn tail_hash(file: &mut std::fs::File, offset: u64) -> std::io::Result<u64> {
    use std::io::{Read, Seek, SeekFrom};
    let n = offset.min(64) as usize;
    let mut bytes = [0u8; 128];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut bytes[..n])?;
    let _ = ();
    file.seek(SeekFrom::Start(offset - n as u64))?;
    file.read_exact(&mut bytes[n..2 * n])?;
    Ok(fnv(&bytes[..2 * n]))
}
