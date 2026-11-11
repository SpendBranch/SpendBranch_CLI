//! The `ingest/v1` contract (schema/ingest-v1.json) as Rust types. Pure: no I/O.
//! Every text type validates in its constructor, so an invalid value cannot exist.
//! Only deltas travel: the server never sums totals.
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use std::fmt;

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

macro_rules! text_type {
    ($(#[$doc:meta])* $name:ident, $check:expr) => {
        $(#[$doc])*
        #[derive(Clone, Debug, PartialEq, Eq, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let text = String::deserialize(d)?;
                // The message never repeats the text: it may come from a tampered file.
                Self::new(&text).ok_or_else(|| D::Error::custom(concat!("invalid ", stringify!($name))))
            }
        }
        impl $name {
            /// `None` when the text breaks the contract's pattern or length.
            pub fn new(text: &str) -> Option<Self> {
                let check: fn(&str) -> bool = $check;
                check(text).then(|| Self(text.to_owned()))
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

fn all(text: &str, max: usize, ok: impl Fn(char) -> bool) -> bool {
    (1..=max).contains(&text.len()) && text.chars().all(ok)
}

text_type!(
    /// `^[A-Za-z0-9_:-]{1,128}$`
    ExternalId,
    |t| all(t, 128, |c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '-'))
);
text_type!(
    /// `^[A-Za-z0-9<>._-]{1,64}$`
    Model,
    |t| all(t, 64, |c| c.is_ascii_alphanumeric() || matches!(c, '<' | '>' | '.' | '_' | '-'))
);
text_type!(
    /// `^[A-Za-z0-9._+-]{1,32}$`: the agent's own version.
    SourceVersion,
    |t| all(t, 32, |c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
);
text_type!(
    /// `^[A-Za-z0-9._:-]{1,200}$`: the provider account that pays.
    AccountRef,
    |t| all(t, 200, |c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-'))
);
text_type!(
    /// `^[a-z0-9-]{1,48}/v[0-9]{1,4}$`, e.g. `claude-code/v2`.
    ParserVersion,
    |t| {
        let Some((name, v)) = t.split_once("/v") else { return false };
        all(name, 48, |c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            && all(v, 4, |c| c.is_ascii_digit())
    }
);
text_type!(
    /// `host[:port]/owner/name[/...]`, at most 200 characters.
    Repository,
    |t| {
        let mut parts = t.split('/');
        let host_ok = parts.next().is_some_and(|h| {
            let (host, port) = h.split_once(':').map_or((h, None), |(a, b)| (a, Some(b)));
            all(host, 200, |c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-'))
                && port.is_none_or(|p| all(p, 5, |c| c.is_ascii_digit()))
        });
        let segments: Vec<&str> = parts.collect();
        t.len() <= 200
            && host_ok
            && segments.len() >= 2
            && segments.iter().all(|s| {
                all(s, 200, |c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '~' | '-'))
            })
    }
);
text_type!(
    /// 1 to 255 code points, none of them whitespace, control or invisible format characters.
    Branch,
    |t| {
        (1..=255).contains(&t.chars().count()) && t.chars().all(is_branch_char)
    }
);

/// No whitespace, no control (Cc) and no format (Cf: bidi marks, zero-width, BOM...) characters.
fn is_branch_char(c: char) -> bool {
    !(c.is_whitespace() || c.is_control() || is_format_char(c))
}

/// Unicode general category Cf. Std has no such table; this is the Unicode 16 list.
fn is_format_char(c: char) -> bool {
    matches!(u32::from(c),
        0xAD | 0x600..=0x605 | 0x61C | 0x6DD | 0x70F | 0x890..=0x891 | 0x8E2 | 0x180E
        | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064 | 0x2066..=0x206F | 0xFEFF
        | 0xFFF9..=0xFFFB | 0x110BD | 0x110CD | 0x13430..=0x1343F | 0x1BCA0..=0x1BCA3
        | 0x1D173..=0x1D17A | 0xE0001 | 0xE0020..=0xE007F)
}

/// A token counter. `None` means unknown and is sent as `null`: unknown is never zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Count(Option<u64>);

impl<'de> Deserialize<'de> for Count {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match Option::<u64>::deserialize(d)? {
            None => Ok(Count::UNKNOWN),
            Some(n) => Count::new(n).ok_or_else(|| D::Error::custom("counter above the ceiling")),
        }
    }
}

/// A counter that is present even when it is `null` (unknown): present and unknown differ
/// from absent only in the text, and we keep the text as it was written.
fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Count>, D::Error> {
    Count::deserialize(d).map(Some)
}

impl Count {
    pub const UNKNOWN: Count = Count(None);
    /// `None` above 1e12, the contract's ceiling.
    pub fn new(value: u64) -> Option<Self> {
        (value <= MAX_TOKENS_PER_COUNTER).then_some(Self(Some(value)))
    }
    pub fn get(self) -> Option<u64> {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UsageCounters {
    pub input_tokens: Count,
    pub output_tokens: Count,
    pub cache_read_tokens: Count,
    pub cache_write_tokens: Count,
    pub reasoning_tokens: Count,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub cache_write_1h_tokens: Option<Count>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub web_search_requests: Option<Count>,
}

/// A point in time with millisecond precision, written as `YYYY-MM-DDTHH:MM:SS.mmmZ`.
/// Limited to 1970..9999 so the text is always a valid contract datetime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Iso(i64);

impl Iso {
    const MAX_MS: i64 = 253_402_300_799_999;
    pub const EPOCH: Iso = Iso(0);

    pub fn from_millis(ms: i64) -> Option<Self> {
        (0..=Self::MAX_MS).contains(&ms).then_some(Self(ms))
    }

    /// RFC 3339 with an offset or `Z` (what the agents write) and a real calendar date.
    /// Anything else is `None`.
    pub fn parse(text: &str) -> Option<Self> {
        let ts: jiff::Timestamp = text.parse().ok()?;
        Self::from_millis(ts.as_millisecond())
    }

    pub fn millis(self) -> i64 {
        self.0
    }
}

impl fmt::Display for Iso {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ts = jiff::Timestamp::from_millisecond(self.0).map_err(|_| fmt::Error)?;
        write!(f, "{}", ts.strftime("%Y-%m-%dT%H:%M:%S.%3fZ"))
    }
}

impl<'de> Deserialize<'de> for Iso {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        Iso::parse(&text).ok_or_else(|| D::Error::custom("invalid datetime"))
    }
}

impl Serialize for Iso {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

/// `derived_from_cumulative` only exists together with the event it was derived from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "counterOrigin", rename_all = "snake_case")]
pub enum CounterOrigin {
    ReportedDelta,
    DerivedFromCumulative {
        #[serde(rename = "baseEventId")]
        base_event_id: ExternalId,
    },
}

/// Identity of an event: (`source`, `external_session_id`, `external_event_id`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", try_from = "RawObservation")]
pub struct UsageObservation {
    pub source: Source,
    pub external_session_id: ExternalId,
    pub external_event_id: ExternalId,
    pub occurred_at: Iso,
    pub model: Model,
    pub agent_role: AgentRole,
    pub counters: UsageCounters,
    #[serde(flatten)]
    pub origin: CounterOrigin,
    pub parser_version: ParserVersion,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_version: Option<SourceVersion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payer_account_ref: Option<AccountRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payer_evidence: Option<PayerEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub billing_mode: Option<BillingMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<Repository>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<Branch>,
}

/// An observation as written, before the origin and its base event are checked together.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawObservation {
    source: Source,
    external_session_id: ExternalId,
    external_event_id: ExternalId,
    occurred_at: Iso,
    model: Model,
    agent_role: AgentRole,
    counters: UsageCounters,
    counter_origin: RawOrigin,
    base_event_id: Option<ExternalId>,
    parser_version: ParserVersion,
    source_version: Option<SourceVersion>,
    payer_account_ref: Option<AccountRef>,
    payer_evidence: Option<PayerEvidence>,
    billing_mode: Option<BillingMode>,
    repository: Option<Repository>,
    branch: Option<Branch>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum RawOrigin {
    ReportedDelta,
    DerivedFromCumulative,
}

impl TryFrom<RawObservation> for UsageObservation {
    type Error = &'static str;

    fn try_from(r: RawObservation) -> Result<Self, Self::Error> {
        let origin = match (r.counter_origin, r.base_event_id) {
            (RawOrigin::ReportedDelta, None) => CounterOrigin::ReportedDelta,
            (RawOrigin::DerivedFromCumulative, Some(base_event_id)) => {
                CounterOrigin::DerivedFromCumulative { base_event_id }
            }
            _ => return Err("baseEventId is required only with derived_from_cumulative"),
        };
        Ok(Self {
            source: r.source,
            external_session_id: r.external_session_id,
            external_event_id: r.external_event_id,
            occurred_at: r.occurred_at,
            model: r.model,
            agent_role: r.agent_role,
            counters: r.counters,
            origin,
            parser_version: r.parser_version,
            source_version: r.source_version,
            payer_account_ref: r.payer_account_ref,
            payer_evidence: r.payer_evidence,
            billing_mode: r.billing_mode,
            repository: r.repository,
            branch: r.branch,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapReason {
    BufferFull,
    AdapterError,
    SourceMissing,
    UnsupportedFormat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CoverageState {
    Observed,
    Unavailable { reason: GapReason },
}

/// A missing-data gap or an observed range. Missing data is a gap, never zero.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawCoverage")]
pub struct CoverageReport {
    source: Source,
    from: Iso,
    to: Iso,
    #[serde(flatten)]
    state: CoverageState,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCoverage {
    source: Source,
    from: Iso,
    to: Iso,
    state: RawState,
    reason: Option<GapReason>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum RawState {
    Observed,
    Unavailable,
}

impl TryFrom<RawCoverage> for CoverageReport {
    type Error = &'static str;

    fn try_from(r: RawCoverage) -> Result<Self, Self::Error> {
        let state = match (r.state, r.reason) {
            (RawState::Observed, None) => CoverageState::Observed,
            (RawState::Unavailable, Some(reason)) => CoverageState::Unavailable { reason },
            _ => return Err("reason is required only with unavailable"),
        };
        CoverageReport::new(r.source, r.from, r.to, state).ok_or("from must precede to")
    }
}

impl CoverageReport {
    pub fn source(&self) -> Source {
        self.source
    }
    pub fn from(&self) -> Iso {
        self.from
    }
    pub fn to(&self) -> Iso {
        self.to
    }
    pub fn state(&self) -> CoverageState {
        self.state
    }

    /// A gap of one second that ends at `end` (at least 1 ms after the epoch). Never fails.
    pub fn unavailable_ending(source: Source, end: Iso, reason: GapReason) -> Self {
        let to = Iso(end.0.max(1));
        Self {
            source,
            from: Iso((to.0 - 1000).max(0)),
            to,
            state: CoverageState::Unavailable { reason },
        }
    }

    /// `None` unless `from` precedes `to`.
    pub fn new(source: Source, from: Iso, to: Iso, state: CoverageState) -> Option<Self> {
        (from < to).then_some(Self {
            source,
            from,
            to,
            state,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IngestBatch {
    contract_version: &'static str,
    installation_id: InstallationId,
    observations: Vec<UsageObservation>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    coverage: Vec<CoverageReport>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct InstallationId(uuid::Uuid);

impl Serialize for InstallationId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(&self.0.hyphenated())
    }
}

impl IngestBatch {
    /// `None` when the batch breaks a limit (500 observations, 50 coverage reports), is
    /// empty, or the id is not a standard UUID.
    pub fn new(
        installation_id: uuid::Uuid,
        observations: Vec<UsageObservation>,
        coverage: Vec<CoverageReport>,
    ) -> Option<Self> {
        let standard = installation_id.is_nil()
            || installation_id.is_max()
            || ((1..=8).contains(&installation_id.get_version_num())
                && installation_id.get_variant() == uuid::Variant::RFC4122);
        (standard
            && observations.len() <= MAX_BATCH_OBSERVATIONS
            && coverage.len() <= MAX_BATCH_COVERAGE
            && observations.len() + coverage.len() > 0)
            .then_some(Self {
                contract_version: CONTRACT_VERSION,
                installation_id: InstallationId(installation_id),
                observations,
                coverage,
            })
    }
}
