//! The tools whose sessions we can read. Pure: no I/O. Parsers join in the next phase.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    ClaudeCode,
    Codex,
}

impl Source {
    /// Accepts the CLI spellings: `claude-code`, `claude_code` and `codex`.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "claude-code" | "claude_code" => Some(Self::ClaudeCode),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }
}
