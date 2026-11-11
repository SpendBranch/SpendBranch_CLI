//! The tools whose sessions we can read: pure parsers, no I/O. Readers (discovery,
//! cursors, streaming) live elsewhere and feed these functions complete lines.
//!
//! Every line is parsed, with structs that name only the allowlisted fields, so the
//! conversation text is skipped without being stored. Lines are not pre-filtered by a
//! substring: lines without usage still feed format detection, the version warning and
//! the paying account.
//!
//! Known differences from implementations in other languages, all in the safe direction
//! (the line becomes a gap, never a wrong number):
//! - timestamps must be RFC 3339 with an offset or `Z`, and a real calendar date; JavaScript's
//!   `Date.parse` also takes date-only and offset-less text (read as local time) and rolls
//!   `02-30` over to March;
//! - a key repeated inside one line makes that line unsupported (JavaScript keeps the last);
//! - repository URLs with international host names or percent-escapes give no repository.
pub mod claude_code;
pub mod codex;
mod json;
pub mod repository;
pub mod version;

pub use crate::contract::Source;

impl Source {
    /// Accepts the CLI spellings: `claude-code`, `claude_code` and `codex`.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "claude-code" | "claude_code" => Some(Self::ClaudeCode),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }
}

/// Whether the parser understood the format of what it read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    Ok,
    UnsupportedFormat,
}

/// Stable 64-bit FNV-1a of an event key. It is stored in `state.db`, so it must never change.
pub fn key_hash(key: &str) -> u64 {
    key.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}
