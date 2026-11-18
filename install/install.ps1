# Installs the SpendBranch CLI on Windows, then runs `spendbranch setup`:
#
#   irm https://github.com/SpendBranch/SpendBranch_CLI/releases/latest/download/install.ps1 | iex
#
# What it does, in order: picks the archive for this PC (x64 or ARM64), downloads it and
# SHA256SUMS from the same GitHub release, checks the SHA-256 (nothing is installed if it
# differs), installs spendbranch.exe into %LOCALAPPDATA%\Programs\spendbranch (no admin), adds
# that folder to your user Path, and, in an interactive window, runs `spendbranch setup` to
# connect this device.
#
# Optional settings (environment variables, e.g. $env:SPENDBRANCH_NO_SETUP = '1'):
#   SPENDBRANCH_VERSION=1.2.3         install this version instead of the one this script is for
#   SPENDBRANCH_INSTALL_DIR=<dir>     install there instead of %LOCALAPPDATA%\Programs\spendbranch
#   SPENDBRANCH_NO_MODIFY_PATH=1      do not change the user Path
#   SPENDBRANCH_NO_SETUP=1            do not run `spendbranch setup` at the end
#   SPENDBRANCH_DOWNLOAD_URL=<url>    download from this https:// address or local folder instead
#   SPENDBRANCH_SERVER=https://...    not read here: `spendbranch setup` uses it
#
# Works on Windows PowerShell 5.1 and PowerShell 7. Everything runs inside a function called on
# the last line: a download cut short runs nothing. It never calls `exit` when pasted (that would
# close the window); run as a file, it exits with 1 on a failure. The file is plain ASCII on
# purpose: `irm` may read it as Latin-1, so every symbol on the screen is built from its code.

function Install-SpendBranch {
    [CmdletBinding()]
    param()

    $ErrorActionPreference = 'Stop'
    # The progress bar makes downloads many times slower on Windows PowerShell 5.1.
    $ProgressPreference = 'SilentlyContinue'

    # Written by the release pipeline; empty in the repository copy.
    $releaseVersion = ''
    $repository = 'SpendBranch/SpendBranch_CLI'

    # --- the screen: the same marks and colors as the CLI --------------------------------
    # Off a terminal: plain lines with [ok] [!] [x]. In a window: the line between steps and a
    # spinner while downloading. Symbols and color only where the terminal says it shows
    # truecolor (Windows Terminal, COLORTERM), never with NO_COLOR or TERM=dumb.
    $ui = @{ Tty = -not [Console]::IsOutputRedirected; Spinning = $false }
    $ui.Fancy = $ui.Tty -and -not $env:NO_COLOR -and $env:TERM -ne 'dumb' -and
        ($env:WT_SESSION -or $env:COLORTERM -eq 'truecolor' -or $env:COLORTERM -eq '24bit')
    $e = [char]27
    if ($ui.Fancy) {
        $ui.Ok = "$e[32m$([char]0x2713)$e[0m"
        $ui.Warn = "$e[33m!$e[0m"
        $ui.Bad = "$e[31m$([char]0x2717)$e[0m"
        $ui.Bold = "$e[1m"
        $ui.Amber = "$e[1;38;2;240;176;32m"
        $ui.Line = "$e[38;2;156;112;68m"
        $ui.Dim = "$e[2m"
        $ui.Reset = "$e[0m"
        $ui.Frames = @(0x280B, 0x2819, 0x2839, 0x2838, 0x283C, 0x2834, 0x2826, 0x2827, 0x2807, 0x280F) |
            ForEach-Object { [string][char]$_ }
    } else {
        $ui.Ok = '[ok]'; $ui.Warn = '[!]'; $ui.Bad = '[x]'
        $ui.Bold = ''; $ui.Amber = ''; $ui.Line = ''; $ui.Dim = ''; $ui.Reset = ''
        $ui.Frames = @('|', '/', '-', '\')
    }
    $ui.Joint = [string][char]0x2502

    # How a step ended: its mark, its name and one line of detail.
    function Write-Step([string] $mark, [string] $name, [string] $detail) {
        Write-Host ("{0} {1}{2,-11}{3} {4}" -f $mark, $ui.Bold, $name, $ui.Reset, $detail)
    }

    # The line that joins two steps, in a window.
    function Write-Joint {
        if ($ui.Tty) { Write-Host "$($ui.Line)$($ui.Joint)$($ui.Reset)" }
    }

    # A command to type: amber in color, in backticks without.
    function Format-Command([string] $text) {
        if ($ui.Fancy) { return "$($ui.Amber)$text$($ui.Reset)" }
        return '`' + $text + '`'
    }

    # A path as the person knows it: ~ for their profile folder.
    function Format-Path([string] $path) {
        $profileDir = $env:USERPROFILE
        if ($profileDir -and $path.StartsWith($profileDir + '\', [StringComparison]::OrdinalIgnoreCase)) {
            return '~' + $path.Substring($profileDir.Length)
        }
        return $path
    }

    function Stop-Spinner {
        if ($ui.Spinning) {
            $width = 79
            try { $width = [Console]::WindowWidth - 1 } catch { $width = 79 }
            Write-Host ("`r" + (' ' * $width) + "`r") -NoNewline
            try { [Console]::CursorVisible = $true } catch { $null = $_ }
            $ui.Spinning = $false
        }
    }

    # An animated mark while the task runs.
    function Wait-Task($task, [string] $name, [string] $detail) {
        if ($ui.Tty) {
            $ui.Spinning = $true
            try { [Console]::CursorVisible = $false } catch { $null = $_ }
            $i = 0
            while (-not $task.IsCompleted) {
                $frame = $ui.Frames[$i % $ui.Frames.Count]
                Write-Host ("`r{0}{1}{2} {3}{4,-11}{5} {6}{7}{8}" -f $ui.Amber, $frame, $ui.Reset,
                    $ui.Bold, $name, $ui.Reset, $ui.Dim, $detail, $ui.Reset) -NoNewline
                Start-Sleep -Milliseconds 100
                $i++
            }
            Stop-Spinner
        } else {
            while (-not $task.IsCompleted) { Start-Sleep -Milliseconds 50 }
        }
    }

    # --- the work ------------------------------------------------------------------------

    # The CPU of the PC itself: the variable of this process says AMD64 when an x64 PowerShell
    # runs emulated on ARM64, the system's own value does not.
    function Get-Target {
        $arch = $null
        try {
            $arch = (Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager\Environment' -Name PROCESSOR_ARCHITECTURE).PROCESSOR_ARCHITECTURE
        } catch {
            $arch = $env:PROCESSOR_ARCHITECTURE
        }
        switch ($arch) {
            'AMD64' { return 'windows-x64' }
            'ARM64' { return 'windows-arm64' }
            default { throw "this processor ($arch) has no SpendBranch CLI build" }
        }
    }

    # The archive and SHA256SUMS, side by side: https only, or copies from a local folder
    # (tests, mirrors). The spinner turns while they download.
    function Get-Release([string] $base, [string] $archive, [string] $into) {
        $files = @($archive, 'SHA256SUMS')
        if ($base -match '^https://') {
            $tasks = foreach ($name in $files) {
                (New-Object Net.WebClient).DownloadFileTaskAsync("$base/$name", (Join-Path $into $name))
            }
            $all = [Threading.Tasks.Task]::WhenAll([Threading.Tasks.Task[]] $tasks)
            Wait-Task $all 'Download' $archive
            if ($all.IsFaulted) {
                $why = $all.Exception.GetBaseException().Message
                throw "download failed: $base/$archive ($why)"
            }
        } elseif ($base -match '^[A-Za-z][A-Za-z0-9+.-]+://') {
            throw "download failed: only https:// downloads are allowed: $base/$archive"
        } else {
            foreach ($name in $files) {
                Copy-Item -LiteralPath (Join-Path $base $name) -Destination (Join-Path $into $name)
            }
        }
    }

    # The user Path as stored (REG_EXPAND_SZ with %VARIABLES% kept), not expanded.
    function Get-UserPath {
        $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment')
        try {
            return [string] $key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        } finally {
            $key.Close()
        }
    }

    # Adds the folder to the user Path; says whether it was added.
    function Add-UserPath([string] $dir) {
        $added = $false
        $current = Get-UserPath
        $entries = @($current -split ';' | Where-Object { $_ -ne '' })
        $present = $entries | Where-Object {
            [Environment]::ExpandEnvironmentVariables($_).TrimEnd('\') -ieq $dir.TrimEnd('\')
        }
        if (-not $present) {
            $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
            try {
                $key.SetValue('Path', (@($entries) + $dir) -join ';', [Microsoft.Win32.RegistryValueKind]::ExpandString)
            } finally {
                $key.Close()
            }
            # Setting a user variable through .NET tells open programs (Explorer, new terminals)
            # that the environment changed; this one is set and removed only for that.
            [Environment]::SetEnvironmentVariable('SPENDBRANCH_INSTALLER', '1', 'User')
            [Environment]::SetEnvironmentVariable('SPENDBRANCH_INSTALLER', $null, 'User')
            $added = $true
        }
        $session = @($env:Path -split ';' | Where-Object { $_.TrimEnd('\') -ieq $dir.TrimEnd('\') })
        if (-not $session) {
            $env:Path = "$dir;$env:Path"
        }
        return $added
    }

    try {
        $version = if ($env:SPENDBRANCH_VERSION) { $env:SPENDBRANCH_VERSION } else { $releaseVersion }
        if (-not $version) {
            throw 'no version: use the install.ps1 of a release, or set SPENDBRANCH_VERSION'
        }
        $version = $version -replace '^v', ''
        if ($version -notmatch '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$') {
            throw "not a version: $version"
        }

        # Windows PowerShell 5.1 does not offer TLS 1.2 on its own on every system.
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

        $target = Get-Target
        $archive = "spendbranch-$version-$target.zip"
        $base = if ($env:SPENDBRANCH_DOWNLOAD_URL) { $env:SPENDBRANCH_DOWNLOAD_URL } else { "https://github.com/$repository/releases/download/v$version" }
        $base = $base.TrimEnd('/', '\')
        $dir = if ($env:SPENDBRANCH_INSTALL_DIR) { $env:SPENDBRANCH_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\spendbranch' }
        $exe = Join-Path $dir 'spendbranch.exe'

        Write-Host ("{0}SpendBranch CLI {1}{2} {3}{4} {5}{6}" -f $ui.Amber, $version, $ui.Reset,
            $ui.Dim, [char]0x00B7, $target, $ui.Reset)
        Write-Host ''

        $tmp = Join-Path ([IO.Path]::GetTempPath()) ('spendbranch-' + [IO.Path]::GetRandomFileName())
        New-Item -ItemType Directory -Path $tmp | Out-Null
        try {
            Get-Release $base $archive $tmp
            $size = '{0:N1} MB' -f ((Get-Item -LiteralPath (Join-Path $tmp $archive)).Length / 1MB)
            Write-Step $ui.Ok 'Download' "$archive $([char]0x00B7) $size"
            Write-Joint

            $want = $null
            foreach ($line in Get-Content -LiteralPath (Join-Path $tmp 'SHA256SUMS')) {
                $parts = $line -split '\s+', 2
                if ($parts.Count -eq 2 -and ($parts[1] -eq $archive -or $parts[1] -eq "*$archive")) {
                    $want = $parts[0]
                    break
                }
            }
            if (-not $want) {
                throw "SHA256SUMS has no line for $archive; nothing was installed"
            }
            $got = (Get-FileHash -LiteralPath (Join-Path $tmp $archive) -Algorithm SHA256).Hash
            if ($got -ine $want) {
                throw "checksum mismatch for $archive (expected $want, got $got); nothing was installed"
            }
            Write-Step $ui.Ok 'Verify' 'checksum verified (SHA-256)'
            Write-Joint

            $unpacked = Join-Path $tmp 'unpacked'
            Expand-Archive -LiteralPath (Join-Path $tmp $archive) -DestinationPath $unpacked
            $new = Join-Path $unpacked 'spendbranch.exe'
            if (-not (Test-Path -LiteralPath $new -PathType Leaf)) {
                throw "$archive does not hold spendbranch.exe; nothing was installed"
            }
            & $new --version | Out-Null
            if ($LASTEXITCODE -ne 0) {
                throw 'the downloaded program does not run on this PC; nothing was installed'
            }

            New-Item -ItemType Directory -Force -Path $dir | Out-Null
            # A running spendbranch.exe cannot be overwritten, but it can be renamed: move it
            # aside, put the new one in its place, and delete the old one when nothing uses it.
            $old = "$exe.old"
            if (Test-Path -LiteralPath $old) {
                Remove-Item -LiteralPath $old -Force -ErrorAction SilentlyContinue
            }
            if (Test-Path -LiteralPath $exe) {
                Move-Item -LiteralPath $exe -Destination $old -Force
            }
            try {
                Move-Item -LiteralPath $new -Destination $exe
            } catch {
                if (Test-Path -LiteralPath $old) {
                    Move-Item -LiteralPath $old -Destination $exe -Force
                }
                throw "cannot write $exe; nothing was changed ($_)"
            }
            if (Test-Path -LiteralPath $old) {
                Remove-Item -LiteralPath $old -Force -ErrorAction SilentlyContinue
            }
            $installed = & $exe --version
            Write-Step $ui.Ok 'Install' "$(Format-Path $exe) $([char]0x00B7) $installed"
            Write-Joint
        } finally {
            Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
        }

        if ($env:SPENDBRANCH_NO_MODIFY_PATH -eq '1') {
            Write-Step $ui.Warn 'Path' "not changed (SPENDBRANCH_NO_MODIFY_PATH): add $(Format-Path $dir) yourself"
        } elseif (Add-UserPath $dir) {
            Write-Step $ui.Ok 'Path' "added $(Format-Path $dir) to your user Path"
        } else {
            Write-Step $ui.Ok 'Path' "$(Format-Path $dir) is already on your user Path"
        }
        $found = Get-Command spendbranch -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($found -and ($found.Source -ine $exe)) {
            Write-Step $ui.Warn 'Path' "another spendbranch comes first: $($found.Source)"
        }
    } catch {
        Stop-Spinner
        Write-Host "$($ui.Bad) $($_.Exception.Message)"
        $global:LASTEXITCODE = 1
        return
    }

    if ($env:SPENDBRANCH_NO_SETUP -eq '1') {
        Write-Host ''
        Write-Host "Next: run $(Format-Command 'spendbranch setup') to connect this device."
    } elseif (-not [Environment]::UserInteractive -or [Console]::IsInputRedirected) {
        Write-Host ''
        Write-Host "Next: run $(Format-Command 'spendbranch setup') in a terminal to connect this device."
    } else {
        Write-Joint
        & $exe setup
        if ($LASTEXITCODE -ne 0) {
            $code = $LASTEXITCODE
            Write-Host ''
            Write-Host "$($ui.Warn) The CLI is installed; finish connecting with $(Format-Command 'spendbranch setup')."
            $global:LASTEXITCODE = $code
            return
        }
    }
    $global:LASTEXITCODE = 0
}

Install-SpendBranch
# Run as a file (powershell -File install.ps1), a failure is the exit code. Pasted into a window
# (irm | iex) there is no file, and `exit` would close that window.
if ($PSCommandPath -and $LASTEXITCODE) { exit $LASTEXITCODE }
