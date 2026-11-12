//! The shapes of a rollout line, naming only the allowlisted fields, and the small checks on
//! their text. Nothing else of a line is ever stored.
use super::super::json::{Leaf, Obj};
use crate::contract::{ExternalId, Iso, MAX_TOKENS_PER_COUNTER, Model};
use serde::Deserialize;

// Only allowlisted fields are named; serde skips the rest unstored. A repeated key in one
// line makes it unsupported (the reference keeps the last one).
#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct Line {
    pub(super) timestamp: Leaf,
    #[serde(rename = "type")]
    pub(super) kind: Leaf,
    pub(super) ordinal: Leaf,
    pub(super) payload: Obj<Payload>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct Payload {
    // session_meta
    pub(super) id: Leaf,
    pub(super) session_id: Leaf,
    pub(super) parent_thread_id: Leaf,
    pub(super) cli_version: Leaf,
    pub(super) model_provider: Leaf,
    pub(super) creator_account_id: Leaf,
    pub(super) git: Obj<Git>,
    // turn_context
    pub(super) turn_id: Leaf,
    pub(super) model: Leaf,
    // event_msg
    #[serde(rename = "type")]
    pub(super) kind: Leaf,
    pub(super) thread_settings: Obj<ThreadSettings>,
    pub(super) rate_limits: Obj<RateLimits>,
    // token_usage_record
    pub(super) thread_id: Leaf,
    pub(super) response_id: Leaf,
    pub(super) usage: Obj<RawUsage>,
    pub(super) thread_token_usage: Obj<RawUsage>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct Git {
    pub(super) repository_url: Leaf,
    pub(super) branch: Leaf,
}
#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct ThreadSettings {
    pub(super) model: Leaf,
}
#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct RateLimits {
    pub(super) plan_type: Leaf,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct RawUsage {
    pub(super) input_tokens: Leaf,
    pub(super) cached_input_tokens: Leaf,
    pub(super) cache_write_input_tokens: Leaf,
    pub(super) output_tokens: Leaf,
    pub(super) reasoning_output_tokens: Leaf,
    pub(super) total_tokens: Leaf,
}

/// input, cached input, cache write, output, reasoning, total.
pub(super) type Usage = [u64; 6];

pub(super) fn usage_of(raw: &Obj<RawUsage>) -> Option<Usage> {
    let r = raw.0.as_ref()?;
    let mut out = [0; 6];
    for (slot, leaf) in out.iter_mut().zip([
        &r.input_tokens,
        &r.cached_input_tokens,
        &r.cache_write_input_tokens,
        &r.output_tokens,
        &r.reasoning_output_tokens,
        &r.total_tokens,
    ]) {
        let n = leaf
            .safe_int()
            .filter(|n| (0..=MAX_TOKENS_PER_COUNTER as i64).contains(n))?;
        *slot = n as u64;
    }
    Some(out)
}

pub(super) struct Record {
    pub(super) line: usize,
    pub(super) ordinal: i64,
    pub(super) at: Iso,
    pub(super) turn_id: String,
    pub(super) response_id: String,
    pub(super) usage: Usage,
    pub(super) thread: Option<Usage>,
    pub(super) model_at_time: Option<String>,
    pub(super) after_broken_line: bool,
}

pub(super) fn text(leaf: &Leaf, valid: impl Fn(&str) -> bool) -> Option<&str> {
    leaf.str().filter(|s| valid(s))
}
pub(super) fn is_id(s: &str) -> bool {
    ExternalId::new(s).is_some()
}
pub(super) fn is_model(s: &str) -> bool {
    Model::new(s).is_some()
}
