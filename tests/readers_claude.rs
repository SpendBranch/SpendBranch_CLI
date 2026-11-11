//! Claude Code reader, against temporary folders only (never the real ~/.claude).
use serde_json::json;
use spendbranch::contract::{
    AgentRole, BillingMode, CoverageState, GapReason, Iso, PayerEvidence, UsageObservation,
};
use spendbranch::infra::store::{Commit, SpoolLimits, Store};
use spendbranch::readers::claude_code::{Config, read};
use spendbranch::sources::{Source, outcome::ReadOutcome};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, UNIX_EPOCH},
};

fn now() -> Iso {
    Iso::parse("2026-06-01T12:00:00Z").unwrap()
}

struct Rig {
    root: tempfile::TempDir,
    store: Store,
    _data: tempfile::TempDir,
}

impl Rig {
    fn new() -> Self {
        let data = tempfile::tempdir().unwrap();
        let store = Store::open(data.path()).unwrap();
        Rig {
            root: tempfile::tempdir().unwrap(),
            store,
            _data: data,
        }
    }
    fn config(&self) -> Config {
        Config {
            projects_dir: self.root.path().join("projects"),
            account_file: self.root.path().join(".claude.json"),
            home: Some(self.root.path().join("home")),
            max_line: 1 << 20,
        }
    }
    fn path(&self, rel: &str) -> PathBuf {
        self.root.path().join("projects").join(rel)
    }
    /// Reads and commits, like one `sync` collection step.
    fn collect(&mut self) -> ReadOutcome {
        self.collect_at(now())
    }
    fn collect_at(&mut self, at: Iso) -> ReadOutcome {
        let outcome = read(
            &self.config(),
            &self.store.reader_state(Source::ClaudeCode).unwrap(),
            at,
        );
        assert_eq!(
            self.store
                .commit_read(&outcome, SpoolLimits::default())
                .unwrap(),
            Commit::Stored
        );
        outcome
    }
}

fn line(session: &str, request: &str, ts: &str, output: u64) -> String {
    line_in(
        session,
        request,
        ts,
        output,
        "/nonexistent/synthetic-project",
    )
}

fn line_in(session: &str, request: &str, ts: &str, output: u64, cwd: &str) -> String {
    json!({
        "type": "assistant", "sessionId": session, "requestId": request, "timestamp": ts,
        "version": "2.1.300", "cwd": cwd,
        "message": {"id": format!("m_{request}"), "model": "claude-test-1",
            "usage": {"input_tokens": 10, "output_tokens": output,
                      "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0}}
    })
    .to_string()
}

fn set_age(path: &Path, seconds_before_now: u64) {
    let ms = now().millis() as u64 - seconds_before_now * 1000;
    OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(UNIX_EPOCH + Duration::from_millis(ms))
        .unwrap();
}

fn write(path: &Path, lines: &[String]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        lines.iter().map(|l| format!("{l}\n")).collect::<String>(),
    )
    .unwrap();
    set_age(path, 3600);
}

fn append(path: &Path, lines: &[String]) {
    let mut f = OpenOptions::new().append(true).open(path).unwrap();
    for l in lines {
        writeln!(f, "{l}").unwrap();
    }
    drop(f);
    set_age(path, 3600);
}

fn ids(o: &ReadOutcome) -> Vec<String> {
    o.observations
        .iter()
        .map(|x| x.external_event_id.as_str().to_owned())
        .collect()
}

fn gaps(o: &ReadOutcome) -> Vec<GapReason> {
    o.coverage
        .iter()
        .filter_map(|c| match c.state() {
            CoverageState::Unavailable { reason } => Some(reason),
            CoverageState::Observed => None,
        })
        .collect()
}

#[test]
fn a_first_read_buffers_everything_and_a_second_read_finds_nothing_new() {
    let mut rig = Rig::new();
    write(
        &rig.path("p/s1.jsonl"),
        &[
            line("s1", "r1", "2026-06-01T10:00:00Z", 5),
            line("s1", "r2", "2026-06-01T10:00:01Z", 6),
        ],
    );
    let first = rig.collect();
    assert_eq!(ids(&first), ["r1", "r2"]);
    assert_eq!(rig.store.pending_counts().unwrap().0, 2);
    let second = rig.collect();
    assert!(
        second.observations.is_empty() && second.cursor_set.is_empty() && gaps(&second).is_empty()
    );
    assert_eq!(
        rig.store.pending_counts().unwrap().0,
        2,
        "nothing was buffered twice"
    );
}

#[test]
fn a_growing_file_is_read_only_from_where_it_stopped() {
    let mut rig = Rig::new();
    let file = rig.path("p/s1.jsonl");
    write(&file, &[line("s1", "r1", "2026-06-01T10:00:00Z", 5)]);
    rig.collect();
    append(&file, &[line("s1", "r2", "2026-06-01T10:00:05Z", 7)]);
    assert_eq!(ids(&rig.collect()), ["r2"]);
    assert_eq!(rig.store.pending_counts().unwrap().0, 2);
}

#[test]
fn the_last_message_of_a_file_still_being_written_is_held_until_it_settles() {
    let mut rig = Rig::new();
    let file = rig.path("p/s1.jsonl");
    // The same request streams in with growing counters; the file was touched 5 s ago.
    write(
        &file,
        &[
            line("s1", "r1", "2026-06-01T10:00:00Z", 5),
            line("s1", "r2", "2026-06-01T10:00:01Z", 1),
            line("s1", "r2", "2026-06-01T10:00:01Z", 9),
        ],
    );
    set_age(&file, 5);
    assert_eq!(ids(&rig.collect()), ["r1"], "r2 might still grow");
    set_age(&file, 3600); // it settled; nothing was appended
    let later = rig.collect();
    assert_eq!(ids(&later), ["r2"]);
    assert_eq!(
        later.observations[0].counters.output_tokens.get(),
        Some(9),
        "the final counter, not a frozen smaller one"
    );
}

#[test]
fn a_line_without_its_line_break_is_left_for_later() {
    let mut rig = Rig::new();
    let file = rig.path("p/s1.jsonl");
    write(&file, &[line("s1", "r1", "2026-06-01T10:00:00Z", 5)]);
    let half = line("s1", "r2", "2026-06-01T10:00:01Z", 6);
    OpenOptions::new()
        .append(true)
        .open(&file)
        .unwrap()
        .write_all(&half.as_bytes()[..half.len() / 2])
        .unwrap();
    set_age(&file, 3600);
    assert_eq!(ids(&rig.collect()), ["r1"]);
    OpenOptions::new()
        .append(true)
        .open(&file)
        .unwrap()
        .write_all(format!("{}\n", &half[half.len() / 2..]).as_bytes())
        .unwrap();
    set_age(&file, 3600);
    assert_eq!(ids(&rig.collect()), ["r2"]);
}

#[test]
fn a_replaced_file_is_read_again_but_already_sent_events_are_not() {
    let mut rig = Rig::new();
    let file = rig.path("p/s1.jsonl");
    write(&file, &[line("s1", "r1", "2026-06-01T10:00:00Z", 5)]);
    rig.collect();
    fs::remove_file(&file).unwrap();
    write(
        &file,
        &[
            line("s1", "r1", "2026-06-01T10:00:00Z", 5),
            line("s1", "r3", "2026-06-01T10:00:09Z", 8),
        ],
    );
    assert_eq!(ids(&rig.collect()), ["r3"]);
}

#[test]
fn a_file_rewritten_in_place_and_grown_does_not_resume_in_the_middle() {
    let mut rig = Rig::new();
    let file = rig.path("p/s1.jsonl");
    write(&file, &[line("s1", "r1", "2026-06-01T10:00:00Z", 5)]);
    rig.collect();
    // Same inode, bigger, different content: the saved offset points into the new text.
    let mut f = File::create(&file).unwrap();
    for l in [
        line("s1", "x1", "2026-06-01T10:00:00Z", 5),
        line("s1", "x2", "2026-06-01T10:00:01Z", 6),
        line("s1", "x3", "2026-06-01T10:00:02Z", 7),
    ] {
        writeln!(f, "{l}").unwrap();
    }
    drop(f);
    set_age(&file, 3600);
    assert_eq!(ids(&rig.collect()), ["x1", "x2", "x3"]);
}

#[test]
fn a_shrunk_file_starts_over() {
    let mut rig = Rig::new();
    let file = rig.path("p/s1.jsonl");
    write(
        &file,
        &[
            line("s1", "r1", "2026-06-01T10:00:00Z", 5),
            line("s1", "r2", "2026-06-01T10:00:01Z", 5),
        ],
    );
    rig.collect();
    let f = OpenOptions::new().write(true).open(&file).unwrap();
    f.set_len(0).unwrap();
    drop(f);
    append(&file, &[line("s1", "r9", "2026-06-01T10:00:09Z", 5)]);
    assert_eq!(ids(&rig.collect()), ["r9"]);
}

#[test]
fn a_deleted_file_leaves_no_cursor_behind() {
    let mut rig = Rig::new();
    let file = rig.path("p/s1.jsonl");
    write(&file, &[line("s1", "r1", "2026-06-01T10:00:00Z", 5)]);
    rig.collect();
    assert_eq!(
        rig.store
            .reader_state(Source::ClaudeCode)
            .unwrap()
            .cursors
            .len(),
        1
    );
    fs::remove_file(&file).unwrap();
    let out = rig.collect();
    assert_eq!(out.cursor_remove.len(), 1);
    assert!(
        rig.store
            .reader_state(Source::ClaudeCode)
            .unwrap()
            .cursors
            .is_empty()
    );
}

#[cfg(unix)]
#[test]
fn an_unreadable_file_is_one_gap_not_a_gap_every_time_and_is_read_once_fixed() {
    use std::os::unix::fs::PermissionsExt;
    let mut rig = Rig::new();
    let file = rig.path("p/s1.jsonl");
    write(&file, &[line("s1", "r1", "2026-06-01T10:00:00Z", 5)]);
    fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();
    if File::open(&file).is_ok() {
        return; // running as root: permissions do not apply
    }
    assert_eq!(gaps(&rig.collect()), [GapReason::AdapterError]);
    assert!(
        gaps(&rig.collect()).is_empty(),
        "no repeated gap while it stays unreadable"
    );
    assert_eq!(
        rig.store
            .reader_state(Source::ClaudeCode)
            .unwrap()
            .unsupported_files
            .len(),
        1
    );
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        ids(&rig.collect()),
        ["r1"],
        "read from the start once it opens again"
    );
    assert!(
        rig.store
            .reader_state(Source::ClaudeCode)
            .unwrap()
            .unsupported_files
            .is_empty()
    );
}

#[test]
fn a_forked_session_does_not_send_the_copied_history_twice() {
    let mut rig = Rig::new();
    let shared = [
        line("s1", "r1", "2026-06-01T10:00:00Z", 5),
        line("s1", "r2", "2026-06-01T10:00:01Z", 6),
    ];
    write(&rig.path("p/a.jsonl"), &shared);
    let mut forked = shared.to_vec();
    forked.push(line("s1", "r3", "2026-06-01T10:00:02Z", 7));
    write(&rig.path("p/b.jsonl"), &forked);
    let out = rig.collect();
    assert_eq!(
        ids(&out),
        ["r1", "r2", "r3"],
        "each event once even though two files hold r1 and r2"
    );
}

#[test]
fn files_under_subagents_are_subagent_work() {
    let mut rig = Rig::new();
    write(
        &rig.path("p/s1.jsonl"),
        &[line("s1", "r1", "2026-06-01T10:00:00Z", 5)],
    );
    write(
        &rig.path("p/s1/subagents/agent-1.jsonl"),
        &[line("s1", "r2", "2026-06-01T10:00:01Z", 5)],
    );
    let out = rig.collect();
    let role = |id: &str| {
        out.observations
            .iter()
            .find(|o| o.external_event_id.as_str() == id)
            .unwrap()
            .agent_role
    };
    assert_eq!(
        (role("r1"), role("r2")),
        (AgentRole::Main, AgentRole::Subagent)
    );
}

#[test]
fn discovery_stops_six_levels_down() {
    let mut rig = Rig::new();
    write(
        &rig.path("a/b/c/d/e/f/ok.jsonl"),
        &[line("s1", "r1", "2026-06-01T10:00:00Z", 5)],
    ); // depth 6
    write(
        &rig.path("a/b/c/d/e/f/g/too-deep.jsonl"),
        &[line("s1", "r2", "2026-06-01T10:00:01Z", 5)],
    );
    write(&rig.path("p/notes.txt.bak"), &["not a transcript".into()]);
    assert_eq!(ids(&rig.collect()), ["r1"]);
}

#[cfg(unix)]
#[test]
fn symbolic_links_are_never_followed() {
    let mut rig = Rig::new();
    let outside = rig.root.path().join("outside");
    write(
        &outside.join("secret.jsonl"),
        &[line("s9", "leak", "2026-06-01T10:00:00Z", 5)],
    );
    fs::create_dir_all(rig.path("p")).unwrap();
    std::os::unix::fs::symlink(outside.join("secret.jsonl"), rig.path("p/link.jsonl")).unwrap();
    std::os::unix::fs::symlink(&outside, rig.path("p/dir")).unwrap();
    assert!(rig.collect().observations.is_empty());
}

#[test]
fn a_file_in_an_unknown_format_is_one_gap_then_stays_quiet() {
    let mut rig = Rig::new();
    let file = rig.path("p/s1.jsonl");
    write(
        &file,
        &[
            "garbage".into(),
            "{\"nothing\":\"known\"}".into(),
            "42".into(),
        ],
    );
    let first = rig.collect();
    assert_eq!(gaps(&first), [GapReason::UnsupportedFormat]);
    assert!(first.observations.is_empty());
    let second = rig.collect();
    assert!(
        gaps(&second).is_empty(),
        "same unchanged file: no repeated gap"
    );
    assert_eq!(
        second.unsupported_files.len(),
        1,
        "but it is still known to be unreadable"
    );
}

#[test]
fn many_strange_lines_make_a_gap_but_the_good_ones_are_still_sent() {
    let mut rig = Rig::new();
    let mut lines: Vec<String> = (0..10)
        .map(|i| line("s1", &format!("r{i}"), "2026-06-01T10:00:00Z", 5))
        .collect();
    lines.extend(std::iter::repeat_n("junk".to_string(), 2)); // 2 of 12 lines: above 5%
    write(&rig.path("p/s1.jsonl"), &lines);
    let out = rig.collect();
    assert_eq!(out.observations.len(), 10);
    assert_eq!(gaps(&out), [GapReason::UnsupportedFormat]);
}

#[test]
fn a_huge_line_is_skipped_without_being_loaded() {
    let rig = Rig::new();
    let mut config = rig.config();
    config.max_line = 4096;
    let huge = json!({"type": "user", "sessionId": "s1", "message": {"content": "SYNTHETIC PROMPT ".repeat(10_000)}}).to_string();
    write(
        &rig.path("p/s1.jsonl"),
        &[
            line("s1", "r1", "2026-06-01T10:00:00Z", 5),
            huge,
            line("s1", "r2", "2026-06-01T10:00:01Z", 5),
        ],
    );
    let out = read(
        &config,
        &rig.store.reader_state(Source::ClaudeCode).unwrap(),
        now(),
    );
    assert_eq!(ids(&out), ["r1", "r2"]);
    assert!(
        !serde_json::to_string(&out.observations)
            .unwrap()
            .contains("SYNTHETIC PROMPT")
    );
}

#[test]
fn coverage_reports_what_was_observed_and_stops_at_a_gap() {
    let mut rig = Rig::new();
    write(
        &rig.path("p/s1.jsonl"),
        &[
            line("s1", "r1", "2026-06-01T10:00:00Z", 5),
            line("s1", "r2", "2026-06-01T11:00:00Z", 5),
        ],
    );
    let out = rig.collect();
    let observed: Vec<_> = out
        .coverage
        .iter()
        .filter(|c| c.state() == CoverageState::Observed)
        .collect();
    assert_eq!(observed.len(), 1);
    assert_eq!(
        observed[0].from(),
        Iso::parse("2026-06-01T10:00:00Z").unwrap()
    );
    assert_eq!(observed[0].to(), now());
    assert_eq!(rig.store.pending_counts().unwrap().1, 1);

    write(&rig.path("p/bad.jsonl"), &["junk".into()]);
    let with_gap = rig.collect();
    let gap_from = with_gap
        .coverage
        .iter()
        .find(|c| c.state() != CoverageState::Observed)
        .unwrap()
        .from();
    assert!(
        with_gap
            .coverage
            .iter()
            .filter(|c| c.state() == CoverageState::Observed)
            .all(|c| c.to() <= gap_from),
        "observed must not cover the failure"
    );
}

#[test]
fn the_local_account_pays_unless_the_transcript_names_a_payer() {
    let mut rig = Rig::new();
    fs::write(
        rig.root.path().join(".claude.json"),
        json!({"oauthAccount": {"organizationUuid": "org_local", "billingType": "stripe_subscription",
            "emailAddress": "nobody@example.invalid", "displayName": "Synthetic Person"}, "history": ["SYNTHETIC PROMPT"]}).to_string(),
    ).unwrap();
    write(
        &rig.path("p/a.jsonl"),
        &[line("s1", "r1", "2026-06-01T10:00:00Z", 5)],
    );
    let bridge =
        json!({"type": "bridge-session", "ownerOrganizationUuid": "org_bridge"}).to_string();
    write(
        &rig.path("p/b.jsonl"),
        &[bridge, line("s2", "r2", "2026-06-01T10:00:01Z", 5)],
    );
    let out = rig.collect();
    let by = |id: &str| -> &UsageObservation {
        out.observations
            .iter()
            .find(|o| o.external_event_id.as_str() == id)
            .unwrap()
    };
    let (local, bridged) = (by("r1"), by("r2"));
    assert_eq!(
        local.payer_account_ref.as_ref().unwrap().as_str(),
        "org_local"
    );
    assert_eq!(
        (local.payer_evidence, local.billing_mode),
        (
            Some(PayerEvidence::LocalAccount),
            Some(BillingMode::Subscription)
        )
    );
    assert_eq!(
        bridged.payer_account_ref.as_ref().unwrap().as_str(),
        "org_bridge"
    );
    assert_eq!(
        (bridged.payer_evidence, bridged.billing_mode),
        (Some(PayerEvidence::Transcript), None)
    );
    let text = serde_json::to_string(&out.observations).unwrap();
    for private in [
        "nobody@example.invalid",
        "Synthetic Person",
        "SYNTHETIC PROMPT",
    ] {
        assert!(
            !text.contains(private),
            "{private} must never leave the file"
        );
    }
}

#[test]
fn a_repository_comes_from_the_folder_and_the_folder_never_leaves() {
    if std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_err()
    {
        return; // no git on this machine
    }
    let mut rig = Rig::new();
    let project = rig.root.path().join("work").join("portal");
    fs::create_dir_all(&project).unwrap();
    let git = |args: &[&str]| {
        assert!(
            std::process::Command::new("git")
                .args(args)
                .current_dir(&project)
                .output()
                .unwrap()
                .status
                .success()
        )
    };
    git(&["init", "-q"]);
    git(&[
        "remote",
        "add",
        "origin",
        "https://user:tok_FIXTURE@github.com/Acme/Portal.git",
    ]);
    let cwd = project.to_string_lossy().into_owned();
    write(
        &rig.path("p/s1.jsonl"),
        &[line_in("s1", "r1", "2026-06-01T10:00:00Z", 5, &cwd)],
    );
    let out = rig.collect();
    assert_eq!(
        out.observations[0].repository.as_ref().unwrap().as_str(),
        "github.com/acme/portal"
    );
    let text = format!("{out:?}");
    assert!(!text.contains(&cwd) && !text.contains("tok_FIXTURE"));
    let row: String = rig
        .store
        .reader_state(Source::ClaudeCode)
        .unwrap()
        .cursors
        .values()
        .cloned()
        .collect();
    assert!(
        !row.contains("portal"),
        "the cursor keeps no folder names either"
    );
}

#[test]
fn nothing_is_saved_until_the_store_commits() {
    let mut rig = Rig::new();
    write(
        &rig.path("p/s1.jsonl"),
        &[line("s1", "r1", "2026-06-01T10:00:00Z", 5)],
    );
    // "crash" between reading and committing: the outcome is simply dropped
    let lost = read(
        &rig.config(),
        &rig.store.reader_state(Source::ClaudeCode).unwrap(),
        now(),
    );
    assert_eq!(ids(&lost), ["r1"]);
    assert_eq!(rig.store.pending_counts().unwrap(), (0, 0));
    assert!(
        rig.store
            .reader_state(Source::ClaudeCode)
            .unwrap()
            .cursors
            .is_empty()
    );
    // so the next run reads the same thing: nothing was lost
    let again = rig.collect();
    assert_eq!(ids(&again), ["r1"]);
    // "crash" right after the commit: the next run finds the cursor and the seen key already saved
    assert!(rig.collect().observations.is_empty());
    assert_eq!(
        rig.store.pending_counts().unwrap().0,
        1,
        "nothing was duplicated"
    );
}

#[test]
fn a_full_buffer_commits_nothing_and_loses_nothing() {
    let mut rig = Rig::new();
    write(
        &rig.path("p/s1.jsonl"),
        &[
            line("s1", "r1", "2026-06-01T10:00:00Z", 5),
            line("s1", "r2", "2026-06-01T10:00:01Z", 5),
        ],
    );
    let outcome = read(
        &rig.config(),
        &rig.store.reader_state(Source::ClaudeCode).unwrap(),
        now(),
    );
    let tiny = SpoolLimits {
        items: 1,
        bytes: u64::MAX,
    };
    assert_eq!(rig.store.commit_read(&outcome, tiny).unwrap(), Commit::Full);
    assert_eq!(rig.store.pending_counts().unwrap(), (0, 0));
    assert!(
        rig.store
            .reader_state(Source::ClaudeCode)
            .unwrap()
            .cursors
            .is_empty(),
        "no cursor moved"
    );
    let by_size = SpoolLimits {
        items: u64::MAX,
        bytes: 10,
    };
    assert_eq!(
        rig.store.commit_read(&outcome, by_size).unwrap(),
        Commit::Full
    );
    // once there is room, the very same data goes in
    assert_eq!(
        rig.store
            .commit_read(&outcome, SpoolLimits::default())
            .unwrap(),
        Commit::Stored
    );
    assert_eq!(rig.store.pending_counts().unwrap().0, 2);
}

#[test]
fn a_failure_in_the_middle_of_a_commit_rolls_everything_back() {
    let mut rig = Rig::new();
    write(
        &rig.path("p/s1.jsonl"),
        &[line("s1", "r1", "2026-06-01T10:00:00Z", 5)],
    );
    let outcome = read(
        &rig.config(),
        &rig.store.reader_state(Source::ClaudeCode).unwrap(),
        now(),
    );
    // make the cursor write (the last step but one) fail
    let raw = rusqlite::Connection::open(_state_db(&rig)).unwrap();
    raw.execute_batch(
        "CREATE TRIGGER boom BEFORE INSERT ON cursor BEGIN SELECT RAISE(ABORT, 'boom'); END;",
    )
    .unwrap();
    assert!(
        rig.store
            .commit_read(&outcome, SpoolLimits::default())
            .is_err()
    );
    assert_eq!(
        rig.store.pending_counts().unwrap(),
        (0, 0),
        "the buffered observation was rolled back"
    );
    assert!(
        rig.store
            .reader_state(Source::ClaudeCode)
            .unwrap()
            .seen
            .is_empty(),
        "and so were the seen keys"
    );
}

fn _state_db(rig: &Rig) -> PathBuf {
    rig._data.path().join("state.db")
}

#[test]
fn the_state_roundtrips_and_old_event_keys_are_forgotten_after_45_days() {
    let mut rig = Rig::new();
    write(
        &rig.path("p/s1.jsonl"),
        &[line("s1", "r1", "2026-06-01T10:00:00Z", 5)],
    );
    rig.collect();
    assert_eq!(
        rig.store
            .reader_state(Source::ClaudeCode)
            .unwrap()
            .seen
            .len(),
        1
    );
    let later = Iso::parse("2026-07-30T12:00:00Z").unwrap(); // 59 days on
    let empty = ReadOutcome::empty(Source::ClaudeCode, later);
    rig.store
        .commit_read(&empty, SpoolLimits::default())
        .unwrap();
    assert!(
        rig.store
            .reader_state(Source::ClaudeCode)
            .unwrap()
            .seen
            .is_empty()
    );
}

#[test]
fn enabling_a_source_is_remembered() {
    let rig = Rig::new();
    assert!(!rig.store.source_enabled(Source::Codex).unwrap());
    rig.store.enable_source(Source::Codex, now()).unwrap();
    assert!(rig.store.source_enabled(Source::Codex).unwrap());
    assert!(!rig.store.source_enabled(Source::ClaudeCode).unwrap());
}
