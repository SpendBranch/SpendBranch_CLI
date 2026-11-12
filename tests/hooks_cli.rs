//! `hooks install|uninstall` and the hook the agents run, against a fake `System` (no process
//! is ever started).
#[path = "common/fake_server.rs"]
mod fake_server;
#[path = "common/rig.rs"]
mod rig;

use rig::{EXE, Rig};
use serde_json::{Value, json};
use spendbranch::cli::messages as m;
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

// --- hooks install / uninstall --------------------------------------------------------

#[test]
fn hooks_install_creates_the_file_owner_only_and_a_second_run_changes_nothing() {
    let rig = Rig::new();
    let out = rig.run(&["hooks", "install", "claude-code"]);
    assert_eq!(out.code, 0, "{}", out.text);
    assert!(
        out.text.contains("Installed the Claude Code hook"),
        "{}",
        out.text
    );
    let file = claude_file(&rig);
    let first = fs::read(&file).unwrap();
    assert!(
        String::from_utf8_lossy(&first)
            .contains("'/opt/spend branch/spendbranch' hook claude-code")
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let again = rig.run(&["hooks", "install", "claude-code"]);
    assert_eq!(
        (again.code, again.text.as_str()),
        (0, "The Claude Code hook is already installed.\n")
    );
    assert_eq!(fs::read(&file).unwrap(), first);
    assert!(
        !file
            .with_file_name("settings.json.spendbranch.bak")
            .exists(),
        "no change, no backup"
    );
}

#[test]
fn an_existing_file_is_backed_up_keeps_its_mode_and_its_other_content() {
    let rig = Rig::new();
    let file = claude_file(&rig);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    let original = "{\n  \"model\": \"opus\"\n}\n";
    fs::write(&file, original).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&file, fs::Permissions::from_mode(0o640)).unwrap();
    }
    assert_eq!(rig.run(&["hooks", "install", "claude-code"]).code, 0);
    let backup = file.with_file_name("settings.json.spendbranch.bak");
    assert_eq!(fs::read_to_string(&backup).unwrap(), original);
    assert_eq!(read_json(&file)["model"], "opus");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }
    // Uninstalling gives the original file back, byte for byte.
    let out = rig.run(&["hooks", "uninstall", "claude-code"]);
    assert_eq!(out.code, 0, "{}", out.text);
    assert_eq!(fs::read_to_string(&file).unwrap(), original);
}

#[test]
fn a_file_that_is_not_valid_json_is_never_touched() {
    let rig = Rig::new();
    let file = codex_file(&rig);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    for content in [&b"{ nope"[..], b"[]", b"\xff\xfe\x00"] {
        fs::write(&file, content).unwrap();
        for action in ["install", "uninstall"] {
            let out = rig.run(&["hooks", action, "codex"]);
            assert_eq!(out.code, 1, "{action}");
            assert!(
                out.text.contains("is not valid JSON; nothing was changed."),
                "{}",
                out.text
            );
            assert_eq!(fs::read(&file).unwrap(), content);
        }
    }
    assert!(!file.with_file_name("hooks.json.spendbranch.bak").exists());
    assert!(!file.with_file_name("hooks.json.spendbranch.tmp").exists());
}

#[test]
fn a_hooks_value_of_another_shape_is_refused_with_its_own_message() {
    let rig = Rig::new();
    let file = claude_file(&rig);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, r#"{"hooks": []}"#).unwrap();
    let out = rig.run(&["hooks", "install", "claude-code"]);
    assert_eq!(out.code, 1);
    assert!(
        out.text.contains("does not have the layout"),
        "{}",
        out.text
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), r#"{"hooks": []}"#);
}

#[test]
fn codex_asks_the_person_to_trust_the_new_hook() {
    let rig = Rig::new();
    let out = rig.run(&["hooks", "install", "codex"]);
    assert_eq!(out.code, 0);
    assert!(out.text.contains(m::CODEX_TRUST), "{}", out.text);
    assert_eq!(event_count(&codex_file(&rig), "Stop"), 1);
    assert_eq!(event_count(&codex_file(&rig), "SessionEnd"), 1);
    let claude = rig.run(&["hooks", "install", "claude-code"]);
    assert!(!claude.text.contains("trust"), "{}", claude.text);
}

#[cfg(unix)]
#[test]
fn a_symlinked_settings_file_stays_a_symlink() {
    let rig = Rig::new();
    let real = rig.dir.path().join("dotfiles/settings.json");
    fs::create_dir_all(real.parent().unwrap()).unwrap();
    fs::write(&real, "{}\n").unwrap();
    let link = claude_file(&rig);
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert_eq!(rig.run(&["hooks", "install", "claude-code"]).code, 0);
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(
        fs::read_to_string(&real)
            .unwrap()
            .contains("hook claude-code")
    );
}

#[test]
fn the_old_typescript_line_is_replaced_when_installing() {
    let rig = Rig::new();
    let file = claude_file(&rig);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    let old = "node '/x/spendbranch/dist/cli.js' hook claude-code";
    fs::write(
        &file,
        json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": old}]}]}}).to_string(),
    )
    .unwrap();
    assert_eq!(rig.run(&["hooks", "install", "claude-code"]).code, 0);
    let text = fs::read_to_string(&file).unwrap();
    assert!(!text.contains("node '"), "{text}");
    assert_eq!(event_count(&file, "Stop"), 1);
}

#[test]
fn the_hook_does_not_carry_a_custom_data_folder_and_says_so() {
    let rig = Rig::new();
    let out = rig.run(&["hooks", "install", "claude-code"]); // the rig sets SPENDBRANCH_HOME
    assert!(out.text.contains(m::HOME_NOT_CARRIED), "{}", out.text);
}

// --- the hook -------------------------------------------------------------------------

fn spawned(rig: &Rig) -> usize {
    rig.system.spawned.borrow().len()
}

#[test]
fn the_end_of_a_session_starts_a_detached_sync_and_logs_only_the_event() {
    let rig = Rig::ready();
    rig.claude_event("SessionEnd");
    let out = rig.run(&["hook", "claude-code"]);
    assert_eq!(
        (out.code, out.text.as_str()),
        (0, ""),
        "the hook says nothing"
    );
    let spawned_now = rig.system.spawned.borrow();
    assert_eq!(spawned_now.len(), 1);
    let (exe, args, log) = &spawned_now[0];
    assert_eq!(
        (exe.to_str().unwrap(), args.as_slice()),
        (EXE, ["sync".to_string()].as_slice())
    );
    assert_eq!(log, &rig.dir.path().join("hook.log"));
    let text = fs::read_to_string(log).unwrap();
    assert!(
        text.contains(r#""event":"SessionEnd""#) && text.contains(r#""session":"sess-1""#),
        "{text}"
    );
    assert!(
        !text.contains("PRIVATE"),
        "nothing the agent said reaches the log: {text}"
    );
}

#[test]
fn a_stop_waits_ten_minutes_after_the_last_sync_but_a_session_end_does_not() {
    let rig = Rig::ready();
    let ago = |ms: i64| {
        spendbranch::contract::Iso::from_millis(rig::now_ms() - ms)
            .unwrap()
            .to_string()
    };
    let min = 60_000;
    rig.store().sync_set("lastSyncAt", &ago(2 * min)).unwrap();
    rig.claude_event("Stop");
    assert_eq!(rig.run(&["hook", "claude-code"]).code, 0);
    assert_eq!(spawned(&rig), 0, "synced two minutes ago");
    rig.claude_event("SessionEnd");
    rig.run(&["hook", "claude-code"]);
    assert_eq!(spawned(&rig), 1, "the end of a session always goes");
    rig.store().sync_set("lastSyncAt", &ago(11 * min)).unwrap();
    rig.claude_event("Stop");
    rig.run(&["hook", "claude-code"]);
    assert_eq!(spawned(&rig), 2);
}

#[test]
fn codex_never_reads_stdin_and_logs_its_event() {
    let rig = Rig::ready();
    *rig.system.stdin.borrow_mut() = Some(b"{\"last_assistant_message\":\"PRIVATE\"}".to_vec());
    let out = rig.run(&["hook", "codex", "SessionEnd"]);
    assert_eq!((out.code, out.text.as_str()), (0, ""));
    assert_eq!(
        rig.system.stdin_reads.get(),
        0,
        "Codex's stdin carries the assistant's answer"
    );
    assert_eq!(spawned(&rig), 1);
    let log = fs::read_to_string(rig.dir.path().join("hook.log")).unwrap();
    assert!(log.contains(r#""event":"codex:SessionEnd""#), "{log}");
    assert!(!log.contains("PRIVATE"));
    // A Codex Stop obeys the interval too.
    rig.store()
        .sync_set(
            "lastSyncAt",
            &spendbranch::contract::Iso::from_millis(rig::now_ms())
                .unwrap()
                .to_string(),
        )
        .unwrap();
    rig.run(&["hook", "codex", "Stop"]);
    assert_eq!(spawned(&rig), 1);
    // An event name with odd characters is not logged as it came.
    rig.run(&["hook", "codex", "Stop;rm -rf"]);
    let log = fs::read_to_string(rig.dir.path().join("hook.log")).unwrap();
    assert!(
        log.contains(r#""event":"codex:?""#) && !log.contains("rm -rf"),
        "{log}"
    );
}

#[test]
fn a_device_that_is_not_connected_starts_nothing_and_creates_no_database() {
    let rig = Rig::new();
    rig.claude_event("SessionEnd");
    assert_eq!(rig.run(&["hook", "claude-code"]).code, 0);
    assert_eq!(spawned(&rig), 0);
    assert!(!rig.dir.path().join("state.db").exists());
}

#[test]
fn a_database_without_a_pairing_starts_nothing() {
    let rig = Rig::new();
    rig.run(&["status"]); // opens the database, creating it, but nobody logged in
    assert!(rig.dir.path().join("state.db").exists());
    rig.claude_event("SessionEnd");
    assert_eq!(rig.run(&["hook", "claude-code"]).code, 0);
    assert_eq!(spawned(&rig), 0);
}

#[test]
fn a_leftover_temporary_file_does_not_leak_into_the_new_one() {
    let rig = Rig::new();
    let file = claude_file(&rig);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(
        file.with_file_name("settings.json.spendbranch.tmp"),
        "x".repeat(10_000),
    )
    .unwrap();
    assert_eq!(rig.run(&["hooks", "install", "claude-code"]).code, 0);
    assert_eq!(event_count(&file, "Stop"), 1, "still valid JSON");
}

#[test]
fn systemd_quoting_survives_percent_dollar_quote_and_backslash() {
    use spendbranch::scheduler::{Job, systemd_service};
    let service = systemd_service(&Job {
        exe: "/opt/100%/a$b \"q\"/spendbranch",
        home: Some("/d/50%"),
    });
    assert!(
        service.contains("ExecStart=\"/opt/100%%/a$$b \\\"q\\\"/spendbranch\" sync\n"),
        "{service}"
    );
    assert!(
        service.contains("Environment=\"SPENDBRANCH_HOME=/d/50%%\"\n"),
        "{service}"
    );
}

#[test]
fn the_hook_exits_0_even_when_everything_fails() {
    let rig = Rig::ready();
    // The data folder is a file: the log and the database cannot be used.
    let blocker = rig.dir.path().join("a-file");
    fs::write(&blocker, "x").unwrap();
    rig.claude_event("SessionEnd");
    let out = rig.run_with(
        &["hook", "claude-code"],
        "",
        false,
        &[("SPENDBRANCH_HOME", blocker.to_str().unwrap())],
    );
    assert_eq!((out.code, out.text.as_str()), (0, ""));
    // A relative data folder too.
    let out = rig.run_with(
        &["hook", "codex", "Stop"],
        "",
        false,
        &[("SPENDBRANCH_HOME", "relative")],
    );
    assert_eq!((out.code, out.text.as_str()), (0, ""));
    assert_eq!(rig.run(&["hook", "gemini"]).code, 2);
}
