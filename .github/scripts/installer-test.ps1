# End-to-end test of install/install.ps1: installer-test.ps1 -Package <folder>
# <folder> holds one Windows zip made by package.sh (version 0.0.0-test). The test installs it
# from a copy of that folder into throwaway folders, running the script the way `irm | iex`
# does. The user Path in the registry is changed by the install and put back exactly as it was
# at the end. Run by the CI on Windows.
param(
    [Parameter(Mandatory)] [string] $Package
)
$ErrorActionPreference = 'Stop'
$script = (Resolve-Path (Join-Path $PSScriptRoot '..\..\install\install.ps1')).Path
$version = '0.0.0-test'
$work = Join-Path ([IO.Path]::GetTempPath()) ('sb-installer-test-' + [IO.Path]::GetRandomFileName())
New-Item -ItemType Directory -Path $work | Out-Null

$failures = 0
function Test-Check([string] $name, [scriptblock] $test) {
    $ok = $false
    try { $ok = [bool] (& $test) } catch { Write-Host "  ($_)" }
    if ($ok) { Write-Host "ok   - $name" } else { Write-Host "FAIL - $name"; $script:failures++ }
}

# Runs install.ps1 in a new PowerShell as `irm ... | iex` would, with these variables set.
# Returns the exit code; the output goes to $work\out.txt.
function Invoke-Installer([hashtable] $vars) {
    $all = @{
        SPENDBRANCH_DOWNLOAD_URL = $dist
        SPENDBRANCH_VERSION = $version
        SPENDBRANCH_NO_SETUP = '1'
    }
    foreach ($k in $vars.Keys) { $all[$k] = $vars[$k] }
    $saved = @{}
    foreach ($k in $all.Keys) {
        $saved[$k] = [Environment]::GetEnvironmentVariable($k)
        [Environment]::SetEnvironmentVariable($k, $all[$k])
    }
    # Windows PowerShell 5.1 turns a child's stderr into errors; only its exit code counts here.
    $ErrorActionPreference = 'Continue'
    try {
        $command = "Get-Content -Raw -LiteralPath '$script' | Invoke-Expression"
        powershell -NoProfile -NonInteractive -Command $command *> (Join-Path $work 'out.txt')
        return $LASTEXITCODE
    } finally {
        foreach ($k in $saved.Keys) { [Environment]::SetEnvironmentVariable($k, $saved[$k]) }
    }
}

function Get-RegistryPath {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment')
    try {
        $value = $key.GetValue('Path', $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        $kind = if ($null -ne $value) { $key.GetValueKind('Path') } else { $null }
        return @{ Value = $value; Kind = $kind }
    } finally { $key.Close() }
}

function Restore-RegistryPath($saved) {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
    try {
        if ($null -eq $saved.Value) { $key.DeleteValue('Path', $false) }
        else { $key.SetValue('Path', $saved.Value, $saved.Kind) }
    } finally { $key.Close() }
}

$original = Get-RegistryPath
try {
    $dist = Join-Path $work 'dist'
    New-Item -ItemType Directory -Path $dist | Out-Null
    Copy-Item -Path (Join-Path $Package "spendbranch-$version-windows-*.zip") -Destination $dist
    $zip = Get-ChildItem -LiteralPath $dist -Filter '*.zip' | Select-Object -First 1
    if (-not $zip) { throw "no spendbranch-$version-windows-*.zip in $Package" }
    $sum = (Get-FileHash -LiteralPath $zip.FullName -Algorithm SHA256).Hash.ToLower()
    [IO.File]::WriteAllText((Join-Path $dist 'SHA256SUMS'), "$sum  $($zip.Name)`n")

    # A Path entry with a variable in it must stay unexpanded, and the value an expandable string.
    $marker = '%USERPROFILE%\sb-installer-test-marker'
    $start = if ($original.Value) { "$($original.Value);$marker" } else { $marker }
    Restore-RegistryPath @{ Value = $start; Kind = [Microsoft.Win32.RegistryValueKind]::ExpandString }

    $dir = Join-Path $work 'Programs\spend branch'
    $exe = Join-Path $dir 'spendbranch.exe'
    Test-Check 'installs and the program runs' {
        (Invoke-Installer @{ SPENDBRANCH_INSTALL_DIR = $dir }) -eq 0 -and (& $exe --version) -and $LASTEXITCODE -eq 0
    }
    Test-Check 'verifies the checksum' { Select-String -Quiet -SimpleMatch 'checksum verified' (Join-Path $work 'out.txt') }
    Test-Check 'with SPENDBRANCH_NO_SETUP, only says the next step' {
        Select-String -Quiet -SimpleMatch "next: run 'spendbranch setup' to connect" (Join-Path $work 'out.txt')
    }
    Test-Check 'a second install works' {
        (Invoke-Installer @{ SPENDBRANCH_INSTALL_DIR = $dir }) -eq 0 -and (Test-Path -LiteralPath $exe)
    }
    # A running .exe is open the way this handle is: it can be renamed, not overwritten.
    Test-Check 'a second install over a program in use works' {
        $inUse = [IO.File]::Open($exe, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]'Read, Delete')
        try {
            (Invoke-Installer @{ SPENDBRANCH_INSTALL_DIR = $dir }) -eq 0
        } finally { $inUse.Close() }
    }
    Test-Check 'and the new program runs' { (& $exe --version) -and $LASTEXITCODE -eq 0 }
    Test-Check 'the folder is on the user Path once' {
        $entries = @((Get-RegistryPath).Value -split ';' | Where-Object { $_ -ieq $dir })
        $entries.Count -eq 1
    }
    Test-Check 'the user Path keeps its variables and stays expandable' {
        $now = Get-RegistryPath
        ($now.Value -split ';' -contains $marker) -and $now.Kind -eq [Microsoft.Win32.RegistryValueKind]::ExpandString
    }
    Test-Check 'only spendbranch.exe is left in the folder' {
        $names = @(Get-ChildItem -LiteralPath $dir -Force | ForEach-Object Name)
        $names.Count -eq 1 -and $names[0] -eq 'spendbranch.exe'
    }

    $other = Join-Path $work 'nopath'
    Restore-RegistryPath $original
    Test-Check 'SPENDBRANCH_NO_MODIFY_PATH leaves the user Path alone' {
        (Invoke-Installer @{ SPENDBRANCH_INSTALL_DIR = $other; SPENDBRANCH_NO_MODIFY_PATH = '1' }) -eq 0 -and
        (Test-Path (Join-Path $other 'spendbranch.exe')) -and ((Get-RegistryPath).Value -eq $original.Value)
    }

    # Refusals: nothing is installed.
    $bad = Join-Path $work 'bad'
    Copy-Item -Recurse -LiteralPath $dist -Destination $bad
    [IO.File]::WriteAllText((Join-Path $bad 'SHA256SUMS'), ('0' * 64) + "  $($zip.Name)`n")
    $none = Join-Path $work 'none'
    Test-Check 'a wrong checksum is refused' {
        (Invoke-Installer @{ SPENDBRANCH_INSTALL_DIR = $none; SPENDBRANCH_DOWNLOAD_URL = $bad }) -ne 0
    }
    Test-Check 'and says so' { Select-String -Quiet -SimpleMatch 'checksum mismatch' (Join-Path $work 'out.txt') }
    Test-Check 'and installs nothing' { -not (Test-Path -LiteralPath $none) }
    Test-Check 'a version that is not one is refused' {
        (Invoke-Installer @{ SPENDBRANCH_INSTALL_DIR = $none; SPENDBRANCH_VERSION = '1.0;calc' }) -ne 0 -and
        (Select-String -Quiet -SimpleMatch 'not a version' (Join-Path $work 'out.txt'))
    }
    Test-Check 'an http:// address is refused' {
        (Invoke-Installer @{ SPENDBRANCH_INSTALL_DIR = $none; SPENDBRANCH_DOWNLOAD_URL = 'http://example.com' }) -ne 0 -and
        (Select-String -Quiet -SimpleMatch 'only https://' (Join-Path $work 'out.txt'))
    }
    Test-Check 'and none of the refusals installed anything' { -not (Test-Path -LiteralPath $none) }
} finally {
    Restore-RegistryPath $original
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host ''
if ($failures) {
    Write-Host "$failures check(s) failed"
    exit 1
}
Write-Host 'all installer checks passed'
# The refusal checks leave a failing $LASTEXITCODE behind; the runner would report it.
exit 0
