//! The `status` answer, validated and cut down to what is shown.
use super::pairing::printable;
use crate::contract::Iso;
use serde::Deserialize;

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

/// `None` if the device is malformed.
fn device_view(d: RawDevice) -> Option<DeviceView> {
    if !is_fingerprint(&d.key_fingerprint) || !printable(&d.label, 200) {
        return None;
    }
    Some(DeviceView {
        label: d.label,
        key_fingerprint: d.key_fingerprint,
        created_at: Iso::parse(&d.created_at)?,
        expires_at: Iso::parse(&d.expires_at)?,
    })
}

/// Outer `None`: malformed. Inner `None`: a source this build does not know, left out.
fn source_view(s: RawSource) -> Option<Option<SourceView>> {
    let Some(source) = source_of(&s.source) else {
        return Some(None);
    };
    if s.fields.len() > MAX_FIELDS || !s.fields.iter().all(|f| printable(f, 200)) {
        return None;
    }
    Some(Some(SourceView {
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
    }))
}

/// Outer `None`: malformed. Inner `None`: a source this build does not know, left out.
fn collection_view(c: RawCollection) -> Option<Option<CollectionView>> {
    let Some(source) = source_of(&c.source) else {
        return Some(None);
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
    Some(Some(CollectionView {
        source,
        last_seen_at: optional_time(c.last_seen_at.as_deref())?,
        observed_until: optional_time(c.observed_until.as_deref())?,
        open_gaps,
    }))
}

impl CliStatus {
    /// `None` if any field is malformed. Sources this build does not know are left out.
    pub fn checked(self) -> Option<StatusView> {
        if self.sources.len() > MAX_LISTED || self.collection.len() > MAX_LISTED {
            return None;
        }
        let device = device_view(self.device)?;
        let mut sources = Vec::new();
        for s in self.sources {
            sources.extend(source_view(s)?);
        }
        let mut collection = Vec::new();
        for c in self.collection {
            collection.extend(collection_view(c)?);
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
