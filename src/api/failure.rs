//! What a failed answer becomes: a short code and numbers, never the server's own text.
use serde::Deserialize;
use std::fmt;

/// A server code is shown only if it looks like one: it is text we do not control, and a
/// terminal would obey any escape sequence hidden in it.
pub fn is_plain_code(code: &str) -> bool {
    (1..=64).contains(&code.len())
        && code
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

/// A call that did not succeed. `status` is 0 when the server was never reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiFailure {
    pub status: u16,
    pub code: String,
    pub retry_after_secs: Option<u64>,
    /// `X-SB-Server-Time`, in unix milliseconds: how the server's clock reads.
    pub server_time_ms: Option<i64>,
}

/// A server cannot make the CLI sleep for longer than this.
const MAX_RETRY_AFTER_SECS: u64 = 24 * 60 * 60;

impl ApiFailure {
    pub fn network() -> Self {
        Self::plain(0, "NETWORK")
    }

    pub fn plain(status: u16, code: &str) -> Self {
        Self {
            status,
            code: code.to_owned(),
            retry_after_secs: None,
            server_time_ms: None,
        }
    }

    /// Builds the failure for a non-2xx answer: `{"error":{"code":"..."}}` plus two headers.
    /// A body that is not that, or a code that is not plain, becomes `INTERNAL`.
    pub fn from_response(
        status: u16,
        body: &[u8],
        retry_after: Option<&str>,
        server_time: Option<&str>,
    ) -> Self {
        #[derive(Deserialize)]
        struct Body {
            error: Inner,
        }
        #[derive(Deserialize)]
        struct Inner {
            code: String,
        }
        let code = serde_json::from_slice::<Body>(body)
            .ok()
            .map(|b| b.error.code)
            .filter(|c| is_plain_code(c))
            .unwrap_or_else(|| "INTERNAL".into());
        Self {
            status,
            code,
            retry_after_secs: retry_after
                .and_then(|v| v.trim().parse::<u64>().ok())
                .filter(|s| *s > 0)
                .map(|s| s.min(MAX_RETRY_AFTER_SECS)),
            server_time_ms: server_time
                .and_then(|v| v.trim().parse::<i64>().ok())
                .filter(|t| *t > 0),
        }
    }

    /// The server answered, but not with what the contract says.
    pub fn bad_response(status: u16) -> Self {
        Self::plain(status, "BAD_RESPONSE")
    }
}

impl fmt::Display for ApiFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "request failed ({} {})", self.status, self.code)
    }
}

impl std::error::Error for ApiFailure {}
