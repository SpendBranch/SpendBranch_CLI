//! What makes the collection run by itself: the hook files, the hook, the system schedule and
//! `setup`, against the fake server and a fake `System` (no process is ever started).
#[path = "common/fake_server.rs"]
mod fake_server;
#[path = "common/rig.rs"]
mod rig;

use rig::{FakeSystem, Rig, authorize};
use serde_json::{Value, json};
use spendbranch::cli::messages as m;
use spendbranch::infra::scheduler::{self, Method, Platform};
use std::{collections::HashMap, fs, path::Path};

const EXE: &str = "/opt/spend branch/spendbranch";

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

fn claude_event(rig: &Rig, event: &str) {
    *rig.system.stdin.borrow_mut() = Some(
        json!({
            "hook_event_name": event, "session_id": "sess-1",
            "transcript_path": "/home/me/PRIVATE-PATH.jsonl", "prompt": "PRIVATE-PROMPT",
        })
        .to_string()
        .into_bytes(),
    );
}

#[test]
fn the_end_of_a_session_starts_a_detached_sync_and_logs_only_the_event() {
    let rig = Rig::ready();
    claude_event(&rig, "SessionEnd");
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
    claude_event(&rig, "Stop");
    assert_eq!(rig.run(&["hook", "claude-code"]).code, 0);
    assert_eq!(spawned(&rig), 0, "synced two minutes ago");
    claude_event(&rig, "SessionEnd");
    rig.run(&["hook", "claude-code"]);
    assert_eq!(spawned(&rig), 1, "the end of a session always goes");
    rig.store().sync_set("lastSyncAt", &ago(11 * min)).unwrap();
    claude_event(&rig, "Stop");
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
    claude_event(&rig, "SessionEnd");
    assert_eq!(rig.run(&["hook", "claude-code"]).code, 0);
    assert_eq!(spawned(&rig), 0);
    assert!(!rig.dir.path().join("state.db").exists());
}

#[test]
fn a_database_without_a_pairing_starts_nothing() {
    let rig = Rig::new();
    rig.run(&["status"]); // opens the database, creating it, but nobody logged in
    assert!(rig.dir.path().join("state.db").exists());
    claude_event(&rig, "SessionEnd");
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
    claude_event(&rig, "SessionEnd");
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

// --- the schedule ---------------------------------------------------------------------

fn unit_dir(rig: &Rig) -> std::path::PathBuf {
    rig.dir.path().join(".config/systemd/user")
}

fn env_of(rig: &Rig, extra: &[(&str, &str)]) -> HashMap<String, String> {
    let mut env: HashMap<String, String> = [("HOME", rig.dir.path().to_str().unwrap())]
        .map(|(k, v)| (k.into(), v.into()))
        .into();
    env.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    env
}

fn install(rig: &Rig, platform: Platform, extra: &[(&str, &str)]) -> Result<Method, String> {
    let env = env_of(rig, extra);
    scheduler::install(&rig.system, &|k| env.get(k).cloned(), platform, EXE)
}

#[test]
fn systemd_gets_a_service_and_a_timer_that_run_the_sync_every_half_hour_and_at_login() {
    let rig = Rig::new();
    assert_eq!(
        install(&rig, Platform::Linux, &[("SPENDBRANCH_HOME", "/data/sb")]),
        Ok(Method::Systemd)
    );
    let service = fs::read_to_string(unit_dir(&rig).join("spendbranch-sync.service")).unwrap();
    assert!(
        service.contains("ExecStart=\"/opt/spend branch/spendbranch\" sync\n"),
        "{service}"
    );
    assert!(
        service.contains("Environment=\"SPENDBRANCH_HOME=/data/sb\"\n"),
        "{service}"
    );
    assert!(service.contains("Type=oneshot"));
    let timer = fs::read_to_string(unit_dir(&rig).join("spendbranch-sync.timer")).unwrap();
    assert!(
        timer.contains("OnStartupSec=2min") && timer.contains("OnUnitActiveSec=30min"),
        "{timer}"
    );
    let calls = rig.system.tool_calls("systemctl");
    assert_eq!(
        calls.last().unwrap(),
        &["--user", "enable", "--now", "spendbranch-sync.timer"]
    );
    assert!(calls.iter().any(|c| c == &["--user", "daemon-reload"]));
}

#[test]
fn installing_the_schedule_again_leaves_the_same_files_and_one_timer() {
    let rig = Rig::new();
    install(&rig, Platform::Linux, &[]).unwrap();
    let before = fs::read_to_string(unit_dir(&rig).join("spendbranch-sync.timer")).unwrap();
    install(&rig, Platform::Linux, &[]).unwrap();
    assert_eq!(
        fs::read_to_string(unit_dir(&rig).join("spendbranch-sync.timer")).unwrap(),
        before
    );
    assert_eq!(fs::read_dir(unit_dir(&rig)).unwrap().count(), 2);
}

#[test]
fn without_systemd_the_crontab_gets_our_lines_and_the_others_stay() {
    let rig = Rig::new();
    rig.system.answer("systemctl --user", 1, "");
    rig.system.answer("crontab -l", 0, "0 3 * * * backup\n");
    assert_eq!(
        install(&rig, Platform::Linux, &[("SPENDBRANCH_HOME", "/d/it's")]),
        Ok(Method::Cron)
    );
    let calls = rig.system.ran.borrow();
    let (_, args, stdin) = calls.iter().rfind(|(p, ..)| p == "crontab").unwrap();
    assert_eq!(args, &["-"]);
    let table = stdin.clone().unwrap();
    assert!(table.starts_with("0 3 * * * backup\n"), "{table}");
    assert!(table.contains("*/30 * * * * SPENDBRANCH_HOME='/d/it'\\''s' '/opt/spend branch/spendbranch' sync >/dev/null 2>&1 # spendbranch-sync\n"), "{table}");
    assert!(table.contains("@reboot "), "{table}");
    drop(calls);
    // Run again over a table that already has our lines: they are replaced, not added.
    let once = table.clone();
    rig.system.answer("crontab -l", 0, &once);
    install(&rig, Platform::Linux, &[("SPENDBRANCH_HOME", "/d/it's")]).unwrap();
    let calls = rig.system.ran.borrow();
    let (_, _, stdin) = calls.iter().rfind(|(p, ..)| p == "crontab").unwrap();
    assert_eq!(stdin.as_deref(), Some(once.as_str()));
}

#[test]
fn with_neither_systemd_nor_cron_it_says_so() {
    let rig = Rig::new();
    rig.system
        .missing
        .borrow_mut()
        .extend(["systemctl".to_string(), "crontab".to_string()]);
    let why = install(&rig, Platform::Linux, &[]).unwrap_err();
    assert!(
        why.contains("neither `systemctl --user` nor `crontab`"),
        "{why}"
    );
}

#[test]
fn a_tool_that_fails_is_reported_with_what_it_said() {
    let rig = Rig::new();
    rig.system.answer("systemctl --user enable", 1, "");
    let why = install(&rig, Platform::Linux, &[]).unwrap_err();
    assert!(why.contains("systemctl"), "{why}");
}

#[test]
fn macos_gets_a_launch_agent_that_runs_at_load_and_every_half_hour() {
    let rig = Rig::new();
    assert_eq!(install(&rig, Platform::MacOs, &[]), Ok(Method::Launchd));
    let plist = fs::read_to_string(
        rig.dir
            .path()
            .join("Library/LaunchAgents/com.spendbranch.sync.plist"),
    )
    .unwrap();
    assert!(
        plist.contains("<string>/opt/spend branch/spendbranch</string>"),
        "{plist}"
    );
    assert!(
        plist.contains("<key>StartInterval</key>\n  <integer>1800</integer>"),
        "{plist}"
    );
    assert!(plist.contains("<key>RunAtLoad</key>\n  <true/>"), "{plist}");
    let calls = rig.system.tool_calls("launchctl");
    assert_eq!(calls.last().unwrap()[0..2], ["load", "-w"]);
}

#[test]
fn windows_gets_two_tasks_one_every_half_hour_and_one_at_logon() {
    let rig = Rig::new();
    assert_eq!(
        install(&rig, Platform::Windows, &[]),
        Ok(Method::TaskScheduler)
    );
    let calls = rig.system.tool_calls("schtasks");
    assert_eq!(calls.len(), 2);
    let all = calls.iter().map(|c| c.join(" ")).collect::<Vec<_>>();
    assert!(
        all[0].contains(
            "/TN SpendBranchSync /TR \"/opt/spend branch/spendbranch\" sync /SC MINUTE /MO 30 /F"
        ),
        "{}",
        all[0]
    );
    assert!(
        all[1].contains("/TN SpendBranchSyncLogon") && all[1].contains("/SC ONLOGON"),
        "{}",
        all[1]
    );
}

#[test]
fn removing_takes_away_what_was_installed_and_is_quiet_when_nothing_was() {
    for platform in [Platform::Linux, Platform::MacOs] {
        let rig = Rig::new();
        let env = env_of(&rig, &[]);
        let env = |k: &str| env.get(k).cloned();
        assert_eq!(scheduler::remove(&rig.system, &env, platform), Ok(false));
        assert!(!scheduler::installed(&rig.system, &env, platform));
        install(&rig, platform, &[]).unwrap();
        assert!(
            scheduler::installed(&rig.system, &env, platform),
            "{platform:?}"
        );
        assert_eq!(scheduler::remove(&rig.system, &env, platform), Ok(true));
        assert!(
            !scheduler::installed(&rig.system, &env, platform),
            "{platform:?}"
        );
    }
}

#[test]
fn removing_cleans_our_cron_lines_and_nothing_else() {
    let rig = Rig::new();
    rig.system.answer("systemctl --user", 1, "");
    rig.system.answer(
        "crontab -l",
        0,
        "0 3 * * * backup\n*/30 * * * * '/x' sync >/dev/null 2>&1 # spendbranch-sync\n@reboot '/x' sync # spendbranch-sync\n",
    );
    let env = env_of(&rig, &[]);
    assert_eq!(
        scheduler::remove(&rig.system, &|k| env.get(k).cloned(), Platform::Linux),
        Ok(true)
    );
    let calls = rig.system.ran.borrow();
    let (_, _, stdin) = calls
        .iter()
        .rfind(|(p, a, _)| p == "crontab" && a == &["-"])
        .unwrap();
    assert_eq!(stdin.as_deref(), Some("0 3 * * * backup\n"));
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
        "Claude Code: found",
        "Codex: found",
        "Collection on for Claude Code",
        "Installed the Codex hook",
        "Scheduled sync: every 30 minutes and at login (systemd user timer).",
        "Summary: Claude Code: connected, with the hook; Codex: connected, with the hook.",
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
    assert!(unit_dir(&rig).join("spendbranch-sync.timer").exists());
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
        unit_dir(&rig).join("spendbranch-sync.timer"),
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
        out.text.contains("Claude Code: not found") && out.text.contains(m::SETUP_NO_AGENT),
        "{}",
        out.text
    );
    assert!(
        !unit_dir(&rig).exists(),
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
    assert!(!out.text.contains("Claude Code: found"), "{}", out.text);
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
        out.text
            .contains("Summary: Claude Code: connected, with the hook; Codex: not connected."),
        "{}",
        out.text
    );
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
        out.text.contains("Claude Code: connected, with the hook"),
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
            .contains("Hook: not installed (run `spendbranch hooks install claude-code`)"),
        "{}",
        out.text
    );
    assert!(
        out.text
            .contains("Scheduled sync: not installed (run `spendbranch setup`)"),
        "{}",
        out.text
    );
    rig.run(&["hooks", "install", "claude-code"]);
    rig.run(&["hooks", "install", "codex"]);
    install(&rig, Platform::Linux, &[]).unwrap();
    let out = rig.run(&["status"]);
    assert!(out.text.contains("  Hook: installed"), "{}", out.text);
    assert!(
        out.text.contains("Scheduled sync: installed"),
        "{}",
        out.text
    );
    // Codex only runs hooks it trusts: no run in the log is worth a warning.
    assert!(
        out.text.contains("Codex has not run the hook in 24 hours"),
        "{}",
        out.text
    );
    claude_event(&rig, "Stop");
    rig.run(&["hook", "codex", "Stop"]);
    let out = rig.run(&["status"]);
    assert!(out.text.contains("Last hook run: "), "{}", out.text);
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
    assert!(
        out.text
            .contains("Hook: installed, but it is not the line this build writes"),
        "{}",
        out.text
    );
}

#[test]
fn logout_removes_the_scheduled_sync() {
    let rig = Rig::ready();
    install(&rig, Platform::Linux, &[]).unwrap();
    let out = rig.run(&["logout"]);
    assert_eq!(out.code, 0, "{}", out.text);
    assert!(out.text.contains(m::SCHEDULER_REMOVED), "{}", out.text);
    assert!(!unit_dir(&rig).join("spendbranch-sync.timer").exists());
    // A second logout has nothing left to remove and says nothing about it.
    assert!(!rig.run(&["logout"]).text.contains("Scheduled"));
}

/// On a machine with a systemd user manager: the real timer, started by hand, runs the stand-in
/// program, and removing leaves nothing behind. Touches the real user units (briefly), so it
/// only runs when asked: `cargo test --test triggers -- --ignored real_systemd`.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "installs a real user timer for a moment"]
fn real_systemd_runs_the_sync_and_removal_leaves_nothing() {
    use spendbranch::infra::system::{Real, System};
    use std::os::unix::fs::PermissionsExt;
    struct Cleanup;
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let env = |k: &str| std::env::var(k).ok();
            let _ = scheduler::remove(&Real, &env, Platform::Linux);
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let stamp = dir.path().join("ran");
    let stub = dir.path().join("spendbranch");
    fs::write(
        &stub,
        format!("#!/bin/sh\necho \"$@\" > '{}'\n", stamp.display()),
    )
    .unwrap();
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
    let env = |k: &str| std::env::var(k).ok();
    let _cleanup = Cleanup;
    assert_eq!(
        scheduler::install(&Real, &env, Platform::Linux, stub.to_str().unwrap()),
        Ok(Method::Systemd)
    );
    assert!(scheduler::installed(&Real, &env, Platform::Linux));
    let timers = Real
        .run(
            "systemctl",
            &[
                "--user",
                "list-timers",
                "spendbranch-sync.timer",
                "--no-pager",
            ],
            None,
        )
        .unwrap();
    assert!(
        timers.stdout.contains("spendbranch-sync.timer"),
        "{}",
        timers.stdout
    );
    let started = Real
        .run(
            "systemctl",
            &["--user", "start", "spendbranch-sync.service"],
            None,
        )
        .unwrap();
    assert_eq!(started.code, 0, "{}", started.stderr);
    assert_eq!(fs::read_to_string(&stamp).unwrap().trim(), "sync");
    assert_eq!(scheduler::remove(&Real, &env, Platform::Linux), Ok(true));
    assert!(!scheduler::installed(&Real, &env, Platform::Linux));
    let after = Real
        .run(
            "systemctl",
            &[
                "--user",
                "list-timers",
                "spendbranch-sync.timer",
                "--no-pager",
            ],
            None,
        )
        .unwrap();
    assert!(
        !after.stdout.contains("spendbranch-sync.timer"),
        "{}",
        after.stdout
    );
}
