# Install and uninstall the SpendBranch CLI

The CLI is one file. Install it once per machine, outside any project, then run `spendbranch setup`.
It reads only what your company authorized from Claude Code and Codex sessions; prompts, responses,
code and local paths never leave your computer.

## 1. Download

Get the archive for your system from the [latest release](https://github.com/SpendBranch/SpendBranch_CLI/releases/latest),
together with `SHA256SUMS`:

| System | Archive |
| --- | --- |
| Linux (Intel/AMD) | `spendbranch-<version>-linux-x64.tar.gz` |
| Linux (ARM) | `spendbranch-<version>-linux-arm64.tar.gz` |
| macOS (Apple silicon) | `spendbranch-<version>-macos-arm64.tar.gz` |
| macOS (Intel) | `spendbranch-<version>-macos-x64.tar.gz` |
| Windows (Intel/AMD) | `spendbranch-<version>-windows-x64.zip` |
| Windows (ARM) | `spendbranch-<version>-windows-arm64.zip` |

## 2. Check the download

The sum printed must match the line for your archive in `SHA256SUMS`.

```sh
sha256sum -c SHA256SUMS --ignore-missing          # Linux
shasum -a 256 -c SHA256SUMS --ignore-missing      # macOS
```

```powershell
Get-FileHash .\spendbranch-<version>-windows-x64.zip -Algorithm SHA256   # Windows
```

## 3. Install

**Linux and macOS**: into `~/.local/bin`.

```sh
tar -xzf spendbranch-<version>-<system>.tar.gz
mkdir -p ~/.local/bin
mv spendbranch-<version>-<system>/spendbranch ~/.local/bin/
spendbranch --version
```

If `spendbranch` is not found, add `~/.local/bin` to your `PATH` (for example, add
`export PATH="$HOME/.local/bin:$PATH"` to `~/.bashrc` or `~/.zshrc`) and open a new terminal.

**Windows** (PowerShell): into `%LOCALAPPDATA%\Programs\spendbranch`.

```powershell
Expand-Archive .\spendbranch-<version>-windows-x64.zip -DestinationPath $env:TEMP\spendbranch
$dir = "$env:LOCALAPPDATA\Programs\spendbranch"
New-Item -ItemType Directory -Force $dir | Out-Null
Move-Item -Force "$env:TEMP\spendbranch\spendbranch-<version>-windows-x64\spendbranch.exe" $dir
[Environment]::SetEnvironmentVariable('Path', "$([Environment]::GetEnvironmentVariable('Path','User'));$dir", 'User')
```

Open a new terminal and run `spendbranch --version`.

If a release note says a build is **unsigned**, your system may warn the first time it runs: on
macOS, run `xattr -d com.apple.quarantine ~/.local/bin/spendbranch` once; on Windows, choose
"More info" then "Run anyway" in SmartScreen. Signed builds need neither.

## 4. Connect

```sh
spendbranch setup
```

`setup` connects this device to your organization (you approve a code in the browser), finds
Claude Code and Codex, turns on reading, installs their session-end hooks and schedules a sync every
30 minutes and at login. It is safe to run again. If your company gave you a server address, pass
it: `spendbranch setup --server https://...`.

Then, any time:

```sh
spendbranch status    # what is connected, what is waiting, and what to fix
```

Codex runs a new hook only after you trust it: open Codex and review it under `/hooks`.

## Uninstall

1. Disconnect the device. This revokes it on the server, deletes its key and removes the scheduled
   sync:

   ```sh
   spendbranch logout
   ```

2. Remove the hooks from the agents' settings (other hooks stay untouched):

   ```sh
   spendbranch hooks uninstall claude-code
   spendbranch hooks uninstall codex
   ```

3. Delete the program and its data folder:

   | System | Program | Data |
   | --- | --- | --- |
   | Linux | `~/.local/bin/spendbranch` | `~/.config/spendbranch` (or `$XDG_CONFIG_HOME/spendbranch`) |
   | macOS | `~/.local/bin/spendbranch` | `~/.config/spendbranch` (or `$XDG_CONFIG_HOME/spendbranch`) |
   | Windows | `%LOCALAPPDATA%\Programs\spendbranch\spendbranch.exe` | `%APPDATA%\spendbranch` |

   If you set `SPENDBRANCH_HOME`, the data is there instead. On Windows, also take the folder out of
   your user `Path`.
