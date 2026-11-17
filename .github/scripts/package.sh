#!/bin/sh
# Packages one built binary for a release: package.sh <binary> <version> <target> <out-dir>
# (target: linux-x64, macos-arm64, windows-x64, ...). Used by the release and by the CI, so the
# installers are tested against the very layout they download.
#
# - Linux and macOS: <out>/spendbranch-<version>-<target>.tar.gz, with one folder of that name
#   inside (install.sh knows the path).
# - Windows: <out>/spendbranch-<version>-<target>.zip, flat: spendbranch.exe at the root, so a
#   folder installed into keeps no version in its path (the hooks and schedule hold that path).
set -eu

bin=$1 version=$2 target=$3 out=$4
name="spendbranch-$version-$target"
root=$(cd "$(dirname "$0")/../.." && pwd)

mkdir -p "$out"
out=$(cd "$out" && pwd)
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT

case "$target" in
  windows-*)
    cp "$bin" "$stage/spendbranch.exe"
    cp "$root/LICENSE" "$root/README.md" "$root/docs/INSTALL.md" "$stage/"
    rm -f "$out/$name.zip"
    if command -v 7z > /dev/null; then
      (cd "$stage" && 7z a -tzip "$out/$name.zip" ./* > /dev/null)
    else
      (cd "$stage" && zip -q "$out/$name.zip" ./*)
    fi
    ;;
  *)
    mkdir "$stage/$name"
    cp "$bin" "$stage/$name/spendbranch"
    chmod 755 "$stage/$name/spendbranch"
    cp "$root/LICENSE" "$root/README.md" "$root/docs/INSTALL.md" "$stage/$name/"
    tar -C "$stage" -czf "$out/$name.tar.gz" "$name"
    ;;
esac
