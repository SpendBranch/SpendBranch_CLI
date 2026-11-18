# Install and uninstall the SpendBranch CLI

The CLI is one program. Install it once per computer, outside any project. It reads only what your
company authorized from Claude Code and Codex sessions; prompts, responses, code and local paths
never leave your computer.

## Install: one command

**macOS and Linux** (Terminal):

```sh
curl -fsSL https://github.com/SpendBranch/SpendBranch_CLI/releases/latest/download/install.sh | sh
```

**Windows** (PowerShell):

```powershell
irm https://github.com/SpendBranch/SpendBranch_CLI/releases/latest/download/install.ps1 | iex
```

That is all. The script:

1. picks the build for your system and processor;
2. downloads it and `SHA256SUMS` from the same GitHub release and checks the SHA-256: if it does
   not match, nothing is installed;
3. installs the program for your user only, without sudo or admin: `~/.local/bin/spendbranch`, or
   `%LOCALAPPDATA%\Programs\spendbranch\spendbranch.exe` on Windows;
4. adds that folder to your `PATH` (your shell's startup files on macOS and Linux, your user
   `Path` on Windows);
5. runs `spendbranch setup`, which connects this computer to your organization: you approve a code
   in the browser.

If your company gave you a server address, set it first:

```sh
curl -fsSL https://github.com/SpendBranch/SpendBranch_CLI/releases/latest/download/install.sh | SPENDBRANCH_SERVER=https://... sh
```

```powershell
$env:SPENDBRANCH_SERVER = 'https://...'; irm https://github.com/SpendBranch/SpendBranch_CLI/releases/latest/download/install.ps1 | iex
```

**To update**, run the same command again. It replaces the program in place, so the hooks and the
scheduled sync keep working.

### Options

Set these before the command (`VAR=value sh` on macOS/Linux, `$env:VAR = 'value'` on Windows):

| Variable | Effect |
| --- | --- |
| `SPENDBRANCH_VERSION=1.2.3` | install that version instead of the latest |
| `SPENDBRANCH_INSTALL_DIR=<folder>` | install into another folder |
| `SPENDBRANCH_NO_MODIFY_PATH=1` | do not change startup files or the user `Path` |
| `SPENDBRANCH_NO_SETUP=1` | install only; run `spendbranch setup` yourself later |

### Read the script before running it

The scripts are short and public: [install.sh](../install/install.sh) and
[install.ps1](../install/install.ps1). To read the exact copy you will run:

```sh
curl -fsSLO https://github.com/SpendBranch/SpendBranch_CLI/releases/latest/download/install.sh
less install.sh
sh install.sh
```

```powershell
irm https://github.com/SpendBranch/SpendBranch_CLI/releases/latest/download/install.ps1 -OutFile install.ps1
notepad install.ps1
powershell -ExecutionPolicy Bypass -File .\install.ps1
```

## Check what you downloaded

Every release publishes `SHA256SUMS` and a **build provenance attestation** for each file. The
script checks the SHA-256 for you. The checksum proves the file arrived intact, but it comes from
the same release, so it cannot prove the release itself is genuine. The attestation can: it is
signed by GitHub and says which workflow, repository and commit built the file. To check it
yourself, with the [GitHub CLI](https://cli.github.com):

```sh
gh attestation verify spendbranch-<version>-<system>.tar.gz -R SpendBranch/SpendBranch_CLI
gh attestation verify install.sh -R SpendBranch/SpendBranch_CLI
```

## Install by hand

Get the archive for your system from the [latest release](https://github.com/SpendBranch/SpendBranch_CLI/releases/latest),
together with `SHA256SUMS`:

| System | Archive |
| --- | --- |
| Linux (Intel/AMD), any distribution | `spendbranch-<version>-linux-x64.tar.gz` |
| Linux (ARM), any distribution | `spendbranch-<version>-linux-arm64.tar.gz` |
| macOS (Apple silicon) | `spendbranch-<version>-macos-arm64.tar.gz` |
| macOS (Intel) | `spendbranch-<version>-macos-x64.tar.gz` |
| Windows (Intel/AMD) | `spendbranch-<version>-windows-x64.zip` |
| Windows (ARM) | `spendbranch-<version>-windows-arm64.zip` |

Check it: the sum printed must match the line for your archive in `SHA256SUMS`.

```sh
sha256sum -c SHA256SUMS --ignore-missing          # Linux
shasum -a 256 -c SHA256SUMS --ignore-missing      # macOS
```

```powershell
Get-FileHash .\spendbranch-<version>-windows-x64.zip -Algorithm SHA256   # Windows
```

**Linux and macOS**: into `~/.local/bin`.

```sh
tar -xzf spendbranch-<version>-<system>.tar.gz
mkdir -p ~/.local/bin
mv spendbranch-<version>-<system>/spendbranch ~/.local/bin/
spendbranch --version
```

If `spendbranch` is not found, add `~/.local/bin` to your `PATH` (for example, add
`export PATH="$HOME/.local/bin:$PATH"` to `~/.bashrc` or `~/.zshrc`) and open a new terminal.

**Windows** (PowerShell): into `%LOCALAPPDATA%\Programs\spendbranch`. The zip holds
`spendbranch.exe` at its root.

```powershell
$dir = "$env:LOCALAPPDATA\Programs\spendbranch"
Expand-Archive .\spendbranch-<version>-windows-x64.zip -DestinationPath $dir -Force
[Environment]::SetEnvironmentVariable('Path', "$([Environment]::GetEnvironmentVariable('Path','User'));$dir", 'User')
```

Open a new terminal, run `spendbranch --version`, then `spendbranch setup`.

If a release note says a build is **unsigned**, a program downloaded with the browser may be
stopped the first time it runs: on macOS, run `xattr -d com.apple.quarantine ~/.local/bin/spendbranch`
once; on Windows, choose "More info" then "Run anyway" in SmartScreen. The install script is not
affected.

## After installing

`setup` connects this device to your organization, finds Claude Code and Codex, turns on reading,
installs their session-end hooks and schedules a sync every 30 minutes and at login. It is safe to
run again.

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

3. Remove the program:

   | System | Remove |
   | --- | --- |
   | macOS, Linux | delete `~/.local/bin/spendbranch`, and the line marked `# added by the SpendBranch CLI installer` from `~/.profile`, `~/.bashrc`, `~/.bash_profile`, `~/.zshrc` (and `~/.config/fish/conf.d/spendbranch.fish`) |
   | Windows | delete `%LOCALAPPDATA%\Programs\spendbranch`, and take that folder out of your user `Path` |

4. Delete the data folder: `~/.config/spendbranch` (or `$XDG_CONFIG_HOME/spendbranch`) on macOS and
   Linux, `%APPDATA%\spendbranch` on Windows. If you set `SPENDBRANCH_HOME`, the data is there
   instead.
