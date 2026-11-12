//! The system schedule `setup` installs and `logout` removes, against a fake `System`; one
//! ignored test uses the real systemd.
#[path = "common/fake_server.rs"]
mod fake_server;
#[path = "common/rig.rs"]
mod rig;

use rig::Rig;
use spendbranch::infra::scheduler::{self, Method, Platform};
use std::fs;

// --- the schedule ---------------------------------------------------------------------

#[test]
fn systemd_gets_a_service_and_a_timer_that_run_the_sync_every_half_hour_and_at_login() {
    let rig = Rig::new();
    assert_eq!(
        rig.install_schedule(Platform::Linux, &[("SPENDBRANCH_HOME", "/data/sb")]),
        Ok(Method::Systemd)
    );
    let service = fs::read_to_string(rig.unit_dir().join("spendbranch-sync.service")).unwrap();
    assert!(
        service.contains("ExecStart=\"/opt/spend branch/spendbranch\" sync\n"),
        "{service}"
    );
    assert!(
        service.contains("Environment=\"SPENDBRANCH_HOME=/data/sb\"\n"),
        "{service}"
    );
    assert!(service.contains("Type=oneshot"));
    let timer = fs::read_to_string(rig.unit_dir().join("spendbranch-sync.timer")).unwrap();
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
    rig.install_schedule(Platform::Linux, &[]).unwrap();
    let before = fs::read_to_string(rig.unit_dir().join("spendbranch-sync.timer")).unwrap();
    rig.install_schedule(Platform::Linux, &[]).unwrap();
    assert_eq!(
        fs::read_to_string(rig.unit_dir().join("spendbranch-sync.timer")).unwrap(),
        before
    );
    assert_eq!(fs::read_dir(rig.unit_dir()).unwrap().count(), 2);
}

#[test]
fn without_systemd_the_crontab_gets_our_lines_and_the_others_stay() {
    let rig = Rig::new();
    rig.system.answer("systemctl --user", 1, "");
    rig.system.answer("crontab -l", 0, "0 3 * * * backup\n");
    assert_eq!(
        rig.install_schedule(Platform::Linux, &[("SPENDBRANCH_HOME", "/d/it's")]),
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
    rig.install_schedule(Platform::Linux, &[("SPENDBRANCH_HOME", "/d/it's")])
        .unwrap();
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
    let why = rig.install_schedule(Platform::Linux, &[]).unwrap_err();
    assert!(
        why.contains("neither `systemctl --user` nor `crontab`"),
        "{why}"
    );
}

#[test]
fn a_tool_that_fails_is_reported_with_what_it_said() {
    let rig = Rig::new();
    rig.system.answer("systemctl --user enable", 1, "");
    let why = rig.install_schedule(Platform::Linux, &[]).unwrap_err();
    assert!(why.contains("systemctl"), "{why}");
}

#[test]
fn macos_gets_a_launch_agent_that_runs_at_load_and_every_half_hour() {
    let rig = Rig::new();
    assert_eq!(
        rig.install_schedule(Platform::MacOs, &[]),
        Ok(Method::Launchd)
    );
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
        rig.install_schedule(Platform::Windows, &[]),
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
        let env = rig.env_of(&[]);
        let env = |k: &str| env.get(k).cloned();
        assert_eq!(scheduler::remove(&rig.system, &env, platform), Ok(false));
        assert!(!scheduler::installed(&rig.system, &env, platform));
        rig.install_schedule(platform, &[]).unwrap();
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
    let env = rig.env_of(&[]);
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

/// On a machine with a systemd user manager: the real timer, started by hand, runs the stand-in
/// program, and removing leaves nothing behind. Touches the real user units (briefly), so it
/// only runs when asked: `cargo test --test scheduler -- --ignored real_systemd`.
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
