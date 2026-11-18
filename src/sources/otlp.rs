//! The lenient filter of Claude Code's OTLP log export (`claude-code-otlp/v1`): what the local
//! relay lets through. Pure. The server filters again, strictly; this one exists so that what
//! leaves the machine is only what the company authorized, even if the agent exports more.
//!
//! Kept: records of an `api_request` event, and of each record only its name, its time and the
//! twelve declared attributes, each in the shape its key promises (counters as whole numbers
//! from 0 to 10^12, identifiers in the contract's patterns). Resource attributes, the record body, every other
//! attribute, free text and any text over 256 characters never pass. Not here: the mapping into
//! observations, which the server does.
//!
//! The body is walked as it is parsed (`lenient_bytes`): only the records that pass are ever held,
//! so a body costs little more than what is kept, never a copy of everything it holds. The same
//! walk reads an already parsed `Value` (`lenient`), which is how the shared corpus checks it.
use serde::de::{self, DeserializeSeed, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};
use std::fmt;

/// Records read per request; the rest are ignored.
pub const MAX_RECORDS: usize = 2000;
/// Longer text is dropped, not cut.
pub const MAX_TEXT: usize = 256;

const EVENTS: [&str; 2] = ["api_request", "claude_code.api_request"];
/// The declared attributes: the `otlp:` entries of `claude_code::FIELDS`.
pub const KEYS: [&str; 12] = [
    "event.name",
    "event.timestamp",
    "session.id",
    "request_id",
    "model",
    "input_tokens",
    "output_tokens",
    "cache_read_tokens",
    "cache_creation_tokens",
    "cache_creation_1h_tokens",
    "query_source",
    "organization.id",
];

pub struct Filtered {
    /// What goes to the server: one resource, one scope, the kept records.
    pub payload: Value,
    pub kept: usize,
    /// Records of another event, or not a record.
    pub dropped: usize,
    /// Records past the 2000th.
    pub ignored: usize,
}

/// The keys whose value is a token count.
const COUNTERS: [&str; 5] = [
    "input_tokens",
    "output_tokens",
    "cache_read_tokens",
    "cache_creation_tokens",
    "cache_creation_1h_tokens",
];

/// A whole number from 0 to the contract's ceiling for a counter; anything else is not a count
/// and never leaves the machine.
fn fits_counter(n: i128) -> bool {
    (0..=i128::from(crate::contract::MAX_TOKENS_PER_COUNTER)).contains(&n)
}

fn is_int_text(s: &str) -> bool {
    let digits = s.strip_prefix('-').unwrap_or(s);
    (1..=19).contains(&digits.len())
        && digits.bytes().all(|b| b.is_ascii_digit())
        && s.parse::<i128>().is_ok_and(fits_counter)
}

/// An attribute's value, as far as the filter looks at it: the last `stringValue` (when it is
/// text) and the last `intValue` (whatever it is). Anything else in it is skipped unread.
#[derive(Default)]
struct Lite {
    string: Option<String>,
    int: Option<Int>,
}

/// An `intValue`: text, a whole number, or something else (which never passes).
enum Int {
    Text(String),
    Whole(serde_json::Number),
    Other,
}

/// The value of `key`, if it has the shape that key promises: a counter is a whole number (as the
/// number or as text), every other key is text matching the contract's pattern for what it names.
/// Free text never passes, whatever writes to the relay.
fn clean_value(key: &str, value: &Lite) -> Option<Value> {
    use crate::contract::{AccountRef, ExternalId, Model};
    let mut out = Map::new();
    if COUNTERS.contains(&key) {
        let (name, kept) = match (&value.int, &value.string) {
            (Some(Int::Text(s)), _) if is_int_text(s) => ("intValue", Value::String(s.clone())),
            (Some(Int::Whole(n)), _) => {
                let whole = n.as_i64().map(i128::from).or(n.as_u64().map(i128::from));
                if !whole.is_some_and(fits_counter) {
                    return None;
                }
                ("intValue", Value::Number(n.clone()))
            }
            (Some(_), _) => return None,
            (None, Some(s)) if is_int_text(s) => ("stringValue", Value::String(s.clone())),
            (None, _) => return None,
        };
        out.insert(name.into(), kept);
    } else {
        let text = value.string.as_deref()?;
        let fine = match key {
            "event.name" => EVENTS.contains(&text),
            "event.timestamp" => {
                (1..=40).contains(&text.len())
                    && text.bytes().all(|b| {
                        b.is_ascii_alphanumeric() || matches!(b, b':' | b'.' | b'+' | b'-')
                    })
            }
            "model" => Model::new(text).is_some(),
            "organization.id" => AccountRef::new(text).is_some(),
            _ => ExternalId::new(text).is_some(), // session.id, request_id, query_source
        };
        if !fine || text.chars().count() > MAX_TEXT {
            return None;
        }
        out.insert("stringValue".into(), Value::String(text.to_owned()));
    }
    Some(Value::Object(out))
}

/// One record, as the filter needs it. Each field is the last one the record gave, as in a
/// parsed object; a new `attributes` starts the attributes over.
#[derive(Default)]
struct Record {
    /// `eventName`, when it is text that is not empty.
    event_name: Option<String>,
    /// The first `event.name` attribute that has a value: its text, if any.
    attribute_name: Option<Option<String>>,
    /// The body's `stringValue`.
    body: Option<String>,
    time: Option<Value>,
    /// The declared attributes that passed, first of each key only.
    kept: Vec<(&'static str, Value)>,
}

impl Record {
    /// The event's name: `eventName`, else the `event.name` attribute, else the body's text.
    fn name(&self) -> Option<&str> {
        self.event_name
            .as_deref()
            .or(self.attribute_name.as_ref().and_then(|n| n.as_deref()))
            .or(self.body.as_deref())
    }

    fn attribute(&mut self, key: &str, value: &Lite) {
        if key == "event.name" && self.attribute_name.is_none() {
            self.attribute_name = Some(value.string.clone());
        }
        let Some(&known) = KEYS.iter().find(|k| **k == key) else {
            return;
        };
        if self.kept.iter().any(|(k, _)| *k == known) {
            return; // the first of a repeated key is the one that counts
        }
        if let Some(v) = clean_value(known, value) {
            self.kept.push((known, v));
        }
    }

    fn rebuilt(self, name: &str) -> Value {
        let mut out = Map::new();
        out.insert("eventName".into(), Value::String(name.to_owned()));
        if let Some(t) = self.time {
            out.insert("timeUnixNano".into(), t);
        }
        let attributes = self
            .kept
            .into_iter()
            .map(|(k, v)| serde_json::json!({ "key": k, "value": v }))
            .collect();
        out.insert("attributes".into(), Value::Array(attributes));
        Value::Object(out)
    }
}

/// What the walk has counted and kept so far.
#[derive(Default)]
struct Walk {
    records: Vec<Value>,
    dropped: usize,
    ignored: usize,
    seen: usize,
}

/// Where in the body a value sits.
#[derive(Clone, Copy)]
enum At {
    /// The body: an object whose `resourceLogs` is read.
    Body,
    /// A list of resources, of scopes, or of records.
    List(Level),
    /// One resource (its `scopeLogs`) or one scope (its `logRecords`).
    Holder(Level),
}

#[derive(Clone, Copy, PartialEq)]
enum Level {
    Resources,
    Scopes,
    Records,
}

impl Level {
    /// The key a holder at this level keeps its list under, and the list's level.
    fn key(self) -> (&'static str, Level) {
        match self {
            Level::Resources => ("scopeLogs", Level::Scopes),
            _ => ("logRecords", Level::Records),
        }
    }
}

/// Accepts every scalar a visitor is not interested in.
macro_rules! skip_scalars {
    () => {
        fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
            Ok(Default::default())
        }
        fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
            Ok(Default::default())
        }
        fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
            Ok(Default::default())
        }
        fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
            Ok(Default::default())
        }
        fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
            Ok(Default::default())
        }
        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(Default::default())
        }
    };
}

fn skip_seq<'de, A: SeqAccess<'de>>(mut seq: A) -> Result<(), A::Error> {
    while seq.next_element::<IgnoredAny>()?.is_some() {}
    Ok(())
}

fn skip_map<'de, A: MapAccess<'de>>(mut map: A) -> Result<(), A::Error> {
    while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
    Ok(())
}

/// The walk over the body, the lists and their holders.
struct Step<'a> {
    at: At,
    walk: &'a mut Walk,
}

impl<'de> DeserializeSeed<'de> for Step<'_> {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Step<'_> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("any JSON value")
    }
    skip_scalars!();

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        let At::List(level) = self.at else {
            return skip_seq(seq);
        };
        if level != Level::Records {
            while seq
                .next_element_seed(Step {
                    at: At::Holder(level),
                    walk: &mut *self.walk,
                })?
                .is_some()
            {}
            return Ok(());
        }
        loop {
            if self.walk.seen >= MAX_RECORDS {
                // Past the ceiling: counted, never read.
                while seq.next_element::<IgnoredAny>()?.is_some() {
                    self.walk.seen += 1;
                    self.walk.ignored += 1;
                }
                return Ok(());
            }
            let Some(record) = seq.next_element_seed(RecordSeed)? else {
                return Ok(());
            };
            self.walk.seen += 1;
            match record.and_then(|r| {
                let name = r.name().filter(|n| EVENTS.contains(n))?.to_owned();
                Some(r.rebuilt(&name))
            }) {
                Some(kept) => self.walk.records.push(kept),
                None => self.walk.dropped += 1,
            }
        }
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let (wanted, list) = match self.at {
            At::Body => ("resourceLogs", Level::Resources),
            At::Holder(level) => level.key(),
            At::List(_) => return skip_map(map),
        };
        let mut found = false;
        while let Some(key) = map.next_key::<String>()? {
            if key != wanted {
                map.next_value::<IgnoredAny>()?;
                continue;
            }
            if found {
                // Two lists under one key: a parsed object would keep the last, a walk cannot
                // take back what it counted. No exporter sends this.
                return Err(de::Error::custom("repeated key"));
            }
            found = true;
            map.next_value_seed(Step {
                at: At::List(list),
                walk: &mut *self.walk,
            })?;
        }
        Ok(())
    }
}

/// One element of `logRecords`: `None` when it is not an object.
struct RecordSeed;

impl<'de> DeserializeSeed<'de> for RecordSeed {
    type Value = Option<Record>;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for RecordSeed {
    type Value = Option<Record>;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a log record")
    }
    skip_scalars!();
    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
        skip_seq(seq).map(|()| None)
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut r = Record::default();
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "eventName" => {
                    r.event_name = map.next_value::<Text>()?.0.filter(|n| !n.is_empty());
                }
                "timeUnixNano" => r.time = map.next_value::<Time>()?.0,
                "body" => {
                    r.body = map
                        .next_value::<Option<LiteSeed>>()?
                        .and_then(|l| l.0.string)
                }
                "attributes" => {
                    (r.attribute_name, r.kept) = (None, Vec::new());
                    map.next_value_seed(Attributes(&mut r))?;
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(Some(r))
    }
}

/// Text, or `None` for any other value (skipped unread).
#[derive(Default)]
struct Text(Option<String>);

impl<'de> de::Deserialize<'de> for Text {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Text;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("text")
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Text, E> {
                Ok(Text(None))
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<Text, E> {
                Ok(Text(None))
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<Text, E> {
                Ok(Text(None))
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Text, E> {
                Ok(Text(None))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Text, E> {
                Ok(Text(None))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Text, E> {
                Ok(Text(Some(v.to_owned())))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Text, A::Error> {
                skip_seq(seq).map(|()| Text(None))
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Text, A::Error> {
                skip_map(map).map(|()| Text(None))
            }
        }
        d.deserialize_any(V)
    }
}

/// `timeUnixNano` when it is up to 20 digits as text, or a whole number that is not negative.
struct Time(Option<Value>);

impl<'de> de::Deserialize<'de> for Time {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Time(match Int::deserialize(d)? {
            Int::Text(s)
                if (1..=20).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit()) =>
            {
                Some(Value::String(s))
            }
            Int::Whole(n) if n.is_u64() => Some(Value::Number(n)),
            _ => None,
        }))
    }
}

impl<'de> de::Deserialize<'de> for Int {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Int;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a whole number")
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Int, E> {
                Ok(Int::Other)
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Int, E> {
                Ok(Int::Whole(v.into()))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Int, E> {
                Ok(Int::Whole(v.into()))
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Int, E> {
                Ok(Int::Other)
            }
            fn visit_unit<E: de::Error>(self) -> Result<Int, E> {
                Ok(Int::Other)
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Int, E> {
                Ok(Int::Text(v.to_owned()))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Int, A::Error> {
                skip_seq(seq).map(|()| Int::Other)
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Int, A::Error> {
                skip_map(map).map(|()| Int::Other)
            }
        }
        d.deserialize_any(V)
    }
}

/// An attribute's value object (`Lite`); any other value reads as an empty one.
struct LiteSeed(Lite);

impl<'de> de::Deserialize<'de> for LiteSeed {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = LiteSeed;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("an attribute value")
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<LiteSeed, E> {
                Ok(LiteSeed(Lite::default()))
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<LiteSeed, E> {
                Ok(LiteSeed(Lite::default()))
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<LiteSeed, E> {
                Ok(LiteSeed(Lite::default()))
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<LiteSeed, E> {
                Ok(LiteSeed(Lite::default()))
            }
            fn visit_unit<E: de::Error>(self) -> Result<LiteSeed, E> {
                Ok(LiteSeed(Lite::default()))
            }
            fn visit_str<E: de::Error>(self, _: &str) -> Result<LiteSeed, E> {
                Ok(LiteSeed(Lite::default()))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<LiteSeed, A::Error> {
                skip_seq(seq).map(|()| LiteSeed(Lite::default()))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<LiteSeed, A::Error> {
                let mut lite = Lite::default();
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "stringValue" => lite.string = map.next_value::<Text>()?.0,
                        "intValue" => lite.int = Some(map.next_value::<Int>()?),
                        _ => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(LiteSeed(lite))
            }
        }
        d.deserialize_any(V)
    }
}

/// A record's `attributes`: each `{ key, value }` with both present goes to the record, in order.
struct Attributes<'a>(&'a mut Record);

impl<'de> DeserializeSeed<'de> for Attributes<'_> {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Attributes<'_> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a list of attributes")
    }
    skip_scalars!();
    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<(), A::Error> {
        skip_map(map)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while let Some(attribute) = seq.next_element::<Option<Attribute>>()? {
            if let Some(Attribute {
                key: Some(key),
                value: Some(value),
            }) = attribute
            {
                self.0.attribute(&key, &value);
            }
        }
        Ok(())
    }
}

/// One attribute: its last `key` when it is text, and its last `value` of any kind.
#[derive(Default)]
struct Attribute {
    key: Option<String>,
    value: Option<Lite>,
}

impl<'de> de::Deserialize<'de> for Attribute {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Attribute;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("an attribute")
            }
            skip_scalars!();
            fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Attribute, A::Error> {
                skip_seq(seq).map(|()| Attribute::default())
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Attribute, A::Error> {
                let mut a = Attribute::default();
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "key" => a.key = map.next_value::<Text>()?.0,
                        "value" => a.value = Some(map.next_value::<LiteSeed>()?.0),
                        _ => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(a)
            }
        }
        d.deserialize_any(V)
    }
}

fn finish(walk: Walk) -> Filtered {
    let kept = walk.records.len();
    let payload = serde_json::json!({
        "resourceLogs": [{ "scopeLogs": [{ "logRecords": walk.records }] }]
    });
    Filtered {
        payload,
        kept,
        dropped: walk.dropped,
        ignored: walk.ignored,
    }
}

/// Whatever `body` is, the answer is a well-formed payload (an empty one for garbage).
pub fn lenient(body: &Value) -> Filtered {
    let mut walk = Walk::default();
    // A parsed object has no repeated key, so the walk cannot fail here.
    let _ = Step {
        at: At::Body,
        walk: &mut walk,
    }
    .deserialize(body);
    finish(walk)
}

/// `lenient` straight from the bytes, holding only what passes. `None` when the body is not
/// JSON, or repeats `resourceLogs`, `scopeLogs` or `logRecords` in one object.
pub fn lenient_bytes(body: &[u8]) -> Option<Filtered> {
    let mut walk = Walk::default();
    let mut d = serde_json::Deserializer::from_slice(body);
    Step {
        at: At::Body,
        walk: &mut walk,
    }
    .deserialize(&mut d)
    .ok()?;
    d.end().ok()?;
    Some(finish(walk))
}
