//! The `ingest/v1` answer: one result per observation, in order.
use super::is_plain_code;
use serde::Deserialize;

/// `POST /api/ingest/v1/events` answer, as sent.
#[derive(Clone, Debug, Deserialize)]
pub struct IngestResults {
    results: Vec<RawResult>,
}

#[derive(Clone, Debug, Deserialize)]
struct RawResult {
    index: usize,
    status: String,
    code: Option<String>,
}

/// What the server said about one observation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemOutcome {
    Accepted,
    Duplicate,
    /// With the server's code, which is plain text of at most 64 characters.
    Rejected(String),
}

impl IngestResults {
    /// One answer per observation sent, in order. Anything else is not trusted: the caller
    /// keeps its buffer, because it cannot tell what was stored.
    pub fn checked(self, sent: usize) -> Option<Vec<ItemOutcome>> {
        if self.results.len() != sent {
            return None;
        }
        let mut out = Vec::with_capacity(sent);
        for (i, r) in self.results.into_iter().enumerate() {
            if r.index != i {
                return None;
            }
            out.push(match (r.status.as_str(), r.code) {
                ("accepted", None) => ItemOutcome::Accepted,
                ("duplicate", None) => ItemOutcome::Duplicate,
                ("rejected", Some(code)) if is_plain_code(&code) => ItemOutcome::Rejected(code),
                _ => return None,
            });
        }
        Some(out)
    }
}
