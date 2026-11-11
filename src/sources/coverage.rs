//! Coverage rules: what we could not read is a gap, never zero. Pure.
use crate::contract::{CoverageReport, CoverageState, GapReason, Iso, Source};

/// More than 5% of strange lines in a chunk makes it an `unsupported_format` gap.
pub const UNSUPPORTED_RATIO: f64 = 0.05;

pub fn too_many_unsupported(unsupported: usize, lines: usize) -> bool {
    unsupported as f64 > lines as f64 * UNSUPPORTED_RATIO
}

/// A one-second gap ending now, for a failure with no better time range.
pub fn failure_now(source: Source, now: Iso, reason: GapReason) -> CoverageReport {
    CoverageReport::unavailable_ending(source, now, reason)
}

/// Adds an `observed` report from the earliest event read up to the first gap of this same
/// read (or now). An `observed` that covered the failure would close the gap on the server.
pub fn with_observed(
    source: Source,
    mut coverage: Vec<CoverageReport>,
    occurred_at: &[Iso],
    now: Iso,
) -> Vec<CoverageReport> {
    let Some(earliest) = occurred_at.iter().min().copied() else {
        return coverage;
    };
    let first_gap = coverage
        .iter()
        .filter(|r| matches!(r.state(), CoverageState::Unavailable { .. }))
        .map(|r| r.from())
        .min();
    let to = first_gap.unwrap_or(now);
    if earliest >= to {
        return coverage;
    }
    if let Some(report) = CoverageReport::new(source, earliest, to, CoverageState::Observed) {
        coverage.push(report);
    }
    coverage
}
