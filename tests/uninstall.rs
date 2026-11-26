//! `uninstall`: the device revoked, the hooks, the schedule, the installer's PATH lines, the data
//! folder and the program removed, against a fake `System` (nothing outside the test folder is
//! touched, and the program file is only recorded).
use crate::rig;

use rig::{EXE, Rig, authorize};
use spendbranch::cli::messages as m;
use spendbranch::infra::scheduler::Platform;
use spendbranch::uninstall::{
    PATH_MARK, folder_is_ours, installed_by_homebrew, powershell_encoded, tilde,
    windows_path_script, without_path_lines,
};
use std::{fs, path::PathBuf};

const BASH_LINE: &str = "export PATH=\"$HOME/.local/bin:$PATH\"";

/// What install.sh appends to a startup file.
fn added(line: &str) -> String {
    format!("\n{PATH_MARK}\n{line}\n")
}

// --- the rules ------------------------------------------------------------------------

#[test]
fn install_sh_writes_the_mark_uninstall_looks_for() {
    let script =
        fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("install/install.sh"))
            .unwrap();
    assert!(
        script.contains(&format!("PATH_MARK=\"{PATH_MARK}\"")),
        "install.sh and uninstall disagree on the mark"
    );
}

#[test]
fn only_the_lines_the_installer_added_are_taken_out() {
    let mine = "alias ll='ls -l'\nexport PATH=\"$HOME/bin:$PATH\"\n";
    // Added once, at the end.
    assert_eq!(
        without_path_lines(&format!("{mine}{}", added(BASH_LINE))).as_deref(),
        Some(mine)
    );
    // Added to an empty file, and twice (both go).
    assert_eq!(without_path_lines(&added(BASH_LINE)).as_deref(), Some(""));
    let twice = format!(
        "{mine}{}{mine}{}",
        added(BASH_LINE),
        added("export PATH='/x y':\"$PATH\"")
    );
    assert_eq!(without_path_lines(&twice), Some(format!("{mine}{mine}")));
    // fish, and Windows line ends.
    assert_eq!(
        without_path_lines(&added("fish_add_path '/home/me/.local/bin'")).as_deref(),
        Some("")
    );
    assert_eq!(
        without_path_lines(&format!("a\r\n\r\n{PATH_MARK}\r\n{BASH_LINE}\r\nb\r\n")).as_deref(),
        Some("a\r\nb\r\n")
    );
    // A mark followed by someone else's line: the mark goes, their line stays.
    assert_eq!(
        without_path_lines(&format!("{PATH_MARK}\nalias x=y\n")).as_deref(),
        Some("alias x=y\n")
    );
    // Nothing of ours: nothing to write.
    assert_eq!(without_path_lines(mine), None);
    assert_eq!(
        without_path_lines(&format!("# {PATH_MARK}\n{BASH_LINE}\n")),
        None
    );
}

#[test]
fn only_a_folder_named_spendbranch_is_ours() {
    assert!(folder_is_ours(Some("spendbranch")));
    assert!(folder_is_ours(Some("SpendBranch")));
    for other in [Some("me"), Some("spendbranch-data"), Some(""), None] {
        assert!(!folder_is_ours(other), "{other:?}");
    }
}

#[test]
fn only_a_windows_program_folder_named_spendbranch_leaves_the_path() {
    use spendbranch::uninstall::windows_folder_of;
    assert_eq!(
        windows_folder_of(r"C:\Users\me\AppData\Local\Programs\spendbranch\spendbranch.exe"),
        Some(r"C:\Users\me\AppData\Local\Programs\spendbranch")
    );
    assert_eq!(windows_folder_of(r"C:\tools\spendbranch.exe"), None);
    assert_eq!(windows_folder_of("spendbranch.exe"), None);
}

#[test]
fn homebrew_installs_are_left_to_brew() {
    assert!(installed_by_homebrew(
        "/opt/homebrew/Cellar/spendbranch/0.2.0/bin/spendbranch"
    ));
    assert!(!installed_by_homebrew("/home/me/.local/bin/spendbranch"));
}

#[test]
fn the_windows_path_script_quotes_the_folder_and_keeps_the_value_expandable() {
    let script = windows_path_script(r"C:\Users\o'neil\AppData\Local\Programs\spendbranch");
    assert!(
        script.contains(r"$d = 'C:\Users\o''neil\AppData\Local\Programs\spendbranch'"),
        "{script}"
    );
    assert!(script.contains("DoNotExpandEnvironmentNames"));
    assert!(script.contains("'ExpandString'"));
    // base64 of UTF-16LE, as -EncodedCommand wants.
    assert_eq!(powershell_encoded("ab"), "YQBiAA==");
}

// --- the command ----------------------------------------------------------------------

/// A data folder named `spendbranch` inside the test folder (the rig's default is the test
/// folder itself, which uninstall must never delete).
fn home(rig: &Rig) -> (PathBuf, String) {
    let dir = rig.dir.path().join("spendbranch");
    let text = dir.to_str().unwrap().to_owned();
    (dir, text)
}

fn run(rig: &Rig, args: &[&str], input: &str, tty: bool) -> rig::Out {
    let (_, data) = home(rig);
    rig.run_with(args, input, tty, &[("SPENDBRANCH_HOME", &data)])
}

#[test]
fn uninstall_removes_everything_and_leaves_the_rest() {
    let rig = Rig::new();
    authorize(&rig);
    let (data, _) = home(&rig);
    assert_eq!(run(&rig, &["login"], "y\n", false).code, 0);
    assert_eq!(
        run(&rig, &["hooks", "install", "claude-code"], "", false).code,
        0
    );
    // Someone else's hook and the person's own shell lines must survive.
    let settings = rig.dir.path().join(".claude/settings.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
    json["hooks"]["SessionEnd"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"hooks": [{"type": "command", "command": "echo bye"}]}));
    fs::write(&settings, json.to_string()).unwrap();
    let profile = rig.dir.path().join(".profile");
    fs::write(&profile, format!("umask 022\n{}", added(BASH_LINE))).unwrap();
    let fish = rig
        .dir
        .path()
        .join(".config")
        .join("fish")
        .join("conf.d")
        .join("spendbranch.fish");
    fs::create_dir_all(fish.parent().unwrap()).unwrap();
    fs::write(&fish, added("fish_add_path '/home/me/.local/bin'")).unwrap();
    rig.install_schedule(Platform::Linux, &[]).unwrap();
    let timer = rig.unit_dir().join("spendbranch-sync.timer");
    assert!(timer.exists());

    let out = run(&rig, &["uninstall", "--yes"], "", false);
    assert_eq!(out.code, 0, "{}", out.text);
    assert!(out.text.contains(m::UNINSTALL_REVOKED), "{}", out.text);
    assert_eq!(
        rig.server.revoked_reason("dev-1").as_deref(),
        Some("logout")
    );
    assert!(
        out.text.contains(m::UNINSTALL_SCHEDULE_REMOVED),
        "{}",
        out.text
    );
    assert!(!timer.exists(), "the schedule is gone");

    let left = fs::read_to_string(&settings).unwrap();
    assert!(!left.contains("spendbranch"), "{left}");
    assert!(left.contains("echo bye"), "{left}");
    assert_eq!(fs::read_to_string(&profile).unwrap(), "umask 022\n");
    assert!(!fish.exists(), "the fish file held only our line");
    assert!(!data.exists(), "the data folder is gone");
    assert_eq!(rig.system.removed.borrow().as_slice(), [PathBuf::from(EXE)]);
    assert!(
        out.text.ends_with(&format!("{}\n", m::UNINSTALL_DONE)),
        "{}",
        out.text
    );
    // One line per step, in order, like setup; paths under the home shown with ~.
    let steps: Vec<&str> = out
        .text
        .lines()
        .filter(|l| l.starts_with("[ok]") || l.starts_with("[!]") || l.starts_with("[x]"))
        .collect();
    let names: Vec<&str> = steps
        .iter()
        .map(|l| l.split_whitespace().nth(1).unwrap())
        .collect();
    assert_eq!(
        names,
        ["Disconnect", "Hooks", "Schedule", "PATH", "Data", "Program"],
        "{}",
        out.text
    );
    assert!(steps.iter().all(|l| l.starts_with("[ok]")), "{}", out.text);
    assert!(
        out.text
            .contains(&m::uninstall_hooks_removed("Claude Code")),
        "{}",
        out.text
    );
    // Built with the platform's separator, as the command builds them.
    let shown = |p: &PathBuf| tilde(p.to_str().unwrap(), rig.dir.path().to_str());
    assert!(
        out.text.contains(&m::uninstall_path_files(&format!(
            "{}, {}",
            shown(&profile),
            shown(&fish)
        ))),
        "{}",
        out.text
    );
    assert!(
        out.text.contains(&m::uninstall_deleted(&shown(&data))),
        "{}",
        out.text
    );
}

#[test]
fn the_shell_files_are_joined_with_the_platform_separator() {
    use spendbranch::infra::uninstall::shell_files;
    let home = std::env::temp_dir();
    let home_text = home.to_str().unwrap().to_owned();
    let env = move |k: &str| (k == "HOME").then(|| home_text.clone());
    let fish = home
        .join(".config")
        .join("fish")
        .join("conf.d")
        .join("spendbranch.fish");
    let files = shell_files(&env);
    // Compared as text: `Path` equality ignores which separator was used.
    assert_eq!(
        files.last().and_then(|f| f.to_str()),
        fish.to_str(),
        "{files:?}"
    );
}

#[test]
fn paths_under_the_home_are_shown_with_a_tilde() {
    assert_eq!(tilde("/home/me/.zshrc", Some("/home/me")), "~/.zshrc");
    assert_eq!(tilde(r"C:\Users\me\x", Some(r"C:\Users\me")), r"~\x");
    assert_eq!(
        tilde("/home/meg/.zshrc", Some("/home/me")),
        "/home/meg/.zshrc"
    );
    assert_eq!(tilde("/home/me/.zshrc", Some("")), "/home/me/.zshrc");
    assert_eq!(tilde("/home/me/.zshrc", None), "/home/me/.zshrc");
}

#[test]
fn a_device_that_is_not_connected_is_still_cleaned_and_no_folder_is_created() {
    let rig = Rig::new();
    let (data, _) = home(&rig);
    // A schedule left by an earlier setup is removed even without a device.
    rig.install_schedule(Platform::Linux, &[]).unwrap();
    let out = run(&rig, &["uninstall", "--yes"], "", false);
    assert_eq!(out.code, 0, "{}", out.text);
    assert!(rig.paths_seen().is_empty(), "nothing to revoke, no request");
    assert!(!data.exists(), "uninstall never creates the data folder");
    assert!(
        out.text.contains(m::UNINSTALL_SCHEDULE_REMOVED),
        "{}",
        out.text
    );
    assert!(!rig.unit_dir().join("spendbranch-sync.timer").exists());
    assert_eq!(rig.system.removed.borrow().len(), 1);
}

#[test]
fn without_a_terminal_it_needs_yes_and_removes_nothing() {
    let rig = Rig::new();
    authorize(&rig);
    let (data, _) = home(&rig);
    assert_eq!(run(&rig, &["login"], "y\n", false).code, 0);
    let out = run(&rig, &["uninstall"], "y\n", false);
    assert_eq!((out.code, out.text.trim()), (2, m::UNINSTALL_NEEDS_YES));
    assert!(data.exists());
    assert!(rig.system.removed.borrow().is_empty());
}

#[test]
fn on_a_terminal_it_asks_first_and_anything_but_yes_cancels() {
    let rig = Rig::new();
    let (data, _) = home(&rig);
    fs::create_dir_all(&data).unwrap();
    let out = run(&rig, &["uninstall"], "n\n", true);
    assert_eq!(out.code, 1);
    assert!(out.text.contains("Continue? [y/N]"), "{}", out.text);
    assert!(out.text.contains(m::UNINSTALL_CANCELLED));
    assert!(data.exists());
    assert!(rig.system.removed.borrow().is_empty());

    let out = run(&rig, &["uninstall"], "Y\n", true);
    assert_eq!(out.code, 0, "{}", out.text);
    assert!(!data.exists());
}

#[test]
fn a_data_folder_that_is_not_named_spendbranch_is_left_alone() {
    let rig = Rig::new();
    // The rig's default: SPENDBRANCH_HOME is the test folder itself, the HOME of this test.
    fs::write(rig.dir.path().join("my-notes.txt"), "keep me").unwrap();
    let out = rig.run_with(&["uninstall", "--yes"], "", false, &[]);
    assert_eq!(out.code, 1, "{}", out.text);
    assert!(
        out.text.contains("is not a folder named spendbranch"),
        "{}",
        out.text
    );
    assert!(rig.dir.path().join("my-notes.txt").exists());
    assert!(out.text.ends_with(&format!("{}\n", m::UNINSTALL_PARTIAL)));
}

#[test]
fn a_homebrew_program_is_left_to_brew() {
    let mut rig = Rig::new();
    rig.system.exe = "/opt/homebrew/Cellar/spendbranch/0.2.0/bin/spendbranch".into();
    let out = run(&rig, &["uninstall", "--yes"], "", false);
    assert_eq!(out.code, 1, "{}", out.text);
    assert!(out.text.contains(m::UNINSTALL_HOMEBREW));
    assert!(rig.system.removed.borrow().is_empty());
}

#[test]
fn on_windows_the_program_folder_leaves_the_user_path() {
    let mut rig = Rig::new();
    rig.system.platform = Platform::Windows;
    rig.system.exe = r"C:\Users\me\AppData\Local\Programs\spendbranch\spendbranch.exe".into();
    rig.system.answer("powershell", 0, "removed\n");
    let out = run(&rig, &["uninstall", "--yes"], "", false);
    assert_eq!(out.code, 0, "{}", out.text);
    assert!(
        out.text.contains(&m::uninstall_path_entry(
            r"C:\Users\me\AppData\Local\Programs\spendbranch"
        )),
        "{}",
        out.text
    );
    let calls = rig.system.tool_calls("powershell");
    let encoded = calls.last().unwrap().last().unwrap().clone();
    assert_eq!(
        encoded,
        powershell_encoded(&windows_path_script(
            r"C:\Users\me\AppData\Local\Programs\spendbranch"
        ))
    );
    // Shell startup files are a Unix thing: none is touched on Windows.
    let profile = rig.dir.path().join(".profile");
    assert!(!profile.exists());
}

#[test]
fn on_windows_a_shared_folder_stays_on_the_path() {
    let mut rig = Rig::new();
    rig.system.platform = Platform::Windows;
    rig.system.exe = r"C:\tools\spendbranch.exe".into();
    let out = run(&rig, &["uninstall", "--yes"], "", false);
    assert_eq!(out.code, 0, "{}", out.text);
    assert!(
        rig.system.tool_calls("powershell").is_empty(),
        "C:\\tools is not ours"
    );
}

#[cfg(windows)]
#[test]
fn on_windows_uninstall_with_a_key_outside_the_profile_skips_the_revoke_and_removes_the_rest() {
    let rig = Rig::new();
    authorize(&rig);
    let (data, _) = home(&rig);
    assert_eq!(run(&rig, &["login"], "y\n", false).code, 0);
    let other = tempfile::tempdir().unwrap();
    let (_, text) = home(&rig);
    let out = rig.run_with(
        &["uninstall", "--yes"],
        "",
        false,
        &[
            ("SPENDBRANCH_HOME", &text),
            ("USERPROFILE", other.path().to_str().unwrap()),
        ],
    );
    assert!(out.text.contains(m::UNINSTALL_NOT_REVOKED), "{}", out.text);
    assert!(rig.server.revoked_reason("dev-1").is_none(), "no revoke");
    assert!(
        !data.exists(),
        "the data folder, key file included, is gone"
    );
    assert_eq!(rig.system.removed.borrow().as_slice(), [PathBuf::from(EXE)]);
}
