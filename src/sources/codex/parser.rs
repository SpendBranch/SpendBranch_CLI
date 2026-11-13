//! The incremental parser: feeds on rollout lines and turns the token records into observations.
use super::raw::{Line, Payload, Record, Usage, is_id, is_model, text, usage_of};
use super::{Meta, ReadOptions, ReadResult, Resume, UnsupportedCode, UnsupportedLine, empty};
use crate::contract::Iso;
use crate::contract::UsageObservation;
use crate::sources::{
    Format,
    json::{Leaf, Obj},
    key_hash, version,
};
use std::collections::HashMap;

use super::build::{meta_of, observation_of};
use super::turns::TurnModels;

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
            Ok(Obj(None)) => return self.broken(n, UnsupportedCode::NotAnObject),
            Err(_) => return self.broken(n, UnsupportedCode::InvalidJson),
        };
        let payload = entry.payload.0.as_ref();
        let none = Payload::default();
        let p = payload.unwrap_or(&none);
        let ordinal = entry.ordinal.safe_int();
        if let Some(o) = ordinal {
            self.last_ordinal = self.last_ordinal.max(o);
        }
        let kind = entry.kind.str();
        if self.meta.is_none() {
            return self.first_line(kind, p, payload.is_some());
        }
        match kind {
            Some("session_meta") => self.same_thread(p),
            Some("turn_context") => self.turn_context(p),
            Some("event_msg") => self.event_msg(p),
            Some("token_usage_record") => self.usage_record(n, &entry, p, ordinal),
            _ => {}
        }
    }

    /// A line that cannot be understood: it is told apart as a gap, and the next record knows.
    fn broken(&mut self, line: usize, code: UnsupportedCode) {
        self.unsupported.push(UnsupportedLine { line, code });
        self.broken_since_last_record = true;
    }

    /// The first line has to be the session meta; anything else is a format we do not know.
    fn first_line(&mut self, kind: Option<&str>, p: &Payload, has_payload: bool) {
        match (kind == Some("session_meta"))
            .then(|| meta_of(p))
            .flatten()
            .filter(|_| has_payload)
        {
            Some(meta) => self.meta = Some(meta),
            None => self.unknown_format = true,
        }
    }

    /// A second session meta must name the same thread.
    fn same_thread(&mut self, p: &Payload) {
        if self.meta.as_ref().map(|m| m.thread_id.as_str()) != p.id.str() {
            self.unknown_format = true;
        }
    }

    fn turn_context(&mut self, p: &Payload) {
        if let Some(model) = text(&p.model, is_model) {
            self.current_model = Some(model.to_owned());
            if let Some(turn) = text(&p.turn_id, is_id) {
                self.turn_models.insert(turn.to_owned(), model.to_owned());
            }
        }
    }

    fn event_msg(&mut self, p: &Payload) {
        match p.kind.str() {
            Some("thread_settings_applied") => {
                if let Some(model) = p
                    .thread_settings
                    .0
                    .as_ref()
                    .and_then(|t| text(&t.model, is_model))
                {
                    self.current_model = Some(model.to_owned());
                }
            }
            Some("token_count") => {
                self.token_count_seen = true;
                self.plan_seen |= p
                    .rate_limits
                    .0
                    .as_ref()
                    .is_some_and(|r| r.plan_type.is_present());
            }
            _ => {}
        }
    }

    fn usage_record(&mut self, n: usize, entry: &Line, p: &Payload, ordinal: Option<i64>) {
        let this_thread = self.meta.as_ref().map(|m| m.thread_id.as_str());
        if p.thread_id != Leaf::Missing && p.thread_id.str() != this_thread {
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
            return self.broken(n, UnsupportedCode::MissingField);
        };
        if usage[1] + usage[2] > usage[0] {
            return self.broken(n, UnsupportedCode::InconsistentCounters);
        }
        if let Some(known) = self.by_response.get(response_id) {
            if *known != usage {
                self.broken(n, UnsupportedCode::ConflictingResponse);
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
    pub fn finish(mut self) -> (ReadResult, Option<Resume>) {
        if self.unknown_format {
            return (empty(Format::UnsupportedFormat, vec![]), None);
        }
        let Some(meta) = self.meta.clone() else {
            return (empty(Format::Ok, self.unsupported), None);
        };
        if (self.token_count_seen && self.records.is_empty() && !self.had_record)
            || !self.totals_agree()
        {
            return (empty(Format::UnsupportedFormat, self.unsupported), None);
        }
        let mut unsupported = std::mem::take(&mut self.unsupported);
        let (observations, new_keys) = self.observations(&meta, &mut unsupported);
        unsupported.sort_by_key(|u| u.line);

        let resume = Resume {
            meta: meta.clone(),
            current_model: self.current_model,
            turn_models: self.turn_models.into_pairs(),
            plan_seen: self.plan_seen,
            had_record: self.had_record || !self.records.is_empty(),
            last_ordinal: self.last_ordinal,
            prev_thread: self.records.last().map_or(self.prev_thread, |r| r.thread),
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

    /// Between two consecutive records with no broken line in between, the step of the running
    /// total must equal the record's own usage. It is only a local check.
    fn totals_agree(&self) -> bool {
        let mut before = self.prev_thread;
        for now in &self.records {
            if let (Some(a), Some(b)) = (before, now.thread)
                && !now.after_broken_line
                && (0..6).any(|i| i128::from(b[i]) - i128::from(a[i]) != i128::from(now.usage[i]))
            {
                return false;
            }
            before = now.thread;
        }
        true
    }

    /// The new records as observations (and their keys); the ones that cannot be become gaps.
    fn observations(
        &self,
        meta: &Meta,
        unsupported: &mut Vec<UnsupportedLine>,
    ) -> (Vec<UsageObservation>, Vec<String>) {
        let (mut observations, mut new_keys) = (Vec::new(), Vec::new());
        for record in &self.records {
            if record.ordinal <= self.after_ordinal
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
            match observation_of(meta, record, model, self.plan_seen) {
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
        (observations, new_keys)
    }
}
