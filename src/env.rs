//! The process environment as a parameter, so nothing reads global state and tests never
//! touch the real environment.

/// Reads one environment variable.
pub type Env<'a> = &'a dyn Fn(&str) -> Option<String>;

/// `$HOME` (`%USERPROFILE%` on Windows); `None` when unset or empty.
pub fn user_home(env: Env) -> Option<std::path::PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    env(var).filter(|v| !v.is_empty()).map(Into::into)
}

/// An agent's own configuration folder: `$CLAUDE_CONFIG_DIR` or `~/.claude`, `$CODEX_HOME` or
/// `~/.codex`. The one place that decides it, for the readers and for the hook files alike.
pub fn agent_dir(env: Env, source: crate::contract::Source) -> Option<std::path::PathBuf> {
    use crate::contract::Source;
    let (var, dot) = match source {
        Source::ClaudeCode => ("CLAUDE_CONFIG_DIR", ".claude"),
        Source::Codex => ("CODEX_HOME", ".codex"),
    };
    match env(var).filter(|d| !d.is_empty()) {
        Some(dir) => Some(dir.into()),
        None => user_home(env).map(|h| h.join(dot)),
    }
}
