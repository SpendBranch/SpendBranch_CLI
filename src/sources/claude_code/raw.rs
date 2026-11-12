//! The shapes of a transcript line, naming only the allowlisted fields, and the small checks on
//! their text. Nothing else of a line is ever stored.
use crate::contract::{AgentRole, ExternalId};
use crate::sources::json::{Leaf, Obj};
use serde::Deserialize;

// Only allowlisted fields are named here. Serde skips every other field, the conversation
// text included, without storing it. A key repeated in one line makes the line unsupported
// (the reference keeps the last one).
#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct Line {
    #[serde(rename = "type")]
    pub(super) kind: Leaf,
    #[serde(rename = "sessionId")]
    pub(super) session_id: Leaf,
    pub(super) timestamp: Leaf,
    pub(super) version: Leaf,
    #[serde(rename = "isSidechain")]
    pub(super) is_sidechain: Leaf,
    #[serde(rename = "gitBranch")]
    pub(super) git_branch: Leaf,
    #[serde(rename = "requestId")]
    pub(super) request_id: Leaf,
    #[serde(rename = "ownerOrganizationUuid")]
    pub(super) owner_organization_uuid: Leaf,
    pub(super) message: Obj<Message>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct Message {
    pub(super) id: Leaf,
    pub(super) model: Leaf,
    pub(super) usage: Obj<Usage>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct Usage {
    pub(super) input_tokens: Leaf,
    pub(super) output_tokens: Leaf,
    pub(super) cache_read_input_tokens: Leaf,
    pub(super) cache_creation_input_tokens: Leaf,
    pub(super) output_tokens_details: Obj<Details>,
    pub(super) cache_creation: Obj<CacheCreation>,
    pub(super) server_tool_use: Obj<ServerToolUse>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct Details {
    pub(super) thinking_tokens: Leaf,
}
#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct CacheCreation {
    pub(super) ephemeral_1h_input_tokens: Leaf,
}
#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct ServerToolUse {
    pub(super) web_search_requests: Leaf,
}

/// input, output, cache read, cache write, reasoning, cache write 1h, web searches.
pub(super) type Counters = [Option<u64>; 7];

pub(super) fn is_id(s: &str) -> bool {
    ExternalId::new(s).is_some()
}

pub(super) struct Pending {
    pub(super) session_id: String,
    pub(super) model: String,
    pub(super) at: i64,
    pub(super) role: AgentRole,
    pub(super) counters: Counters,
    pub(super) version: Option<String>,
    pub(super) branch: Option<String>,
    pub(super) broken: bool,
}

pub(super) fn max(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}
