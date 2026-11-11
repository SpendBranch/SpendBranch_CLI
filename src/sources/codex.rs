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
    MAX_TOKENS_PER_COUNTER, Model, ParserVersion, PayerEvidence, Source, SourceVersion,
    UsageCounters, UsageObservation,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

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
    /// Hashes (see `key_hash`) of the response ids already sent.
    pub seen: Option<&'a HashSet<u64>>,
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

pub fn read(input: &str, options: &ReadOptions) -> ReadResult {
    // The last piece has no line break yet: Codex may still be writing it.
    let complete = input.rfind('\n').map_or("", |end| &input[..end]);
    let lines: Box<dyn Iterator<Item = &str>> = if input.contains('\n') {
        Box::new(complete.split('\n'))
    } else {
        Box::new(std::iter::empty())
    };

    let mut unsupported: Vec<UnsupportedLine> = Vec::new();
    let mut turn_models: HashMap<String, String> = HashMap::new();
    let mut records: Vec<Record> = Vec::new();
    let mut by_response: HashMap<String, Usage> = HashMap::new();
    let mut meta: Option<Payload> = None;
    let mut current_model: Option<String> = None;
    let (mut plan_seen, mut token_count_seen) = (false, false);
    let mut last_ordinal = -1i64;
    let mut foreign = 0usize;
    let mut broken_since_last_record = false;

    for (index, raw) in lines.enumerate() {
        let n = index + 1;
        if raw.trim().is_empty() {
            continue;
        }
        let entry = match serde_json::from_str::<Obj<Line>>(raw) {
            Ok(Obj(Some(entry))) => entry,
            Ok(Obj(None)) => {
                unsupported.push(UnsupportedLine {
                    line: n,
                    code: UnsupportedCode::NotAnObject,
                });
                broken_since_last_record = true;
                continue;
            }
            Err(_) => {
                unsupported.push(UnsupportedLine {
                    line: n,
                    code: UnsupportedCode::InvalidJson,
                });
                broken_since_last_record = true;
                continue;
            }
        };
        let payload = entry.payload.0.as_ref();
        let none = Payload::default();
        let p = payload.unwrap_or(&none);
        let ordinal = entry.ordinal.safe_int();
        if let Some(o) = ordinal {
            last_ordinal = last_ordinal.max(o);
        }
        let kind = entry.kind.str();

        let Some(meta_payload) = meta.as_ref() else {
            if kind != Some("session_meta") || payload.is_none() || text(&p.id, is_id).is_none() {
                return empty(Format::UnsupportedFormat, vec![]);
            }
            meta = entry.payload.0;
            continue;
        };
        if kind == Some("session_meta") {
            if p.id != meta_payload.id {
                return empty(Format::UnsupportedFormat, vec![]);
            }
            continue;
        }
        if kind == Some("turn_context") {
            if let Some(model) = text(&p.model, is_model) {
                current_model = Some(model.to_owned());
                if let Some(turn) = text(&p.turn_id, is_id) {
                    turn_models.insert(turn.to_owned(), model.to_owned());
                }
            }
            continue;
        }
        if kind == Some("event_msg") && p.kind.str() == Some("thread_settings_applied") {
            if let Some(model) = p
                .thread_settings
                .0
                .as_ref()
                .and_then(|t| text(&t.model, is_model))
            {
                current_model = Some(model.to_owned());
            }
            continue;
        }
        if kind == Some("event_msg") && p.kind.str() == Some("token_count") {
            token_count_seen = true;
            plan_seen |= p
                .rate_limits
                .0
                .as_ref()
                .is_some_and(|r| r.plan_type.is_present());
            continue;
        }
        if kind != Some("token_usage_record") {
            continue;
        }
        if p.thread_id != Leaf::Missing && p.thread_id != meta_payload.id {
            foreign += 1;
            continue;
        }
        let response_id = text(&p.response_id, is_id);
        let turn_id = text(&p.turn_id, is_id);
        let usage = usage_of(&p.usage);
        let at = entry.timestamp.str().and_then(Iso::parse);
        let (Some(response_id), Some(turn_id), Some(usage), Some(at), Some(ordinal)) =
            (response_id, turn_id, usage, at, ordinal)
        else {
            unsupported.push(UnsupportedLine {
                line: n,
                code: UnsupportedCode::MissingField,
            });
            broken_since_last_record = true;
            continue;
        };
        if usage[1] + usage[2] > usage[0] {
            unsupported.push(UnsupportedLine {
                line: n,
                code: UnsupportedCode::InconsistentCounters,
            });
            broken_since_last_record = true;
            continue;
        }
        if let Some(known) = by_response.get(response_id) {
            if *known != usage {
                unsupported.push(UnsupportedLine {
                    line: n,
                    code: UnsupportedCode::ConflictingResponse,
                });
                broken_since_last_record = true;
            }
            continue;
        }
        by_response.insert(response_id.to_owned(), usage);
        records.push(Record {
            line: n,
            ordinal,
            at,
            turn_id: turn_id.to_owned(),
            response_id: response_id.to_owned(),
            usage,
            thread: usage_of(&p.thread_token_usage),
            model_at_time: current_model.clone(),
            after_broken_line: broken_since_last_record,
        });
        broken_since_last_record = false;
    }

    let Some(meta) = meta else {
        return empty(Format::Ok, unsupported);
    };
    if token_count_seen && records.is_empty() {
        return empty(Format::UnsupportedFormat, unsupported);
    }
    // Between two consecutive records with no broken line in between, the step of the
    // running total must equal the record's own usage. It is only a local check.
    for pair in records.windows(2) {
        let (before, now) = (&pair[0], &pair[1]);
        let (Some(a), Some(b)) = (before.thread, now.thread) else {
            continue;
        };
        if now.after_broken_line {
            continue;
        }
        if (0..6).any(|i| i128::from(b[i]) - i128::from(a[i]) != i128::from(now.usage[i])) {
            return empty(Format::UnsupportedFormat, unsupported);
        }
    }

    let thread_id = meta.id.str().unwrap_or_default().to_owned();
    let session_id = text(&meta.session_id, is_id).map_or_else(|| thread_id.clone(), str::to_owned);
    let subagent = text(&meta.parent_thread_id, is_id).is_some();
    let cli_version = text(&meta.cli_version, |v| SourceVersion::new(v).is_some());
    let account = (meta.model_provider.str() == Some("openai"))
        .then(|| text(&meta.creator_account_id, |a| AccountRef::new(a).is_some()))
        .flatten();
    let git = meta.git.0.as_ref();
    let repository = git
        .and_then(|g| g.repository_url.str())
        .and_then(repository_key_of);
    let branch = git.and_then(|g| text(&g.branch, |b| Branch::new(b).is_some()));
    let after = options.after_ordinal.unwrap_or(-1);

    let mut observations = Vec::new();
    let mut new_keys = Vec::new();
    for record in &records {
        if record.ordinal <= after
            || options
                .seen
                .is_some_and(|s| s.contains(&key_hash(&record.response_id)))
        {
            continue;
        }
        let Some(model) = turn_models
            .get(&record.turn_id)
            .or(record.model_at_time.as_ref())
        else {
            unsupported.push(UnsupportedLine {
                line: record.line,
                code: UnsupportedCode::UnresolvedModel,
            });
            continue;
        };
        let u = record.usage;
        let built = (|| {
            let c = |v: u64| Count::new(v);
            Some(UsageObservation {
                source: Source::Codex,
                external_session_id: ExternalId::new(&session_id)?,
                external_event_id: ExternalId::new(&record.response_id)?,
                occurred_at: record.at,
                model: Model::new(model)?,
                agent_role: if subagent {
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
                source_version: cli_version.and_then(SourceVersion::new),
                payer_account_ref: account.and_then(AccountRef::new),
                payer_evidence: account.map(|_| PayerEvidence::Transcript),
                billing_mode: (account.is_some() && plan_seen).then_some(BillingMode::Subscription),
                repository: match repository.as_deref() {
                    Some(r) => Some(crate::contract::Repository::new(r)?),
                    None => None,
                },
                branch: branch.and_then(Branch::new),
            })
        })();
        match built {
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

    ReadResult {
        thread_id: Some(thread_id),
        session_id: Some(session_id),
        observations,
        new_keys,
        last_ordinal,
        foreign_records: foreign,
        unsupported_lines: unsupported,
        untested_version: cli_version
            .filter(|v| !version::in_range(v, &version::CODEX_TESTED))
            .map(str::to_owned),
        format: Format::Ok,
    }
}
