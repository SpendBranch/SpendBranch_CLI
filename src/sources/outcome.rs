//! What a reader hands to the store: data to buffer plus the state to remember, applied
//! together in one transaction so a crash can neither lose nor duplicate anything. Pure.
use crate::contract::{CoverageReport, Iso, Source, UsageObservation};
use std::collections::{HashMap, HashSet};

/// The saved state a reader starts from (read from `state.db`, never written by the reader).
pub struct ReaderState<'a> {
    /// Cursor key to its JSON. Claude: one per file. Codex: `file:<path>` and `thread:<id>`.
    pub cursors: HashMap<String, String>,
    /// Files whose format we could not read, so a repeated gap is not reported.
    pub unsupported_files: HashSet<String>,
    pub untested_version: Option<String>,
    seen: Box<dyn Fn(u64) -> bool + 'a>,
}

impl<'a> ReaderState<'a> {
    pub fn new(
        cursors: HashMap<String, String>,
        unsupported_files: HashSet<String>,
        untested_version: Option<String>,
        seen: impl Fn(u64) -> bool + 'a,
    ) -> Self {
        Self {
            cursors,
            unsupported_files,
            untested_version,
            seen: Box::new(seen),
        }
    }

    /// Whether the event with this `key_hash` was already read.
    pub fn is_seen(&self, hash: u64) -> bool {
        (self.seen)(hash)
    }
}

/// An empty state: nothing read yet.
impl Default for ReaderState<'static> {
    fn default() -> Self {
        Self::new(HashMap::new(), HashSet::new(), None, |_| false)
    }
}

#[derive(Debug)]
pub struct ReadOutcome {
    pub source: Source,
    pub read_at: Iso,
    pub observations: Vec<UsageObservation>,
    pub coverage: Vec<CoverageReport>,
    /// Event keys read now, with their time (kept for 45 days).
    pub seen: Vec<(u64, Iso)>,
    pub cursor_set: Vec<(String, String)>,
    pub cursor_remove: Vec<String>,
    /// The complete set after this read.
    pub unsupported_files: Vec<String>,
    pub untested_version: Option<String>,
}

impl ReadOutcome {
    pub fn empty(source: Source, read_at: Iso) -> Self {
        Self {
            source,
            read_at,
            observations: vec![],
            coverage: vec![],
            seen: vec![],
            cursor_set: vec![],
            cursor_remove: vec![],
            unsupported_files: vec![],
            untested_version: None,
        }
    }
}
