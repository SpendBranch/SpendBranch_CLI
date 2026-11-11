//! `claude-code/v2`: Claude Code transcript text to usage observations. Pure and
//! deterministic: the same text and options always give the same result.
use super::{
    Format,
    json::{Counter, Leaf, Obj, counter},
    key_hash, version,
};
use crate::contract::{
    AccountRef, AgentRole, Branch, Count, CounterOrigin, ExternalId, Iso, Model, ParserVersion,
    PayerEvidence, Source, SourceVersion, UsageCounters, UsageObservation,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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

// Only allowlisted fields are named here. Serde skips every other field, the conversation
// text included, without storing it. A key repeated in one line makes the line unsupported
// (the reference keeps the last one).
#[derive(Deserialize, Default)]
#[serde(default)]
struct Line {
    #[serde(rename = "type")]
    kind: Leaf,
    #[serde(rename = "sessionId")]
    session_id: Leaf,
    timestamp: Leaf,
    version: Leaf,
    #[serde(rename = "isSidechain")]
    is_sidechain: Leaf,
    #[serde(rename = "gitBranch")]
    git_branch: Leaf,
    #[serde(rename = "requestId")]
    request_id: Leaf,
    #[serde(rename = "ownerOrganizationUuid")]
    owner_organization_uuid: Leaf,
    message: Obj<Message>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Message {
    id: Leaf,
    model: Leaf,
    usage: Obj<Usage>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Usage {
    input_tokens: Leaf,
    output_tokens: Leaf,
    cache_read_input_tokens: Leaf,
    cache_creation_input_tokens: Leaf,
    output_tokens_details: Obj<Details>,
    cache_creation: Obj<CacheCreation>,
    server_tool_use: Obj<ServerToolUse>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Details {
    thinking_tokens: Leaf,
}
#[derive(Deserialize, Default)]
#[serde(default)]
struct CacheCreation {
    ephemeral_1h_input_tokens: Leaf,
}
#[derive(Deserialize, Default)]
#[serde(default)]
struct ServerToolUse {
    web_search_requests: Leaf,
}

/// input, output, cache read, cache write, reasoning, cache write 1h, web searches.
type Counters = [Option<u64>; 7];

fn counters_of(u: &Usage) -> Option<Counters> {
    let details = u
        .output_tokens_details
        .0
        .as_ref()
        .map(|d| &d.thinking_tokens);
    let cache = u
        .cache_creation
        .0
        .as_ref()
        .map(|c| &c.ephemeral_1h_input_tokens);
    let tools = u.server_tool_use.0.as_ref().map(|t| &t.web_search_requests);
    let missing = Leaf::Missing;
    let leaves = [
        &u.input_tokens,
        &u.output_tokens,
        &u.cache_read_input_tokens,
        &u.cache_creation_input_tokens,
        details.unwrap_or(&missing),
        cache.unwrap_or(&missing),
        tools.unwrap_or(&missing),
    ];
    let mut out = [None; 7];
    for (slot, l) in out.iter_mut().zip(leaves) {
        *slot = match counter(l) {
            Counter::Unknown => None,
            Counter::Value(n) => Some(n),
            Counter::Bad => return None,
        };
    }
    Some(out)
}

fn is_id(s: &str) -> bool {
    ExternalId::new(s).is_some()
}

struct Pending {
    session_id: String,
    model: String,
    at: i64,
    role: AgentRole,
    counters: Counters,
    version: Option<String>,
    branch: Option<String>,
    broken: bool,
}

fn max(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

/// Reads a whole text at once. Same result as feeding its lines to a `Parser`.
pub fn read(text: &str, options: &ReadOptions) -> ReadResult {
    let mut parser = Parser::new(options);
    for raw in text.split('\n') {
        parser.push_line(raw);
    }
    parser.finish()
}

/// Incremental form of `read`, for readers that stream a file line by line.
/// Memory grows with the number of distinct requests, never with the text.
pub struct Parser<'a> {
    options: &'a ReadOptions<'a>,
    order: Vec<String>,
    messages: HashMap<String, Pending>,
    unsupported: Vec<usize>,
    line: usize,
    recognized: usize,
    payer: Option<String>,
    untested: Option<String>,
}

impl<'a> Parser<'a> {
    pub fn new(options: &'a ReadOptions<'a>) -> Self {
        Self {
            options,
            order: Vec::new(),
            messages: HashMap::new(),
            unsupported: Vec::new(),
            line: 0,
            recognized: 0,
            payer: None,
            untested: None,
        }
    }

    /// A line the reader refused to load (too long to be a usage record). It counts as
    /// an unsupported line, so a file full of them becomes a gap.
    pub fn push_unsupported_line(&mut self) {
        self.line += 1;
        self.unsupported.push(self.line);
    }

    /// One line, without its line break.
    pub fn push_line(&mut self, raw: &str) {
        self.line += 1;
        let n = self.line;
        if raw.trim().is_empty() {
            return;
        }
        let Ok(Obj(Some(line))) = serde_json::from_str::<Obj<Line>>(raw) else {
            self.unsupported.push(n);
            return;
        };
        if line.kind.str() == Some("bridge-session") {
            self.recognized += 1;
            if let Some(owner) = line
                .owner_organization_uuid
                .str()
                .filter(|o| AccountRef::new(o).is_some())
            {
                self.payer = Some(owner.to_owned());
            }
            return;
        }
        let Some(session_id) = line.session_id.str().filter(|s| is_id(s)) else {
            return;
        };
        self.recognized += 1;
        let line_version = line
            .version
            .str()
            .filter(|v| SourceVersion::new(v).is_some());
        if let Some(v) =
            line_version.filter(|v| !version::in_range(v, &version::CLAUDE_CODE_TESTED))
        {
            self.untested = Some(v.to_owned());
        }
        if line.kind.str() != Some("assistant") {
            return;
        }
        let (Some(message), Some(usage)) = (
            line.message.0.as_ref(),
            line.message.0.as_ref().and_then(|m| m.usage.0.as_ref()),
        ) else {
            return;
        };
        if message.model.str() == Some("<synthetic>") {
            return;
        }
        let Some(model) = message.model.str().filter(|m| Model::new(m).is_some()) else {
            self.unsupported.push(n);
            return;
        };
        let key = match (line.request_id.str().filter(|r| is_id(r)), message.id.str()) {
            (Some(request), _) => Some(request.to_owned()),
            (None, Some(id)) if is_id(&format!("msg:{id}")) => Some(format!("msg:{id}")),
            _ => None,
        };
        let counters = counters_of(usage);
        let at = line.timestamp.str().and_then(Iso::parse);
        let (Some(key), Some(counters), Some(at)) = (key, counters, at) else {
            self.unsupported.push(n);
            return;
        };
        let at = at.millis();
        let branch = line
            .git_branch
            .str()
            .filter(|b| Branch::new(b).is_some())
            .map(str::to_owned);
        let role = if self.options.subagent_file || line.is_sidechain == Leaf::Bool(true) {
            AgentRole::Subagent
        } else {
            AgentRole::Main
        };
        let version = line_version.map(str::to_owned);
        match self.messages.get_mut(&key) {
            None => {
                self.order.push(key.clone());
                self.messages.insert(
                    key,
                    Pending {
                        session_id: session_id.to_owned(),
                        model: model.to_owned(),
                        at,
                        role,
                        counters,
                        version,
                        branch,
                        broken: false,
                    },
                );
            }
            Some(known) if known.model != model || known.session_id != session_id => {
                known.broken = true;
                self.unsupported.push(n);
            }
            Some(known) => {
                known.at = known.at.min(at);
                known.branch = known.branch.take().or(branch);
                known.version = known.version.take().or(version);
                for (slot, new) in known.counters.iter_mut().zip(counters) {
                    *slot = max(*slot, new);
                }
            }
        }
    }

    pub fn finish(self) -> ReadResult {
        let mut observations = Vec::new();
        let mut new_keys = Vec::new();
        for key in self.order {
            let m = &self.messages[&key];
            if m.broken || self.options.seen.is_some_and(|seen| seen(key_hash(&key))) {
                continue;
            }
            if m.counters[..4].iter().all(|c| c.is_none_or(|c| c == 0)) {
                continue; // nothing priced in this message
            }
            if let Some(observation) = observation_of(&key, m, self.payer.as_deref()) {
                observations.push(observation);
                new_keys.push(key);
            }
        }
        let format = if self.recognized == 0 && !self.unsupported.is_empty() {
            Format::UnsupportedFormat
        } else {
            Format::Ok
        };
        ReadResult {
            observations,
            new_keys,
            unsupported_lines: self.unsupported,
            untested_version: self.untested,
            format,
        }
    }
}

fn observation_of(key: &str, m: &Pending, payer: Option<&str>) -> Option<UsageObservation> {
    let count = |c: Option<u64>| c.map_or(Some(Count::UNKNOWN), Count::new);
    let [
        input,
        output,
        cache_read,
        cache_write,
        reasoning,
        cache_write_1h,
        web,
    ] = m.counters;
    Some(UsageObservation {
        source: Source::ClaudeCode,
        external_session_id: ExternalId::new(&m.session_id)?,
        external_event_id: ExternalId::new(key)?,
        occurred_at: Iso::from_millis(m.at)?,
        model: Model::new(&m.model)?,
        agent_role: m.role,
        counters: UsageCounters {
            input_tokens: count(input)?,
            output_tokens: count(output)?,
            cache_read_tokens: count(cache_read)?,
            cache_write_tokens: count(cache_write)?,
            reasoning_tokens: count(reasoning)?,
            cache_write_1h_tokens: Some(count(cache_write_1h)?),
            web_search_requests: Some(count(web)?),
        },
        origin: CounterOrigin::ReportedDelta,
        parser_version: ParserVersion::new(PARSER)?,
        source_version: m.version.as_deref().and_then(SourceVersion::new),
        payer_account_ref: payer.and_then(AccountRef::new),
        payer_evidence: payer.map(|_| PayerEvidence::Transcript),
        billing_mode: None,
        repository: None,
        branch: m.branch.as_deref().and_then(Branch::new),
    })
}
