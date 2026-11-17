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
# the last line: a download cut short runs nothing. It never calls `exit`, which would close the
# window it was pasted into.

function Install-SpendBranch {
    [CmdletBinding()]
    param()

    $ErrorActionPreference = 'Stop'
    # The progress bar makes downloads many times slower on Windows PowerShell 5.1.
    $ProgressPreference = 'SilentlyContinue'

    # Written by the release pipeline; empty in the repository copy.
    $releaseVersion = ''
    $repository = 'SpendBranch/SpendBranch_CLI'

    function Write-Step([string] $text) {
        Write-Host "spendbranch-install: $text"
    }

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

    # Https only, or a copy from a local folder (tests, mirrors).
    function Get-File([string] $source, [string] $destination) {
        if ($source -match '^https://') {
            Invoke-WebRequest -UseBasicParsing -Uri $source -OutFile $destination
        } elseif ($source -match '^[A-Za-z][A-Za-z0-9+.-]+://') {
            throw "only https:// downloads are allowed: $source"
        } else {
            Copy-Item -LiteralPath $source -Destination $destination
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

    function Add-UserPath([string] $dir) {
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
            Write-Step "added $dir to your user Path"
        }
        $session = @($env:Path -split ';' | Where-Object { $_.TrimEnd('\') -ieq $dir.TrimEnd('\') })
        if (-not $session) {
            $env:Path = "$dir;$env:Path"
        }
    }

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
    $join = if ($base -match '^https://') { '/' } else { '\' }
    $dir = if ($env:SPENDBRANCH_INSTALL_DIR) { $env:SPENDBRANCH_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\spendbranch' }
    $exe = Join-Path $dir 'spendbranch.exe'

    $tmp = Join-Path ([IO.Path]::GetTempPath()) ('spendbranch-' + [IO.Path]::GetRandomFileName())
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        Write-Step "downloading SpendBranch CLI $version for $target"
        Get-File "$base$join$archive" (Join-Path $tmp $archive)
        Get-File "$base${join}SHA256SUMS" (Join-Path $tmp 'SHA256SUMS')

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
        Write-Step 'checksum verified'

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
        # A running spendbranch.exe cannot be overwritten, but it can be renamed: move it aside,
        # put the new one in its place, and delete the old one when nothing uses it any more.
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
        Write-Step "installed SpendBranch CLI $installed in $dir"
    } finally {
        Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }

    if ($env:SPENDBRANCH_NO_MODIFY_PATH -eq '1') {
        Write-Step "SPENDBRANCH_NO_MODIFY_PATH is set: add $dir to your Path yourself"
    } else {
        Add-UserPath $dir
    }
    $found = Get-Command spendbranch -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($found -and ($found.Source -ine $exe)) {
        Write-Step "warning: another spendbranch comes first on your Path: $($found.Source)"
    }

    if ($env:SPENDBRANCH_NO_SETUP -eq '1') {
        Write-Step "next: run 'spendbranch setup' to connect this device"
    } elseif (-not [Environment]::UserInteractive -or [Console]::IsInputRedirected) {
        Write-Step "next: run 'spendbranch setup' in a terminal to connect this device"
    } else {
        Write-Step 'connecting this device: spendbranch setup'
        & $exe setup
        if ($LASTEXITCODE -ne 0) {
            Write-Step 'the CLI is installed; finish connecting with: spendbranch setup'
            $global:LASTEXITCODE = $LASTEXITCODE
            return
        }
    }
    Write-Step "open a new terminal to use 'spendbranch' everywhere"
}

Install-SpendBranch
