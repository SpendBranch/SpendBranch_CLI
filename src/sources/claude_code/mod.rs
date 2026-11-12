//! `claude-code/v2`: Claude Code transcript text to usage observations. Pure and
//! deterministic: the same text and options always give the same result.
use super::{
    Format,
    json::{Leaf, Obj},
    key_hash, version,
};
use crate::contract::{AccountRef, AgentRole, Branch, Iso, Model, SourceVersion, UsageObservation};
use serde::Serialize;

pub const PARSER: &str = "claude-code/v2";

/// Exactly what the owner authorizes. Compared as plain strings with the authorization:
/// changing a single character breaks `enable` (`FIELDS_NOT_AUTHORIZED`).
pub const FIELDS: [&str; 32] = [
    "type",
    "sessionId",
    "timestamp",
    "version",
    "isSidechain",
    "gitBranch",
    "requestId",
    "message.id",
    "message.model",
    "message.usage.input_tokens",
    "message.usage.output_tokens",
    "message.usage.cache_read_input_tokens",
    "message.usage.cache_creation_input_tokens",
    "message.usage.cache_creation.ephemeral_1h_input_tokens",
    "message.usage.output_tokens_details.thinking_tokens",
    "message.usage.server_tool_use.web_search_requests",
    "bridge-session.ownerOrganizationUuid",
    "local:cwd (only on the device, becomes the repository key)",
    "local:.claude.json oauthAccount.organizationUuid",
    "local:.claude.json oauthAccount.billingType",
    "otlp:event.name",
    "otlp:event.timestamp",
    "otlp:session.id",
    "otlp:request_id",
    "otlp:model",
    "otlp:input_tokens",
    "otlp:output_tokens",
    "otlp:cache_read_tokens",
    "otlp:cache_creation_tokens",
    "otlp:cache_creation_1h_tokens",
    "otlp:query_source",
    "otlp:organization.id",
];

#[derive(Default)]
pub struct ReadOptions<'a> {
    /// The file lives under a `subagents` folder.
    pub subagent_file: bool,
    /// Whether an event key (see `key_hash`) was already read: a forked session copies history.
    pub seen: Option<&'a dyn Fn(u64) -> bool>,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadResult {
    pub observations: Vec<UsageObservation>,
    pub new_keys: Vec<String>,
    /// 1-based line numbers the parser could not use.
    pub unsupported_lines: Vec<usize>,
    pub untested_version: Option<String>,
    pub format: Format,
}
/// Reads a whole text at once. Same result as feeding its lines to a `Parser`.
pub fn read(text: &str, options: &ReadOptions) -> ReadResult {
    let mut parser = Parser::new(options);
    for raw in text.split('\n') {
        parser.push_line(raw);
    }
    parser.finish()
}

mod build;
mod parser;
mod raw;

pub use parser::Parser;
