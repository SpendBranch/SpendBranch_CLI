//! Finding the session files. Symbolic links are never followed.
use crate::contract::AgentRole;
use std::{
    fs,
    path::{Path, PathBuf},
};

fn sorted(mut v: Vec<PathBuf>) -> Vec<PathBuf> {
    v.sort_by_key(|p| p.to_string_lossy().into_owned());
    v
}

/// `<projects>/**/*.jsonl` up to 6 levels deep. A `subagents` folder marks a subagent file.
pub fn claude_transcripts(dir: &Path) -> Vec<(PathBuf, AgentRole)> {
    fn walk(folder: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        if depth > 6 {
            return;
        }
        let Ok(entries) = fs::read_dir(folder) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() {
                walk(&path, depth + 1, out);
            } else if kind.is_file() && path.extension().is_some_and(|e| e == "jsonl") {
                out.push(path);
            }
        }
    }
    let mut found = Vec::new();
    walk(dir, 0, &mut found);
    sorted(found)
        .into_iter()
        .map(|path| {
            let in_subagents = path
                .strip_prefix(dir)
                .is_ok_and(|rel| rel.components().any(|c| c.as_os_str() == "subagents"));
            let role = if in_subagents {
                AgentRole::Subagent
            } else {
                AgentRole::Main
            };
            (path, role)
        })
        .collect()
}

pub struct Rollout {
    pub path: PathBuf,
    /// Compressed with zstd by `codex archive`.
    pub archived: bool,
}

/// `<root>/sessions/**/rollout-*.jsonl` up to 4 levels, and `<root>/archived_sessions/rollout-*.jsonl.zst`.
/// Nothing else under the Codex folder is ever opened.
pub fn codex_rollouts(root: &Path) -> Vec<Rollout> {
    fn walk(folder: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        if depth > 4 {
            return;
        }
        let Ok(entries) = fs::read_dir(folder) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() {
                walk(&path, depth + 1, out);
            } else if kind.is_file() && named(&path, ".jsonl") {
                out.push(path);
            }
        }
    }
    fn named(path: &Path, suffix: &str) -> bool {
        path.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(suffix))
    }
    let mut plain = Vec::new();
    walk(&root.join("sessions"), 0, &mut plain);
    let mut archived = Vec::new();
    if let Ok(entries) = fs::read_dir(root.join("archived_sessions")) {
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|k| k.is_file()) && named(&path, ".jsonl.zst") {
                archived.push(path);
            }
        }
    }
    let mut all: Vec<Rollout> = plain
        .into_iter()
        .map(|path| Rollout {
            path,
            archived: false,
        })
        .chain(archived.into_iter().map(|path| Rollout {
            path,
            archived: true,
        }))
        .collect();
    all.sort_by_key(|r| r.path.to_string_lossy().into_owned());
    all
}
