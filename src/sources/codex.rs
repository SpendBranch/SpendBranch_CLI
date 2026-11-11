//! `codex/v1`: Codex rollout text to usage observations. Pure and deterministic.
//! Counters come from each response's `token_usage_record`, never from differences of
//! the running total.
use super::{
    Format,
    json::{Leaf, Obj},
    key_hash,
    repository::repository_key_of,
    version,
};
use crate::contract::{
    AccountRef, AgentRole, BillingMode, Branch, Count, CounterOrigin, ExternalId, Iso,
    MAX_TOKENS_PER_COUNTER, Model, ParserVersion, PayerEvidence, Repository, Source, SourceVersion,
    UsageCounters, UsageObservation,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

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

// Only allowlisted fields are named; serde skips the rest unstored. A repeated key in one
// line makes it unsupported (the reference keeps the last one).
#[derive(Deserialize, Default)]
#[serde(default)]
struct Line {
    timestamp: Leaf,
    #[serde(rename = "type")]
    kind: Leaf,
    ordinal: Leaf,
    payload: Obj<Payload>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Payload {
    // session_meta
    id: Leaf,
    session_id: Leaf,
    parent_thread_id: Leaf,
    cli_version: Leaf,
    model_provider: Leaf,
    creator_account_id: Leaf,
    git: Obj<Git>,
    // turn_context
    turn_id: Leaf,
    model: Leaf,
    // event_msg
    #[serde(rename = "type")]
    kind: Leaf,
    thread_settings: Obj<ThreadSettings>,
    rate_limits: Obj<RateLimits>,
    // token_usage_record
    thread_id: Leaf,
    response_id: Leaf,
    usage: Obj<RawUsage>,
    thread_token_usage: Obj<RawUsage>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Git {
    repository_url: Leaf,
    branch: Leaf,
}
#[derive(Deserialize, Default)]
#[serde(default)]
struct ThreadSettings {
    model: Leaf,
}
#[derive(Deserialize, Default)]
#[serde(default)]
struct RateLimits {
    plan_type: Leaf,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawUsage {
    input_tokens: Leaf,
    cached_input_tokens: Leaf,
    cache_write_input_tokens: Leaf,
    output_tokens: Leaf,
    reasoning_output_tokens: Leaf,
    total_tokens: Leaf,
}

/// input, cached input, cache write, output, reasoning, total.
type Usage = [u64; 6];

fn usage_of(raw: &Obj<RawUsage>) -> Option<Usage> {
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

struct Record {
    line: usize,
    ordinal: i64,
    at: Iso,
    turn_id: String,
    response_id: String,
    usage: Usage,
    thread: Option<Usage>,
    model_at_time: Option<String>,
    after_broken_line: bool,
}

fn text(leaf: &Leaf, valid: impl Fn(&str) -> bool) -> Option<&str> {
    leaf.str().filter(|s| valid(s))
}
fn is_id(s: &str) -> bool {
    ExternalId::new(s).is_some()
}
fn is_model(s: &str) -> bool {
    Model::new(s).is_some()
}

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

fn meta_of(p: &Payload) -> Option<Meta> {
    let thread_id = text(&p.id, is_id)?.to_owned();
    let git = p.git.0.as_ref();
    Some(Meta {
        session_id: text(&p.session_id, is_id).map_or_else(|| thread_id.clone(), str::to_owned),
        subagent: text(&p.parent_thread_id, is_id).is_some(),
        cli_version: text(&p.cli_version, |v| SourceVersion::new(v).is_some()).map(str::to_owned),
        account: (p.model_provider.str() == Some("openai"))
            .then(|| text(&p.creator_account_id, |a| AccountRef::new(a).is_some()))
            .flatten()
            .map(str::to_owned),
        repository: git
            .and_then(|g| g.repository_url.str())
            .and_then(repository_key_of),
        branch: git
            .and_then(|g| text(&g.branch, |b| Branch::new(b).is_some()))
            .map(str::to_owned),
        thread_id,
    })
}

/// Turn to model, oldest forgotten first. Bounded so a resume state stays small.
#[derive(Default)]
struct TurnModels {
    map: HashMap<String, String>,
    order: VecDeque<String>,
}

impl TurnModels {
    const KEEP: usize = 4096;

    fn from_pairs(pairs: Vec<(String, String)>) -> Self {
        let mut t = Self::default();
        for (turn, model) in pairs {
            t.insert(turn, model);
        }
        t
    }

    fn insert(&mut self, turn: String, model: String) {
        if self.map.insert(turn.clone(), model).is_none() {
            self.order.push_back(turn);
            if self.order.len() > Self::KEEP
                && let Some(old) = self.order.pop_front()
            {
                self.map.remove(&old);
            }
        }
    }

    /// The most recent `RESUME_KEEP` turns, oldest first: what a saved state needs. A response
    /// of an older turn falls back to the model in force when it was written.
    fn into_pairs(self) -> Vec<(String, String)> {
        const RESUME_KEEP: usize = 64;
        let skip = self.order.len().saturating_sub(RESUME_KEEP);
        let mut map = self.map;
        self.order
            .into_iter()
            .skip(skip)
            .filter_map(|t| map.remove(&t).map(|m| (t, m)))
            .collect()
    }
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

/// Incremental form of `read`, for readers that stream a rollout line by line, optionally
/// continuing from a `Resume`. Memory grows with the number of responses, not with the text.
pub struct Parser<'a> {
    options: &'a ReadOptions<'a>,
    after_ordinal: i64,
    line: usize,
    unsupported: Vec<UnsupportedLine>,
    turn_models: TurnModels,
    records: Vec<Record>,
    by_response: HashMap<String, Usage>,
    meta: Option<Meta>,
    current_model: Option<String>,
    plan_seen: bool,
    token_count_seen: bool,
    had_record: bool,
    last_ordinal: i64,
    foreign: usize,
    broken_since_last_record: bool,
    prev_thread: Option<Usage>,
    /// The first line was not a session meta, or a second one named another thread.
    unknown_format: bool,
}

impl<'a> Parser<'a> {
    pub fn new(options: &'a ReadOptions<'a>) -> Self {
        Self {
            options,
            after_ordinal: options.after_ordinal.unwrap_or(-1),
            line: 0,
            unsupported: Vec::new(),
            turn_models: TurnModels::default(),
            records: Vec::new(),
            by_response: HashMap::new(),
            meta: None,
            current_model: None,
            plan_seen: false,
            token_count_seen: false,
            had_record: false,
            last_ordinal: -1,
            foreign: 0,
            broken_since_last_record: false,
            prev_thread: None,
            unknown_format: false,
        }
    }

    /// Continues a rollout whose beginning was read before.
    pub fn resume(options: &'a ReadOptions<'a>, state: Resume) -> Self {
        let mut p = Self::new(options);
        p.meta = Some(state.meta);
        p.current_model = state.current_model;
        p.turn_models = TurnModels::from_pairs(state.turn_models);
        p.plan_seen = state.plan_seen;
        p.had_record = state.had_record;
        p.last_ordinal = state.last_ordinal;
        p.prev_thread = state.prev_thread;
        p.broken_since_last_record = state.broken_since_last_record;
        p
    }

    /// The thread this rollout belongs to, known once its first line was read.
    pub fn thread_id(&self) -> Option<&str> {
        self.meta.as_ref().map(|m| m.thread_id.as_str())
    }

    /// Only records with a greater ordinal are emitted. A reader learns which thread it is
    /// reading from the first line, so it sets this before `finish`.
    pub fn set_after_ordinal(&mut self, after: i64) {
        self.after_ordinal = after;
    }

    /// A line the reader refused to load. It breaks the running-total check like any bad line.
    pub fn push_unsupported_line(&mut self) {
        self.line += 1;
        self.unsupported.push(UnsupportedLine {
            line: self.line,
            code: UnsupportedCode::InvalidJson,
        });
        self.broken_since_last_record = true;
    }

    /// One complete line, without its line break.
    pub fn push_line(&mut self, raw: &str) {
        self.line += 1;
        let n = self.line;
        if self.unknown_format || raw.trim().is_empty() {
            return;
        }
        let entry = match serde_json::from_str::<Obj<Line>>(raw) {
            Ok(Obj(Some(entry))) => entry,
            Ok(Obj(None)) => {
                self.unsupported.push(UnsupportedLine {
                    line: n,
                    code: UnsupportedCode::NotAnObject,
                });
                self.broken_since_last_record = true;
                return;
            }
            Err(_) => {
                self.unsupported.push(UnsupportedLine {
                    line: n,
                    code: UnsupportedCode::InvalidJson,
                });
                self.broken_since_last_record = true;
                return;
            }
        };
        let payload = entry.payload.0.as_ref();
        let none = Payload::default();
        let p = payload.unwrap_or(&none);
        let ordinal = entry.ordinal.safe_int();
        if let Some(o) = ordinal {
            self.last_ordinal = self.last_ordinal.max(o);
        }
        let kind = entry.kind.str();

        let Some(meta) = self.meta.as_ref() else {
            match (kind == Some("session_meta"))
                .then(|| meta_of(p))
                .flatten()
                .filter(|_| payload.is_some())
            {
                Some(meta) => self.meta = Some(meta),
                None => self.unknown_format = true,
            }
            return;
        };
        if kind == Some("session_meta") {
            if p.id.str() != Some(meta.thread_id.as_str()) {
                self.unknown_format = true;
            }
            return;
        }
        if kind == Some("turn_context") {
            if let Some(model) = text(&p.model, is_model) {
                self.current_model = Some(model.to_owned());
                if let Some(turn) = text(&p.turn_id, is_id) {
                    self.turn_models.insert(turn.to_owned(), model.to_owned());
                }
            }
            return;
        }
        if kind == Some("event_msg") && p.kind.str() == Some("thread_settings_applied") {
            if let Some(model) = p
                .thread_settings
                .0
                .as_ref()
                .and_then(|t| text(&t.model, is_model))
            {
                self.current_model = Some(model.to_owned());
            }
            return;
        }
        if kind == Some("event_msg") && p.kind.str() == Some("token_count") {
            self.token_count_seen = true;
            self.plan_seen |= p
                .rate_limits
                .0
                .as_ref()
                .is_some_and(|r| r.plan_type.is_present());
            return;
        }
        if kind != Some("token_usage_record") {
            return;
        }
        if p.thread_id != Leaf::Missing && p.thread_id.str() != Some(meta.thread_id.as_str()) {
            self.foreign += 1;
            return;
        }
        let response_id = text(&p.response_id, is_id);
        let turn_id = text(&p.turn_id, is_id);
        let usage = usage_of(&p.usage);
        let at = entry.timestamp.str().and_then(Iso::parse);
        let (Some(response_id), Some(turn_id), Some(usage), Some(at), Some(ordinal)) =
            (response_id, turn_id, usage, at, ordinal)
        else {
            self.unsupported.push(UnsupportedLine {
                line: n,
                code: UnsupportedCode::MissingField,
            });
            self.broken_since_last_record = true;
            return;
        };
        if usage[1] + usage[2] > usage[0] {
            self.unsupported.push(UnsupportedLine {
                line: n,
                code: UnsupportedCode::InconsistentCounters,
            });
            self.broken_since_last_record = true;
            return;
        }
        if let Some(known) = self.by_response.get(response_id) {
            if *known != usage {
                self.unsupported.push(UnsupportedLine {
                    line: n,
                    code: UnsupportedCode::ConflictingResponse,
                });
                self.broken_since_last_record = true;
            }
            return;
        }
        self.by_response.insert(response_id.to_owned(), usage);
        self.records.push(Record {
            line: n,
            ordinal,
            at,
            turn_id: turn_id.to_owned(),
            response_id: response_id.to_owned(),
            usage,
            thread: usage_of(&p.thread_token_usage),
            model_at_time: self.current_model.clone(),
            after_broken_line: self.broken_since_last_record,
        });
        self.broken_since_last_record = false;
    }

    /// The observations, and where to continue from (`None` when this read could not
    /// understand the rollout, so the caller must not advance).
    pub fn finish(self) -> (ReadResult, Option<Resume>) {
        if self.unknown_format {
            return (empty(Format::UnsupportedFormat, vec![]), None);
        }
        let Some(meta) = self.meta.clone() else {
            return (empty(Format::Ok, self.unsupported), None);
        };
        let records = &self.records;
        if self.token_count_seen && records.is_empty() && !self.had_record {
            return (empty(Format::UnsupportedFormat, self.unsupported), None);
        }
        // Between two consecutive records with no broken line in between, the step of the
        // running total must equal the record's own usage. It is only a local check.
        let mut before = self.prev_thread;
        for now in records {
            if let (Some(a), Some(b)) = (before, now.thread)
                && !now.after_broken_line
                && (0..6).any(|i| i128::from(b[i]) - i128::from(a[i]) != i128::from(now.usage[i]))
            {
                return (empty(Format::UnsupportedFormat, self.unsupported), None);
            }
            before = now.thread;
        }

        let after = self.after_ordinal;
        let mut unsupported = self.unsupported;
        let mut observations = Vec::new();
        let mut new_keys = Vec::new();
        for record in records {
            if record.ordinal <= after
                || self
                    .options
                    .seen
                    .is_some_and(|seen| seen(key_hash(&record.response_id)))
            {
                continue;
            }
            let Some(model) = self
                .turn_models
                .map
                .get(&record.turn_id)
                .or(record.model_at_time.as_ref())
            else {
                unsupported.push(UnsupportedLine {
                    line: record.line,
                    code: UnsupportedCode::UnresolvedModel,
                });
                continue;
            };
            match observation_of(&meta, record, model, self.plan_seen) {
                Some(o) => {
                    observations.push(o);
                    new_keys.push(record.response_id.clone());
                }
                None => unsupported.push(UnsupportedLine {
                    line: record.line,
                    code: UnsupportedCode::Contract,
                }),
            }
        }
        unsupported.sort_by_key(|u| u.line);

        let resume = Resume {
            meta: meta.clone(),
            current_model: self.current_model,
            turn_models: self.turn_models.into_pairs(),
            plan_seen: self.plan_seen,
            had_record: self.had_record || !records.is_empty(),
            last_ordinal: self.last_ordinal,
            prev_thread: records.last().map_or(self.prev_thread, |r| r.thread),
            broken_since_last_record: self.broken_since_last_record,
        };
        let result = ReadResult {
            thread_id: Some(meta.thread_id.clone()),
            session_id: Some(meta.session_id.clone()),
            observations,
            new_keys,
            last_ordinal: self.last_ordinal,
            foreign_records: self.foreign,
            unsupported_lines: unsupported,
            untested_version: meta
                .cli_version
                .filter(|v| !version::in_range(v, &version::CODEX_TESTED)),
            format: Format::Ok,
        };
        (result, Some(resume))
    }
}

fn observation_of(
    meta: &Meta,
    record: &Record,
    model: &str,
    plan_seen: bool,
) -> Option<UsageObservation> {
    let u = record.usage;
    let c = Count::new;
    Some(UsageObservation {
        source: Source::Codex,
        external_session_id: ExternalId::new(&meta.session_id)?,
        external_event_id: ExternalId::new(&record.response_id)?,
        occurred_at: record.at,
        model: Model::new(model)?,
        agent_role: if meta.subagent {
            AgentRole::Subagent
        } else {
            AgentRole::Main
        },
        counters: UsageCounters {
            input_tokens: c(u[0] - u[1] - u[2])?,
            output_tokens: c(u[3])?,
            cache_read_tokens: c(u[1])?,
            cache_write_tokens: c(u[2])?,
            reasoning_tokens: c(u[4])?,
            cache_write_1h_tokens: None,
            web_search_requests: None,
        },
        origin: CounterOrigin::ReportedDelta,
        parser_version: ParserVersion::new(PARSER)?,
        source_version: meta.cli_version.as_deref().and_then(SourceVersion::new),
        payer_account_ref: meta.account.as_deref().and_then(AccountRef::new),
        payer_evidence: meta.account.as_ref().map(|_| PayerEvidence::Transcript),
        billing_mode: (meta.account.is_some() && plan_seen).then_some(BillingMode::Subscription),
        repository: match meta.repository.as_deref() {
            Some(r) => Some(Repository::new(r)?),
            None => None,
        },
        branch: meta.branch.as_deref().and_then(Branch::new),
    })
}
