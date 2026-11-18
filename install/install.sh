#!/bin/sh
# Installs the SpendBranch CLI on Linux and macOS, then runs `spendbranch setup`:
#
#   curl -fsSL https://github.com/SpendBranch/SpendBranch_CLI/releases/latest/download/install.sh | sh
#
# What it does, in order: picks the archive for this system, downloads it and SHA256SUMS from
# the same GitHub release, checks the SHA-256 (nothing is installed if it differs), installs the
# program into ~/.local/bin (no sudo), puts that folder on the PATH in your shell's startup files,
# and, in a terminal, runs `spendbranch setup` to connect this device.
#
# Optional settings (environment variables):
#   SPENDBRANCH_VERSION=1.2.3         install this version instead of the one this script is for
#   SPENDBRANCH_INSTALL_DIR=<dir>     install there instead of ~/.local/bin
#   SPENDBRANCH_NO_MODIFY_PATH=1      do not edit shell startup files
#   SPENDBRANCH_NO_SETUP=1            do not run `spendbranch setup` at the end
#   SPENDBRANCH_DOWNLOAD_URL=<url>    download from this https:// address or local folder instead
#   SPENDBRANCH_SERVER=https://...    not read here: `spendbranch setup` uses it
#
# Everything runs inside main(), called on the last line: a download cut short runs nothing.

set -u

# Written by the release pipeline; empty in the repository copy.
RELEASE_VERSION=""
REPOSITORY="SpendBranch/SpendBranch_CLI"
PATH_MARK="# added by the SpendBranch CLI installer"

say() {
  printf 'spendbranch-install: %s\n' "$*"
}

fail() {
  printf 'spendbranch-install: error: %s\n' "$*" >&2
  exit 1
}

has() {
  command -v "$1" > /dev/null 2>&1
}

# The target name used in the release archives, from the kernel and CPU.
detect_target() {
  os=$(uname -s)
  cpu=$(uname -m)
  case "$os" in
    Linux) os=linux ;;
    Darwin) os=macos ;;
    *) fail "this system ($os) is not supported by this script; on Windows use install.ps1" ;;
  esac
  case "$cpu" in
    x86_64 | amd64) cpu=x64 ;;
    aarch64 | arm64) cpu=arm64 ;;
    *) fail "this processor ($cpu) has no SpendBranch CLI build" ;;
  esac
  # A shell running under Rosetta on Apple silicon reports x86_64: install the native build.
  if [ "$os" = macos ] && [ "$cpu" = x64 ] &&
    [ "$(sysctl -n sysctl.proc_translated 2> /dev/null)" = 1 ]; then
    cpu=arm64
  fi
  printf '%s-%s' "$os" "$cpu"
}

# fetch <source> <destination>: https only, or a copy from a local folder (tests, mirrors).
fetch() {
  case "$1" in
    https://*)
      if has curl; then
        curl --proto '=https' --tlsv1.2 -fsSL --retry 3 -o "$2" "$1"
      elif has wget; then
        wget --https-only --secure-protocol=TLSv1_2 -q -O "$2" "$1"
      else
        fail "curl or wget is needed to download the CLI"
      fi
      ;;
    *://*) fail "only https:// downloads are allowed: $1" ;;
    *) cp "$1" "$2" ;;
  esac
}

sha256_of() {
  if has sha256sum; then
    sha256sum "$1" | cut -d ' ' -f 1
  elif has shasum; then
    shasum -a 256 "$1" | cut -d ' ' -f 1
  elif has openssl; then
    openssl dgst -sha256 -r "$1" | cut -d ' ' -f 1
  else
    fail "sha256sum, shasum or openssl is needed to check the download"
  fi
}

# The expected sum for <file> in <SHA256SUMS> (lines "<sum>  <file>" or "<sum> *<file>").
expected_sum() {
  awk -v f="$2" '$2 == f || $2 == "*" f { print $1; exit }' "$1"
}

# Adds the PATH line to <file> once (an existing identical line counts).
add_line() {
  file=$1 line=$2
  if [ -f "$file" ] && grep -qxF "$line" "$file"; then
    return 0
  fi
  mkdir -p "$(dirname "$file")" || return 1
  printf '\n%s\n%s\n' "$PATH_MARK" "$line" >> "$file" || return 1
  say "added $dir to the PATH in $file"
}

# Puts <dir> on the PATH in the startup files of the shells this person uses.
modify_path() {
  case ":${PATH:-}:" in
    *":$dir:"*) return 0 ;;
  esac
  if [ "${SPENDBRANCH_NO_MODIFY_PATH:-}" = 1 ]; then
    say "$dir is not on your PATH; add it yourself (SPENDBRANCH_NO_MODIFY_PATH is set)"
    return 0
  fi
  if [ "$dir" = "$HOME/.local/bin" ]; then
    # shellcheck disable=SC2016 # written as is: the shell expands it when it starts
    posix_line='export PATH="$HOME/.local/bin:$PATH"'
  else
    quoted=$(printf '%s' "$dir" | sed "s/'/'\\\\''/g")
    posix_line="export PATH='$quoted':\"\$PATH\""
  fi
  login_shell=$(basename "${SHELL:-sh}")

  add_line "$HOME/.profile" "$posix_line"
  if [ "$login_shell" = bash ] || [ -f "$HOME/.bashrc" ]; then
    add_line "$HOME/.bashrc" "$posix_line"
  fi
  # bash reads ~/.bash_profile instead of ~/.profile when it exists (the macOS default).
  [ -f "$HOME/.bash_profile" ] && add_line "$HOME/.bash_profile" "$posix_line"
  zdot=${ZDOTDIR:-$HOME}
  if [ "$login_shell" = zsh ] || [ -f "$zdot/.zshrc" ]; then
    add_line "$zdot/.zshrc" "$posix_line"
  fi
  if [ "$login_shell" = fish ] || [ -d "$HOME/.config/fish" ]; then
    fish_dir=$(printf '%s' "$dir" | sed -e 's/\\/\\\\/g' -e "s/'/\\\\'/g")
    add_line "$HOME/.config/fish/conf.d/spendbranch.fish" "fish_add_path '$fish_dir'"
  fi
  path_changed=1
}

# Runs `spendbranch setup` when there is a terminal to answer it.
run_setup() {
  if [ "${SPENDBRANCH_NO_SETUP:-}" = 1 ]; then
    say "next: run 'spendbranch setup' to connect this device"
    return 0
  fi
  # Under `curl | sh` the standard input is the pipe: setup reads the terminal instead.
  if ! (exec < /dev/tty) 2> /dev/null; then
    say "next: run 'spendbranch setup' in a terminal to connect this device"
    return 0
  fi
  say "connecting this device: spendbranch setup"
  "$dir/spendbranch" setup < /dev/tty
  setup_code=$?
  if [ "$setup_code" -ne 0 ]; then
    say "the CLI is installed; finish connecting with: spendbranch setup"
  fi
}

path_hint() {
  if [ "$path_changed" = 1 ]; then
    say "open a new terminal (or run: export PATH=\"$dir:\$PATH\") to use 'spendbranch'"
  fi
}

main() {
  version=${SPENDBRANCH_VERSION:-$RELEASE_VERSION}
  [ -n "$version" ] || fail "no version: use the install.sh of a release, or set SPENDBRANCH_VERSION"
  version=${version#v}
  printf '%s' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$' ||
    fail "not a version: $version"
  [ -n "${HOME:-}" ] || fail "HOME is not set"

  target=$(detect_target) || exit 1
  archive="spendbranch-$version-$target.tar.gz"
  base=${SPENDBRANCH_DOWNLOAD_URL:-https://github.com/$REPOSITORY/releases/download/v$version}
  base=${base%/}
  dir=${SPENDBRANCH_INSTALL_DIR:-$HOME/.local/bin}
  [ "$(id -u)" -ne 0 ] || say "running as root: installing for root, in $dir"

  tmp=$(mktemp -d 2> /dev/null || mktemp -d -t spendbranch) || fail "cannot create a temporary folder"
  trap 'rm -rf "$tmp"' EXIT
  trap 'exit 1' INT TERM HUP

  say "downloading SpendBranch CLI $version for $target"
  fetch "$base/$archive" "$tmp/$archive" || fail "download failed: $base/$archive"
  fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS" || fail "download failed: $base/SHA256SUMS"

  want=$(expected_sum "$tmp/SHA256SUMS" "$archive")
  [ -n "$want" ] || fail "SHA256SUMS has no line for $archive; nothing was installed"
  got=$(sha256_of "$tmp/$archive") || exit 1
  [ "$got" = "$want" ] || fail "checksum mismatch for $archive (expected $want, got $got); nothing was installed"
  say "checksum verified"

  tar -xzf "$tmp/$archive" -C "$tmp" || fail "could not extract $archive"
  new="$tmp/spendbranch-$version-$target/spendbranch"
  [ -f "$new" ] || fail "$archive does not hold the program; nothing was installed"
  chmod 755 "$new"
  "$new" --version > /dev/null 2>&1 || fail "the downloaded program does not run on this system; nothing was installed"

  mkdir -p "$dir" || fail "cannot create $dir"
  # Copy beside the target, then rename: atomic, and safe while the old program runs.
  staged="$dir/.spendbranch.new.$$"
  if ! { cp "$new" "$staged" && chmod 755 "$staged" && mv -f "$staged" "$dir/spendbranch"; }; then
    rm -f "$staged"
    fail "cannot write $dir/spendbranch"
  fi
  say "installed SpendBranch CLI $("$dir/spendbranch" --version) in $dir"

  path_changed=0
  modify_path
  found=$(command -v spendbranch 2> /dev/null || true)
  if [ -n "$found" ] && [ "$found" != "$dir/spendbranch" ]; then
    say "warning: another spendbranch comes first on your PATH: $found (remove it, or put $dir first)"
  fi
  setup_code=0
  run_setup
  path_hint
  # A setup that did not finish is this run's result: the CLI stays installed.
  exit "$setup_code"
}

main "$@"
