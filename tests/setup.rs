//! `setup`, and how `status`, `enable` and `logout` report the hook and the schedule.
use crate::rig;

use rig::{FakeSystem, Rig, authorize};
use serde_json::Value;
use spendbranch::cli::messages as m;
use spendbranch::infra::scheduler::Platform;
use std::{fs, path::Path};

fn claude_file(rig: &Rig) -> std::path::PathBuf {
    rig.dir.path().join(".claude/settings.json")
}

fn codex_file(rig: &Rig) -> std::path::PathBuf {
    rig.dir.path().join(".codex/hooks.json")
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn event_count(file: &Path, event: &str) -> usize {
    read_json(file)["hooks"][event]
        .as_array()
        .map_or(0, Vec::len)
}

// --- setup ----------------------------------------------------------------------------

fn agents(rig: &Rig, claude: bool, codex: bool) {
    for (there, dir) in [(claude, ".claude"), (codex, ".codex")] {
        if there {
            fs::create_dir_all(rig.dir.path().join(dir)).unwrap();
        }
    }
}

#[test]
fn setup_connects_turns_collection_on_installs_both_hooks_and_the_schedule() {
    let rig = Rig::new();
    authorize(&rig);
    agents(&rig, true, true);
    let out = rig.run(&["setup"]);
    assert_eq!(out.code, 0, "{}", out.text);
    for expected in [
        "[ok] Connect     Acme Inc.",
        "[ok] Agents      Claude Code found · Codex found",
        "[ok] Reading     Claude Code · Codex",
        "    Collection on for Claude Code",
        "    Reads 32 fields from Claude Code sessions", // the list itself is `enable`'s
        "[ok] Hooks       Claude Code · Codex",
        "    Installed the Codex hook",
        "[ok] Schedule    every 30 minutes and at login (systemd user timer)",
        m::SETUP_READY,
    ] {
        assert!(
            out.text.contains(expected),
            "missing `{expected}` in:\n{}",
            out.text
        );
    }
    assert!(
        !out.text.contains("is not installed: run"),
        "setup asks for no hint:\n{}",
        out.text
    );
    assert_eq!(event_count(&claude_file(&rig), "Stop"), 1);
    assert_eq!(event_count(&codex_file(&rig), "SessionEnd"), 1);
    assert!(rig.unit_dir().join("spendbranch-sync.timer").exists());
}

#[test]
fn running_setup_again_changes_nothing() {
    let rig = Rig::new();
    authorize(&rig);
    agents(&rig, true, true);
    assert_eq!(rig.run(&["setup"]).code, 0);
    let device = rig.store().meta_get("device_id").unwrap();
    let files = [
        claude_file(&rig),
        codex_file(&rig),
        rig.unit_dir().join("spendbranch-sync.timer"),
    ];
    let before: Vec<_> = files.iter().map(|f| fs::read(f).unwrap()).collect();
    let pairings = rig
        .paths_seen()
        .iter()
        .filter(|p| p.ends_with("/device-codes"))
        .count();

    let out = rig.run(&["setup"]);
    assert_eq!(out.code, 0, "{}", out.text);
    assert!(out.text.contains("Already connected to"), "{}", out.text);
    assert!(out.text.contains("is already installed"), "{}", out.text);
    assert_eq!(
        rig.store().meta_get("device_id").unwrap(),
        device,
        "no second device"
    );
    assert_eq!(
        rig.paths_seen()
            .iter()
            .filter(|p| p.ends_with("/device-codes"))
            .count(),
        pairings
    );
    let after: Vec<_> = files.iter().map(|f| fs::read(f).unwrap()).collect();
    assert_eq!(before, after);
}

#[test]
fn setup_without_any_agent_says_so_and_fails() {
    let rig = Rig::new();
    authorize(&rig);
    let out = rig.run(&["setup"]);
    assert_eq!(out.code, 1);
    assert!(
        out.text
            .contains("[x] Agents      Claude Code not found · Codex not found")
            && out.text.contains(m::SETUP_NO_AGENT),
        "{}",
        out.text
    );
    assert!(
        !rig.unit_dir().exists(),
        "nothing to collect, nothing scheduled"
    );
}

#[test]
fn setup_only_does_what_was_found_and_asked() {
    let rig = Rig::new();
    authorize(&rig);
    agents(&rig, true, true);
    let out = rig.run(&["setup", "--source", "codex"]);
    assert_eq!(out.code, 0, "{}", out.text);
    assert!(
        out.text.contains("Agents      Codex found\n"),
        "{}",
        out.text
    );
    assert!(codex_file(&rig).exists() && !claude_file(&rig).exists());
}

#[test]
fn setup_fails_when_an_agent_is_found_but_the_company_did_not_authorize_it() {
    let rig = Rig::new();
    authorize(&rig);
    rig.server
        .configure(|s| s.sources.retain(|x| x["source"] == "claude_code"));
    agents(&rig, true, true);
    let out = rig.run(&["setup"]);
    assert_eq!(out.code, 1, "{}", out.text);
    assert!(
        out.text
            .contains("The company has not authorized Codex collection"),
        "{}",
        out.text
    );
    assert!(
        out.text.contains("[!] Reading     Claude Code · Codex off"),
        "{}",
        out.text
    );
    assert!(out.text.contains(m::SETUP_PROBLEMS), "{}", out.text);
    assert!(
        !codex_file(&rig).exists(),
        "no hook for a source that is not on"
    );
}

#[test]
fn setup_fails_but_still_installs_the_rest_when_the_schedule_cannot_be() {
    let rig = Rig::new();
    authorize(&rig);
    agents(&rig, true, false);
    rig.system
        .missing
        .borrow_mut()
        .extend(["systemctl".to_string(), "crontab".to_string()]);
    let out = rig.run(&["setup"]);
    assert_eq!(out.code, 1, "{}", out.text);
    assert!(
        out.text.contains("Scheduled sync: not installed (neither"),
        "{}",
        out.text
    );
    assert!(
        out.text.contains("[ok] Hooks       Claude Code\n")
            && out.text.contains("[!] Schedule    not installed"),
        "{}",
        out.text
    );
    assert_eq!(event_count(&claude_file(&rig), "Stop"), 1);
}

#[test]
fn setup_will_not_silently_switch_to_another_server() {
    let rig = Rig::new();
    authorize(&rig);
    agents(&rig, true, false);
    assert_eq!(rig.run(&["setup"]).code, 0);
    let out = rig.run(&["setup", "--server", "http://127.0.0.1:9"]);
    assert_eq!(out.code, 1);
    assert!(
        out.text
            .contains("setup --relogin --server http://127.0.0.1:9"),
        "{}",
        out.text
    );
}

#[test]
fn setup_reconnects_when_the_device_was_revoked() {
    let rig = Rig::new();
    authorize(&rig);
    agents(&rig, true, false);
    assert_eq!(rig.run(&["setup"]).code, 0);
    rig.server.configure(|s| {
        for d in s.devices.values_mut() {
            d.active = false;
        }
    });
    let out = rig.run(&["setup"]);
    assert!(out.text.contains(m::SETUP_RECONNECT), "{}", out.text);
}

// --- status, enable, logout -----------------------------------------------------------

#[test]
fn enable_hints_at_the_hook_until_it_is_installed() {
    let rig = Rig::new();
    authorize(&rig);
    assert_eq!(rig.run(&["login"]).code, 0);
    let out = rig.run(&["enable", "claude-code"]);
    assert!(
        out.text
            .contains("hook is not installed: run `spendbranch hooks install claude-code`"),
        "{}",
        out.text
    );
    rig.run(&["hooks", "install", "claude-code"]);
    let out = rig.run(&["enable", "claude-code"]);
    assert!(!out.text.contains("hook is not installed"), "{}", out.text);
}

#[test]
fn status_shows_the_hook_and_the_schedule() {
    let rig = Rig::ready();
    let out = rig.run(&["status"]);
    assert!(
        out.text
            .contains("[!] Claude Code has no hook: run `spendbranch hooks install claude-code`."),
        "{}",
        out.text
    );
    assert!(
        out.text
            .contains("[!] No scheduled sync: run `spendbranch setup`."),
        "{}",
        out.text
    );
    rig.run(&["hooks", "install", "claude-code"]);
    rig.run(&["hooks", "install", "codex"]);
    rig.install_schedule(Platform::Linux, &[]).unwrap();
    let out = rig.run(&["status"]);
    assert!(out.text.contains("[ok] hook"), "{}", out.text);
    assert!(!out.text.contains("no hook"), "{}", out.text);
    assert!(
        out.text.contains("Schedule       [ok] installed"),
        "{}",
        out.text
    );
    // Codex only runs hooks it trusts: no run in the log is worth a warning.
    assert!(
        out.text.contains("Codex has not run the hook in 24 hours"),
        "{}",
        out.text
    );
    rig.claude_event("Stop");
    rig.run(&["hook", "codex", "Stop"]);
    let out = rig.run(&["status"]);
    assert!(out.text.contains("hook ran "), "{}", out.text);
    assert!(!out.text.contains("has not run the hook"), "{}", out.text);
}

#[test]
fn status_says_when_the_hook_points_to_another_binary() {
    let rig = Rig::ready();
    rig.run(&["hooks", "install", "claude-code"]);
    let mut elsewhere = FakeSystem::new();
    elsewhere.exe = "/new/place/spendbranch".into();
    let rig = Rig {
        system: elsewhere,
        ..rig
    };
    let out = rig.run(&["status"]);
    assert!(out.text.contains("[!] old hook"), "{}", out.text);
    assert!(
        out.text
            .contains("hook is not the line this build writes (the binary moved?)"),
        "{}",
        out.text
    );
}

#[test]
fn logout_removes_the_scheduled_sync() {
    let rig = Rig::ready();
    rig.install_schedule(Platform::Linux, &[]).unwrap();
    let out = rig.run(&["logout"]);
    assert_eq!(out.code, 0, "{}", out.text);
    assert!(out.text.contains(m::SCHEDULER_REMOVED), "{}", out.text);
    assert!(!rig.unit_dir().join("spendbranch-sync.timer").exists());
    // A second logout has nothing left to remove and says nothing about it.
    assert!(!rig.run(&["logout"]).text.contains("Scheduled"));
}

/// Columns on screen: escape codes take none.
fn columns(line: &str) -> usize {
    let mut n = 0;
    let mut escape = false;
    for c in line.chars() {
        if escape {
            escape = c != 'm';
        } else if c == '\u{1b}' {
            escape = true;
        } else {
            n += 1;
        }
    }
    n
}

#[test]
fn on_a_terminal_setup_and_status_fit_80_columns_and_the_box_closes() {
    let rig = Rig::new();
    authorize(&rig);
    agents(&rig, true, true);
    let color = [("COLORTERM", "truecolor")];
    let setup = rig.run_with(&["setup"], "y\n", true, &color);
    assert_eq!(setup.code, 0, "{}", setup.text);
    let status = rig.run_with(&["status"], "", true, &color);
    assert_eq!(status.code, 0, "{}", status.text);
    if std::env::var_os("SHOW_SCREENS").is_some() {
        println!("{}\n{}", setup.text, status.text);
    }
    assert!(
        setup.text.contains("✓") && setup.text.contains('│'),
        "{}",
        setup.text
    );
    // The answer to [y/N] is not echoed here, so what follows the question shares its line.
    let lines = setup.text.lines().chain(status.text.lines());
    for line in lines.filter(|l| !l.contains("[y/N]")) {
        assert!(columns(line) <= 80, "{} columns: {line}", columns(line));
    }
    for line in status.text.lines().filter(|l| l.contains('│')) {
        assert_eq!(columns(line), 80, "a box line ends short: {line}");
    }
}
