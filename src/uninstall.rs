//! What `uninstall` removes besides the hooks and the schedule: the PATH lines the install
//! script added, the data folder and the program. Pure: text only; `infra::uninstall` reads and
//! writes the files, the `System` removes the program.

/// The comment install.sh writes above each PATH line it adds (tests/uninstall.rs checks that
/// install.sh still writes exactly this).
pub const PATH_MARK: &str = "# added by the SpendBranch CLI installer";

/// `text` without the lines the install script added: each `PATH_MARK` line, the line after it
/// when it is a PATH line, and the empty line the script put before them. `None` when there is
/// nothing of ours.
pub fn without_path_lines(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut kept: Vec<&str> = Vec::with_capacity(lines.len());
    let mut changed = false;
    let mut i = 0;
    while let Some(line) = lines.get(i) {
        if line.trim_end_matches(['\n', '\r']) != PATH_MARK {
            kept.push(line);
            i += 1;
            continue;
        }
        changed = true;
        if kept.last().is_some_and(|l| l.trim().is_empty()) {
            kept.pop();
        }
        i += 1;
        let next = lines.get(i).map(|l| l.trim_start());
        if next.is_some_and(|l| l.starts_with("export PATH=") || l.starts_with("fish_add_path ")) {
            i += 1;
        }
    }
    changed.then(|| kept.concat())
}

/// A folder is ours by name only: the data folder is deleted whole, and the program's folder
/// taken off the Windows Path, only when it is called `spendbranch`. A
/// `SPENDBRANCH_HOME` pointed at a home or a project is never emptied; a shared tools folder stays
/// on the Path.
pub fn folder_is_ours(name: Option<&str>) -> bool {
    name.is_some_and(|n| n.eq_ignore_ascii_case("spendbranch"))
}

/// The folder of a Windows program path, when it is ours (`folder_is_ours`): the one to take
/// off the user Path. Text, so it reads the same on any system.
pub fn windows_folder_of(exe: &str) -> Option<&str> {
    let (dir, _) = exe.rsplit_once(['\\', '/'])?;
    let name = dir.rsplit(['\\', '/']).next();
    folder_is_ours(name).then_some(dir)
}

/// The program was installed by Homebrew: `brew uninstall` removes it, not us.
pub fn installed_by_homebrew(exe: &str) -> bool {
    crate::scheduler::stable_exe(exe) != exe
}

/// A path as the person knows it: `~` for their home folder.
pub fn tilde(path: &str, home: Option<&str>) -> String {
    match home.filter(|h| !h.is_empty()) {
        Some(home) => match path.strip_prefix(home) {
            Some(rest) if rest.starts_with(['/', '\\']) => format!("~{rest}"),
            _ => path.to_owned(),
        },
        None => path.to_owned(),
    }
}

/// A PowerShell literal: single quotes, with each `'` doubled.
fn ps_literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// The script that takes `dir` out of the user Path, keeping the value expandable and the other
/// entries as written (`%VARIABLES%` unexpanded). Prints `removed` when it took something out.
pub fn windows_path_script(dir: &str) -> String {
    format!(
        "$d = {dir}.TrimEnd('\\')\n\
         $k = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)\n\
         try {{\n\
         $all = @(([string] $k.GetValue('Path', '', 'DoNotExpandEnvironmentNames')) -split ';' | Where-Object {{ $_ -ne '' }})\n\
         $kept = @($all | Where-Object {{ [Environment]::ExpandEnvironmentVariables($_).TrimEnd('\\') -ine $d }})\n\
         if ($kept.Count -ne $all.Count) {{\n\
         $k.SetValue('Path', ($kept -join ';'), 'ExpandString')\n\
         [Environment]::SetEnvironmentVariable('SPENDBRANCH_INSTALLER', '1', 'User')\n\
         [Environment]::SetEnvironmentVariable('SPENDBRANCH_INSTALLER', $null, 'User')\n\
         'removed'\n\
         }}\n\
         }} finally {{ $k.Close() }}\n",
        dir = ps_literal(dir)
    )
}

/// The script that deletes the program once process `pid` (this one) has exited: a running
/// .exe cannot be deleted. Then its folder, only if that is now empty.
pub fn windows_delete_script(pid: u32, exe: &str, dir: &str) -> String {
    format!(
        "Wait-Process -Id {pid} -ErrorAction SilentlyContinue\n\
         Remove-Item -LiteralPath {exe} -Force -ErrorAction SilentlyContinue\n\
         Remove-Item -LiteralPath {old} -Force -ErrorAction SilentlyContinue\n\
         if (-not (Get-ChildItem -LiteralPath {dir} -Force -ErrorAction SilentlyContinue)) {{ Remove-Item -LiteralPath {dir} -Force -ErrorAction SilentlyContinue }}\n",
        exe = ps_literal(exe),
        old = ps_literal(&format!("{exe}.old")),
        dir = ps_literal(dir),
    )
}

/// `powershell -EncodedCommand` takes the script as base64 of UTF-16LE: no quoting of paths
/// through the command line.
pub fn powershell_encoded(script: &str) -> String {
    use base64::Engine;
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}
