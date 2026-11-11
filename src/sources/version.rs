//! Tested version ranges. Outside the range the CLI only warns and reads normally.

pub struct Range {
    pub from: &'static str,
    pub to: &'static str,
}

pub const CLAUDE_CODE_TESTED: Range = Range {
    from: "2.1.290",
    to: "2.1.320",
};
pub const CODEX_TESTED: Range = Range {
    from: "0.155.0",
    to: "0.165.999",
};

/// Compares the first three numeric parts (split on `.`, `+`, `-`); a non-number counts as 0.
fn parts(version: &str) -> [u64; 3] {
    let mut out = [0; 3];
    for (slot, part) in out.iter_mut().zip(version.split(['.', '+', '-'])) {
        *slot = part.parse().unwrap_or(0);
    }
    out
}

pub fn in_range(version: &str, range: &Range) -> bool {
    let v = parts(version);
    parts(range.from) <= v && v <= parts(range.to)
}
