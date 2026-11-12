//! What the CLI says to the server and what it accepts back: the server address rules, the
//! pairing messages and the failure type. Pure: the network code lives in `infra::http`.
//!
//! Everything the server sends is untrusted text. It is validated here and never echoed.
use crate::auth::DeviceId;
use crate::contract::Iso;
use serde::{Deserialize, Serialize};
use std::fmt;

// --- Server address -------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerUrlError {
    /// Not `http://` or `https://`, or not a bare address (path, query, user name, spaces...).
    Malformed,
    /// `http://` to a host that is not this machine: it would send everything in the clear.
    InsecureRemote,
}

impl fmt::Display for ServerUrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Malformed => "the server must be a bare address like https://host[:port]",
            Self::InsecureRemote => "plain http:// is only allowed for localhost; use https://",
        })
    }
}

impl std::error::Error for ServerUrlError {}

/// An origin: scheme, host and port, normalised so two spellings of one origin compare equal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    https: bool,
    host: String,
    port: u16,
}

impl Origin {
    /// Reads the origin of `url`, ignoring any path, query or fragment after it.
    fn of(url: &str) -> Option<Self> {
        let (https, rest) = match url.split_once("://")? {
            (s, rest) if s.eq_ignore_ascii_case("https") => (true, rest),
            (s, rest) if s.eq_ignore_ascii_case("http") => (false, rest),
            _ => return None,
        };
        let authority = rest.split(['/', '?', '#']).next()?;
        // No user name or password, backslash, space or non-ASCII: `http://localhost@evil.com`
        // and its cousins are how a loopback check gets fooled.
        if authority.is_empty()
            || !authority
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-.:[]".contains(&b))
        {
            return None;
        }
        let (host, port) = match authority.strip_prefix('[') {
            Some(v6) => {
                let (host, tail) = v6.split_once(']')?;
                if host.is_empty() || !host.bytes().all(|b| b.is_ascii_hexdigit() || b == b':') {
                    return None;
                }
                (format!("[{}]", host.to_ascii_lowercase()), tail)
            }
            None => {
                let end = authority.find(':').unwrap_or(authority.len());
                let host = &authority[..end];
                let labels_ok = host.split('.').all(|l| {
                    !l.is_empty()
                        && !l.starts_with('-')
                        && !l.ends_with('-')
                        && !l.contains(['[', ']'])
                });
                if !labels_ok {
                    return None;
                }
                (host.to_ascii_lowercase(), &authority[end..])
            }
        };
        let port = match tail_port(port) {
            Some(Some(p)) => p,
            Some(None) => {
                if https {
                    443
                } else {
                    80
                }
            }
            None => return None,
        };
        Some(Self { https, host, port })
    }

    fn is_loopback(&self) -> bool {
        matches!(self.host.as_str(), "localhost" | "127.0.0.1" | "[::1]")
    }
}

/// `":8080"` is `Some(Some(8080))`, `""` is `Some(None)`, anything else is `None`.
fn tail_port(tail: &str) -> Option<Option<u16>> {
    if tail.is_empty() {
        return Some(None);
    }
    let digits = tail.strip_prefix(':')?;
    if digits.is_empty() || digits.len() > 5 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u16>().ok().filter(|p| *p > 0).map(Some)
}

/// Where the API lives. It can only be built from an address the CLI is willing to talk to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerUrl(Origin);

impl ServerUrl {
    /// `https://host[:port]`, or `http://` for `localhost`, `127.0.0.1` and `[::1]` only.
    /// One trailing `/` is allowed; any other path is not (the signed path is the API's own).
    pub fn parse(text: &str) -> Result<Self, ServerUrlError> {
        let bare = text.strip_suffix('/').unwrap_or(text);
        let origin = Origin::of(bare).ok_or(ServerUrlError::Malformed)?;
        let after_scheme = &bare[bare.find("://").map_or(0, |i| i + 3)..];
        if after_scheme.contains(['/', '?', '#']) {
            return Err(ServerUrlError::Malformed);
        }
        if !origin.https && !origin.is_loopback() {
            return Err(ServerUrlError::InsecureRemote);
        }
        Ok(Self(origin))
    }

    pub fn is_loopback(&self) -> bool {
        self.0.is_loopback()
    }

    /// The address without a trailing slash, ready to put a path after.
    pub fn base(&self) -> String {
        let o = &self.0;
        let default = if o.https { 443 } else { 80 };
        let scheme = if o.https { "https" } else { "http" };
        if o.port == default {
            format!("{scheme}://{}", o.host)
        } else {
            format!("{scheme}://{}:{}", o.host, o.port)
        }
    }

    /// Whether `url` (a verification link, say) points at this same origin.
    pub fn same_origin(&self, url: &str) -> bool {
        Origin::of(url).is_some_and(|o| o == self.0)
    }
}

// --- Failure --------------------------------------------------------------------------

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

// --- Pairing --------------------------------------------------------------------------

/// `POST /api/cli/v1/device-codes`
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartPairing<'a> {
    pub installation_id: &'a str,
    pub cli_version: Option<&'a str>,
    /// The device's public key, base64url of 32 bytes.
    pub public_key: &'a str,
}

/// `POST /api/cli/v1/tokens`
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PollToken<'a> {
    pub device_code: &'a str,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingStarted {
    /// The CLI's secret for polling.
    pub device_code: String,
    /// What the person sees, `BCDF-GHJK`.
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub expires_in: u64,
    pub interval: u64,
}

const USER_CODE_ALPHABET: &str = "BCDFGHJKLMNPQRSTVWXZ";

impl PairingStarted {
    /// Accepts the answer only if every field is well formed and both links lead to the
    /// server we asked: a hostile or buggy server cannot point the browser elsewhere.
    pub fn checked(self, server: &ServerUrl) -> Option<Self> {
        let code_ok = (1..=64).contains(&self.device_code.len())
            && self
                .device_code
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        let user_ok = self.user_code.len() == 9
            && self.user_code.char_indices().all(|(i, c)| {
                if i == 4 {
                    c == '-'
                } else {
                    USER_CODE_ALPHABET.contains(c)
                }
            });
        let timing_ok = (1..=3600).contains(&self.expires_in) && (1..=60).contains(&self.interval);
        // The links are printed, so nothing but visible ASCII: no escape sequence, no bidi mark.
        let plain = |link: &str| link.len() <= 2048 && link.bytes().all(|b| b.is_ascii_graphic());
        let links_ok = [&self.verification_uri, &self.verification_uri_complete]
            .into_iter()
            .all(|link| plain(link) && server.same_origin(link));
        (code_ok && user_ok && timing_ok && links_ok).then_some(self)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PollOutcome {
    AuthorizationPending,
    SlowDown,
    ExpiredToken,
    AccessDenied,
    #[serde(rename_all = "camelCase")]
    Approved {
        device_id: String,
        expires_at: String,
        organization_id: String,
        organization_name: String,
    },
}

/// A poll answer that passed validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Polled {
    Pending,
    SlowDown,
    Expired,
    Denied,
    Approved(Approval),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Approval {
    pub device_id: DeviceId,
    pub expires_at: Iso,
    pub organization_id: String,
    /// Safe to print: no control or direction-changing characters.
    pub organization_name: String,
}

/// Text the server chose that we will print: no control or invisible-direction characters.
fn printable(text: &str, max_chars: usize) -> bool {
    let n = text.chars().count();
    (1..=max_chars).contains(&n)
        && text.chars().all(|c| {
            !c.is_control()
                && !matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}')
        })
}

impl PollOutcome {
    pub fn checked(self) -> Option<Polled> {
        Some(match self {
            Self::AuthorizationPending => Polled::Pending,
            Self::SlowDown => Polled::SlowDown,
            Self::ExpiredToken => Polled::Expired,
            Self::AccessDenied => Polled::Denied,
            Self::Approved {
                device_id,
                expires_at,
                organization_id,
                organization_name,
            } => Polled::Approved(Approval {
                device_id: DeviceId::new(&device_id)?,
                expires_at: Iso::parse(&expires_at)?,
                organization_id: (printable(&organization_id, 64)).then_some(organization_id)?,
                organization_name: printable(&organization_name, 200)
                    .then_some(organization_name)?,
            }),
        })
    }
}

// --- Status ---------------------------------------------------------------------------

/// Where the API is when no address is given: the product's own origin in development.
pub const DEFAULT_SERVER: &str = "http://localhost:5173";

/// `GET /api/cli/v1/status`, as the server sent it. `checked` is the only way to use it.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliStatus {
    organization_name: String,
    member_name: Option<String>,
    device: RawDevice,
    sources: Vec<RawSource>,
    collection: Vec<RawCollection>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawDevice {
    label: String,
    key_fingerprint: String,
    created_at: String,
    expires_at: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSource {
    source: String,
    state: String,
    parser_version: String,
    fields: Vec<String>,
    window_start: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawCollection {
    source: String,
    last_seen_at: Option<String>,
    observed_until: Option<String>,
    open_gaps: Vec<RawGap>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawGap {
    starts_at: String,
    ends_at: String,
    reason: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceState {
    Active,
    Expired,
    Revoked,
    /// A state this build does not know: never treated as active.
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceView {
    pub label: String,
    /// `a1b2-c3d4-e5f6-0718`.
    pub key_fingerprint: String,
    pub created_at: Iso,
    pub expires_at: Iso,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceView {
    pub source: crate::contract::Source,
    pub state: SourceState,
    pub parser_version: crate::contract::ParserVersion,
    /// What the owner authorized, in catalog order.
    pub fields: Vec<String>,
    pub window_start: Option<Iso>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GapView {
    pub starts_at: Iso,
    pub ends_at: Iso,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollectionView {
    pub source: crate::contract::Source,
    pub last_seen_at: Option<Iso>,
    pub observed_until: Option<Iso>,
    pub open_gaps: Vec<GapView>,
}

/// A status answer that passed validation: everything in it is safe to print.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusView {
    pub organization_name: String,
    pub member_name: Option<String>,
    pub device: DeviceView,
    pub sources: Vec<SourceView>,
    pub collection: Vec<CollectionView>,
}

const MAX_LISTED: usize = 8;
const MAX_FIELDS: usize = 64;
const MAX_GAPS: usize = 20;

fn is_fingerprint(text: &str) -> bool {
    text.len() == 19
        && text.split('-').count() == 4
        && text.split('-').all(|g| {
            g.len() == 4
                && g.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
}

fn optional_time(text: Option<&str>) -> Option<Option<Iso>> {
    match text {
        None => Some(None),
        Some(t) => Iso::parse(t).map(Some),
    }
}

fn source_of(name: &str) -> Option<crate::contract::Source> {
    match name {
        "claude_code" => Some(crate::contract::Source::ClaudeCode),
        "codex" => Some(crate::contract::Source::Codex),
        _ => None,
    }
}

impl CliStatus {
    /// `None` if any field is malformed. Sources this build does not know are left out.
    pub fn checked(self) -> Option<StatusView> {
        if self.sources.len() > MAX_LISTED || self.collection.len() > MAX_LISTED {
            return None;
        }
        let d = self.device;
        if !is_fingerprint(&d.key_fingerprint) || !printable(&d.label, 200) {
            return None;
        }
        let device = DeviceView {
            label: d.label,
            key_fingerprint: d.key_fingerprint,
            created_at: Iso::parse(&d.created_at)?,
            expires_at: Iso::parse(&d.expires_at)?,
        };
        let mut sources = Vec::new();
        for s in self.sources {
            let Some(source) = source_of(&s.source) else {
                continue;
            };
            if s.fields.len() > MAX_FIELDS || !s.fields.iter().all(|f| printable(f, 200)) {
                return None;
            }
            sources.push(SourceView {
                source,
                state: match s.state.as_str() {
                    "active" => SourceState::Active,
                    "expired" => SourceState::Expired,
                    "revoked" => SourceState::Revoked,
                    _ => SourceState::Other,
                },
                parser_version: crate::contract::ParserVersion::new(&s.parser_version)?,
                fields: s.fields,
                window_start: optional_time(s.window_start.as_deref())?,
            });
        }
        let mut collection = Vec::new();
        for c in self.collection {
            let Some(source) = source_of(&c.source) else {
                continue;
            };
            let mut open_gaps = Vec::new();
            for g in c.open_gaps.into_iter().take(MAX_GAPS) {
                open_gaps.push(GapView {
                    starts_at: Iso::parse(&g.starts_at)?,
                    ends_at: Iso::parse(&g.ends_at)?,
                    reason: if is_plain_reason(&g.reason) {
                        g.reason
                    } else {
                        "unknown".into()
                    },
                });
            }
            collection.push(CollectionView {
                source,
                last_seen_at: optional_time(c.last_seen_at.as_deref())?,
                observed_until: optional_time(c.observed_until.as_deref())?,
                open_gaps,
            });
        }
        let member_name = match self.member_name {
            Some(name) if printable(&name, 200) => Some(name),
            Some(_) => return None,
            None => None,
        };
        Some(StatusView {
            organization_name: printable(&self.organization_name, 200)
                .then_some(self.organization_name)?,
            member_name,
            device,
            sources,
            collection,
        })
    }
}

/// A gap reason is a short code like `buffer_full`.
fn is_plain_reason(text: &str) -> bool {
    (1..=64).contains(&text.len())
        && text
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}
