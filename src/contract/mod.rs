//! The `ingest/v1` contract (schema/ingest-v1.json) as Rust types. Pure: no I/O.
//! Every text type validates in its constructor, so an invalid value cannot exist.
//! Only deltas travel: the server never sums totals.
use serde::{Deserialize, Serialize};

pub const CONTRACT_VERSION: &str = "ingest/v1";
pub const MAX_BATCH_OBSERVATIONS: usize = 500;
pub const MAX_BATCH_COVERAGE: usize = 50;
pub const MAX_TOKENS_PER_COUNTER: u64 = 1_000_000_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    ClaudeCode,
    Codex,
}

impl Source {
    /// The contract's name for the source, also the key used in `state.db`.
    pub fn name(self) -> &'static str {
        match self {
            Source::ClaudeCode => "claude_code",
            Source::Codex => "codex",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRole {
    Main,
    Subagent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PayerEvidence {
    Transcript,
    LocalAccount,
    Otlp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingMode {
    Subscription,
    Api,
}

mod batch;
mod counters;
mod coverage;
mod observation;
mod text;
mod time;

pub use batch::*;
pub use counters::*;
pub use coverage::*;
pub use observation::*;
pub use text::*;
pub use time::*;
