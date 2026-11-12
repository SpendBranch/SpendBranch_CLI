//! The files and command lines that make the system run `spendbranch sync` every 30 minutes
//! and at login. Pure: text only; `infra::scheduler` writes the files and runs the tools.

/// How often the system runs `sync`, in minutes. Every format below is built from it.
pub const INTERVAL_MIN: u32 = 30;
pub const SYSTEMD_UNIT: &str = "spendbranch-sync";
pub const LAUNCHD_LABEL: &str = "com.spendbranch.sync";
pub const WINDOWS_TASKS: [&str; 2] = ["SpendBranchSync", "SpendBranchSyncLogon"];
/// Marks the crontab lines that are ours, so only they are replaced or removed.
pub const CRON_TAG: &str = "# spendbranch-sync";

/// What to run: the executable, and the data folder when the person chose one.
pub struct Job<'a> {
    pub exe: &'a str,
    pub home: Option<&'a str>,
}

fn posix(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// systemd: inside double quotes, `\` and `"` are escaped, and `%` and `$` are doubled
/// (they are specifiers and variables there).
fn systemd(text: &str) -> String {
    let body = text
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%")
        .replace('$', "$$");
    format!("\"{body}\"")
}

pub fn systemd_service(job: &Job) -> String {
    let mut out = format!(
        "[Unit]\nDescription=SpendBranch usage sync\n\n[Service]\nType=oneshot\nExecStart={} sync\n",
        systemd(job.exe)
    );
    if let Some(home) = job.home {
        out.push_str(&format!(
            "Environment={}\n",
            systemd(&format!("SPENDBRANCH_HOME={home}"))
        ));
    }
    out
}

pub fn systemd_timer() -> String {
    format!(
        "[Unit]\nDescription=Run the SpendBranch sync every {INTERVAL_MIN} minutes and at login\n\n[Timer]\nOnStartupSec=2min\nOnUnitActiveSec={INTERVAL_MIN}min\nRandomizedDelaySec=60\n\n[Install]\nWantedBy=timers.target\n"
    )
}

/// The crontab with our lines replaced (or added) and everything else untouched.
pub fn cron_merge(existing: &str, job: &Job) -> String {
    let mut out = cron_remove(existing);
    let env = job
        .home
        .map(|h| format!("SPENDBRANCH_HOME={} ", posix(h)))
        .unwrap_or_default();
    // `%` ends a command in cron: it must be escaped.
    let run = format!("{env}{} sync >/dev/null 2>&1", posix(job.exe)).replace('%', "\\%");
    for when in [format!("*/{INTERVAL_MIN} * * * *"), "@reboot".to_owned()] {
        out.push_str(&format!("{when} {run} {CRON_TAG}\n"));
    }
    out
}

/// The crontab without our lines.
pub fn cron_remove(existing: &str) -> String {
    existing
        .lines()
        .filter(|l| !l.trim_end().ends_with(CRON_TAG))
        .map(|l| format!("{l}\n"))
        .collect()
}

pub fn has_cron(existing: &str) -> bool {
    existing.lines().any(|l| l.trim_end().ends_with(CRON_TAG))
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub fn launchd_plist(job: &Job) -> String {
    let secs = INTERVAL_MIN * 60;
    let env = job.home.map_or(String::new(), |h| {
        format!(
            "  <key>EnvironmentVariables</key>\n  <dict>\n    <key>SPENDBRANCH_HOME</key>\n    <string>{}</string>\n  </dict>\n",
            xml(h)
        )
    });
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key>\n  <string>{LAUNCHD_LABEL}</string>\n  <key>ProgramArguments</key>\n  <array>\n    <string>{}</string>\n    <string>sync</string>\n  </array>\n  <key>StartInterval</key>\n  <integer>{secs}</integer>\n  <key>RunAtLoad</key>\n  <true/>\n{env}</dict>\n</plist>\n",
        xml(job.exe)
    )
}

/// `schtasks /Create` arguments for the two tasks: every 30 minutes, and at logon.
pub fn schtasks_create(job: &Job) -> [Vec<String>; 2] {
    let run = format!("\"{}\" sync", job.exe);
    let task = |name: &str, when: &[&str]| {
        let mut args: Vec<String> = ["/Create", "/TN", name, "/TR"]
            .into_iter()
            .map(String::from)
            .collect();
        args.push(run.clone());
        args.extend(when.iter().map(|s| s.to_string()));
        args.extend(["/F".to_string()]);
        args
    };
    [
        task(
            WINDOWS_TASKS[0],
            &["/SC", "MINUTE", "/MO", &INTERVAL_MIN.to_string()],
        ),
        task(WINDOWS_TASKS[1], &["/SC", "ONLOGON"]),
    ]
}
