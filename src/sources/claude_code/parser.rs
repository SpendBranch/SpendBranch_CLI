//! The incremental parser: feeds on transcript lines and merges the streamed copies of a request.
use super::build::{counters_of, observation_of};
use super::raw::*;
use super::*;
use std::collections::HashMap;

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
            return self.bridge(&line);
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
        if line.kind.str() == Some("assistant") {
            self.assistant(n, &line, session_id, line_version);
        }
    }

    /// The line that says which organization pays for the session.
    fn bridge(&mut self, line: &Line) {
        self.recognized += 1;
        if let Some(owner) = line
            .owner_organization_uuid
            .str()
            .filter(|o| AccountRef::new(o).is_some())
        {
            self.payer = Some(owner.to_owned());
        }
    }

    /// An answer of the assistant: the counters of one request (streamed copies merge later).
    fn assistant(&mut self, n: usize, line: &Line, session_id: &str, line_version: Option<&str>) {
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
        let role = if self.options.subagent_file || line.is_sidechain == Leaf::Bool(true) {
            AgentRole::Subagent
        } else {
            AgentRole::Main
        };
        let pending = Pending {
            session_id: session_id.to_owned(),
            model: model.to_owned(),
            at: at.millis(),
            role,
            counters,
            version: line_version.map(str::to_owned),
            branch: line
                .git_branch
                .str()
                .filter(|b| Branch::new(b).is_some())
                .map(str::to_owned),
            broken: false,
        };
        self.merge(n, key, pending);
    }

    /// The first copy of a request is kept; later ones raise its counters to the larger value
    /// (a streamed answer grows), unless they disagree about the model or the session.
    fn merge(&mut self, n: usize, key: String, new: Pending) {
        match self.messages.get_mut(&key) {
            None => {
                self.order.push(key.clone());
                self.messages.insert(key, new);
            }
            Some(known) if known.model != new.model || known.session_id != new.session_id => {
                known.broken = true;
                self.unsupported.push(n);
            }
            Some(known) => {
                known.at = known.at.min(new.at);
                known.branch = known.branch.take().or(new.branch);
                known.version = known.version.take().or(new.version);
                for (slot, add) in known.counters.iter_mut().zip(new.counters) {
                    *slot = max(*slot, add);
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
