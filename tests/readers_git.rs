//! The repository resolver: git without a shell, with a timeout, never outside a project.
use spendbranch::readers::git::RepoResolver;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

fn git_exists() -> bool {
    Command::new("git").arg("--version").output().is_ok()
}

fn repo(dir: &Path, origin: Option<&str>) {
    fs::create_dir_all(dir).unwrap();
    let git = |args: &[&str]| {
        assert!(
            Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .unwrap()
                .status
                .success()
        )
    };
    git(&["init", "-q"]);
    if let Some(url) = origin {
        git(&["remote", "add", "origin", url]);
    }
}

#[test]
fn it_reads_origin_and_remembers_the_answer() {
    if !git_exists() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("work/portal");
    repo(&project, Some("git@github.com:Acme/Portal.git"));
    let mut r = RepoResolver::new(Some(tmp.path().join("home")));
    let cwd = project.to_string_lossy().into_owned();
    assert_eq!(r.resolve(&cwd).as_deref(), Some("github.com/acme/portal"));
    fs::remove_dir_all(&project).unwrap();
    assert_eq!(
        r.resolve(&cwd).as_deref(),
        Some("github.com/acme/portal"),
        "cached: git is not asked twice"
    );
}

#[test]
fn a_folder_without_origin_or_without_git_has_no_repository() {
    if !git_exists() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    repo(&tmp.path().join("no-origin"), None);
    fs::create_dir_all(tmp.path().join("plain")).unwrap();
    let mut r = RepoResolver::new(None);
    for dir in ["no-origin", "plain", "does-not-exist"] {
        assert_eq!(
            r.resolve(&tmp.path().join(dir).to_string_lossy()),
            None,
            "{dir}"
        );
    }
}

#[test]
fn the_home_folder_a_filesystem_root_and_relative_paths_are_never_projects() {
    if !git_exists() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    repo(&home, Some("https://github.com/acme/dotfiles.git")); // a repo, but it is the home folder
    let root: PathBuf = tmp.path().ancestors().last().unwrap().to_path_buf();
    let mut r = RepoResolver::new(Some(home.clone()));
    assert_eq!(r.resolve(&home.to_string_lossy()), None);
    assert_eq!(r.resolve(&root.to_string_lossy()), None);
    assert_eq!(r.resolve("some/relative/dir"), None);
    assert_eq!(r.resolve(""), None);
}

#[test]
fn a_missing_git_is_just_no_repository() {
    let tmp = tempfile::tempdir().unwrap();
    let mut r = RepoResolver::with_program(
        "definitely-not-a-program-xyz".into(),
        None,
        Duration::from_secs(1),
    );
    assert_eq!(r.resolve(&tmp.path().to_string_lossy()), None);
}

#[cfg(unix)]
#[test]
fn a_git_that_hangs_is_stopped_after_the_timeout() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let fake = tmp.path().join("slow-git");
    fs::write(&fake, "#!/bin/sh\nsleep 10\n").unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    let mut r = RepoResolver::with_program(fake.into(), None, Duration::from_millis(300));
    let started = Instant::now();
    assert_eq!(r.resolve(&tmp.path().to_string_lossy()), None);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "took {:?}",
        started.elapsed()
    );
}

#[cfg(unix)]
#[test]
fn the_folder_name_is_never_interpreted_by_a_shell() {
    if !git_exists() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let nasty = tmp
        .path()
        .join("repo $(touch pwned); `touch pwned2` && touch pwned3");
    repo(&nasty, Some("https://github.com/acme/portal.git"));
    let mut r = RepoResolver::new(None);
    assert_eq!(
        r.resolve(&nasty.to_string_lossy()).as_deref(),
        Some("github.com/acme/portal")
    );
    for marker in ["pwned", "pwned2", "pwned3"] {
        assert!(
            !nasty.join(marker).exists() && !tmp.path().join(marker).exists(),
            "{marker} was created"
        );
    }
}
