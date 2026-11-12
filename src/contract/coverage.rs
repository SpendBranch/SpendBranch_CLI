//! What a read could not see: gaps and coverage reports.
use super::*;
use serde::{Deserialize, Serialize};

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
