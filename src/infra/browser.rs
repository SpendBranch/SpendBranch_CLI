//! Opens an address in the user's browser. A failure is ignored: the address is printed too.
use std::process::{Command, Stdio};

pub fn open(url: &str) {
    // Only web addresses, and no shell: the address is one argument, never a command line.
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return;
    }
    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    let mut command = Command::new(program);
    command
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let _ = command.spawn();
}
