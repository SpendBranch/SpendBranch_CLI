#!/bin/sh
# Writes the winget manifests of a release: manifests.sh <version> <SHA256SUMS> <out-dir>
# Only for the first submission of SpendBranch.CLI to microsoft/winget-pkgs (copy the folder it
# prints to manifests/s/SpendBranch/CLI/<version>/ in a fork and open a pull request, or check
# it first with `winget validate <folder>`). Later releases are submitted by the release job.
set -eu

version=$1 sums=$2 out=$3
printf '%s' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || {
  echo "manifests.sh: not a release version: $version" >&2
  exit 1
}
id="SpendBranch.CLI"
repo="https://github.com/SpendBranch/SpendBranch_CLI"
url="$repo/releases/download/v$version"

sum() {
  file="spendbranch-$version-$1.zip"
  found=$(awk -v f="$file" '$2 == f || $2 == "*" f { print $1; exit }' "$sums")
  printf '%s' "$found" | grep -Eq '^[0-9a-f]{64}$' || {
    echo "manifests.sh: $sums has no SHA-256 for $file" >&2
    exit 1
  }
  printf '%s' "$found" | tr 'a-f' 'A-F'
}

x64=$(sum windows-x64)
arm64=$(sum windows-arm64)
dir="$out/$version"
mkdir -p "$dir"

cat > "$dir/$id.yaml" << EOF
PackageIdentifier: $id
PackageVersion: $version
DefaultLocale: en-US
ManifestType: version
ManifestVersion: 1.6.0
EOF

cat > "$dir/$id.installer.yaml" << EOF
PackageIdentifier: $id
PackageVersion: $version
InstallerType: zip
NestedInstallerType: portable
NestedInstallerFiles:
- RelativeFilePath: spendbranch.exe
  PortableCommandAlias: spendbranch
Installers:
- Architecture: x64
  InstallerUrl: $url/spendbranch-$version-windows-x64.zip
  InstallerSha256: $x64
- Architecture: arm64
  InstallerUrl: $url/spendbranch-$version-windows-arm64.zip
  InstallerSha256: $arm64
ManifestType: installer
ManifestVersion: 1.6.0
EOF

cat > "$dir/$id.locale.en-US.yaml" << EOF
PackageIdentifier: $id
PackageVersion: $version
PackageLocale: en-US
Publisher: SpendBranch
PublisherUrl: https://github.com/SpendBranch
PublisherSupportUrl: $repo/issues
PackageName: SpendBranch CLI
PackageUrl: $repo
License: Source-available (not open source)
LicenseUrl: $repo/blob/main/LICENSE
ShortDescription: Collects only authorized AI coding agent session metadata for SpendBranch.
Description: |-
  Reads only company-authorized metadata (IDs, model, token counts, normalized repository and
  branch) from local Claude Code and Codex sessions and uploads it to SpendBranch. Prompts,
  responses, code and local paths never leave the device. Run "spendbranch setup" after
  installing.
Moniker: spendbranch
Tags:
- cli
- ai
- claude-code
- codex
ReleaseNotesUrl: $repo/releases/tag/v$version
ManifestType: defaultLocale
ManifestVersion: 1.6.0
EOF

echo "$dir"
