//! `codex/v1`: Codex rollout text to usage observations. Pure and deterministic.
//! Counters come from each response's `token_usage_record`, never from differences of
//! the running total.
use super::Format;
use crate::contract::UsageObservation;
use serde::{Deserialize, Serialize};

pub const PARSER: &str = "codex/v1";

/// Exactly what the owner authorizes: see `claude_code::FIELDS`.
pub const FIELDS: [&str; 25] = [
    "timestamp",
    "type",
    "ordinal",
    "session_meta.id",
    "session_meta.session_id",
    "session_meta.parent_thread_id",
    "session_meta.cli_version",
    "session_meta.model_provider",
    "session_meta.creator_account_id",
    "session_meta.git.repository_url (becomes host/owner/name, without credentials)",
    "session_meta.git.branch",
    "turn_context.turn_id",
    "turn_context.model",
    "thread_settings_applied.thread_settings.model",
    "token_count.rate_limits.plan_type",
    "token_usage_record.thread_id",
    "token_usage_record.turn_id",
    "token_usage_record.response_id",
    "token_usage_record.usage.input_tokens",
    "token_usage_record.usage.cached_input_tokens",
    "token_usage_record.usage.cache_write_input_tokens",
    "token_usage_record.usage.output_tokens",
    "token_usage_record.usage.reasoning_output_tokens",
    "token_usage_record.usage.total_tokens",
    "local:token_usage_record.thread_token_usage (device-side verification only)",
];

#[derive(Default)]
pub struct ReadOptions<'a> {
    /// Only records with a greater ordinal are emitted (the thread was read up to here).
    pub after_ordinal: Option<i64>,
    /// Whether a response id (see `key_hash`) was already read.
    pub seen: Option<&'a dyn Fn(u64) -> bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnsupportedCode {
    InvalidJson,
    NotAnObject,
    MissingField,
    InconsistentCounters,
    ConflictingResponse,
    UnresolvedModel,
    Contract,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct UnsupportedLine {
    pub line: usize,
    pub code: UnsupportedCode,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadResult {
    pub thread_id: Option<String>,
    pub session_id: Option<String>,
    pub observations: Vec<UsageObservation>,
    pub new_keys: Vec<String>,
    pub last_ordinal: i64,
    pub foreign_records: usize,
    pub unsupported_lines: Vec<UnsupportedLine>,
    pub untested_version: Option<String>,
    pub format: Format,
}

mod build;
mod parser;
mod raw;
mod turns;

pub use parser::Parser;
use raw::Usage;

fn empty(format: Format, unsupported: Vec<UnsupportedLine>) -> ReadResult {
    ReadResult {
        thread_id: None,
        session_id: None,
        observations: vec![],
        new_keys: vec![],
        last_ordinal: -1,
        foreign_records: 0,
        unsupported_lines: unsupported,
        untested_version: None,
        format,
    }
}

/// Reads a whole text at once. Same result as feeding its complete lines to a `Parser`.
pub fn read(input: &str, options: &ReadOptions) -> ReadResult {
    let mut parser = Parser::new(options);
    // The last piece has no line break yet: Codex may still be writing it.
    if let Some(end) = input.rfind('\n') {
        for raw in input[..end].split('\n') {
            parser.push_line(raw);
        }
    }
    parser.finish().0
}

/// What a thread's first lines told us. Needed to read the lines appended later.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meta {
    pub thread_id: String,
    pub session_id: String,
    pub subagent: bool,
    pub cli_version: Option<String>,
    pub account: Option<String>,
    pub repository: Option<String>,
    pub branch: Option<String>,
}

/// Everything needed to continue reading a rollout from where a previous read ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resume {
    pub meta: Meta,
    pub current_model: Option<String>,
    pub turn_models: Vec<(String, String)>,
    pub plan_seen: bool,
    pub had_record: bool,
    pub last_ordinal: i64,
    /// Running total after the last record, to check the first record of the next read.
    pub prev_thread: Option<Usage>,
    pub broken_since_last_record: bool,
}
