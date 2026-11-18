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

# --- the screen: the same marks and colors as the CLI -----------------------------------
# Off a terminal: plain lines with [ok] [!] [x]. On a terminal: the line between steps and a
# spinner while downloading. Symbols and color only where the terminal says it shows truecolor
# (COLORTERM, or Windows Terminal), never with NO_COLOR or TERM=dumb: the CLI's own rule.
tty=0
fancy=0
[ -t 1 ] && tty=1
if [ "$tty" = 1 ] && [ -z "${NO_COLOR:-}" ] && [ "${TERM:-}" != dumb ]; then
  case "${COLORTERM:-}" in truecolor | 24bit) fancy=1 ;; esac
  [ -n "${WT_SESSION:-}" ] && fancy=1
fi
if [ "$fancy" = 1 ]; then
  E=$(printf '\033')
  OK="${E}[32m✓${E}[0m"
  WARN="${E}[33m!${E}[0m"
  BAD="${E}[31m✗${E}[0m"
  BOLD="${E}[1m"
  AMBER="${E}[1;38;2;240;176;32m"
  LINE="${E}[38;2;156;112;68m"
  DIM="${E}[2m"
  RESET="${E}[0m"
  FRAMES="⠋ ⠙ ⠹ ⠸ ⠼ ⠴ ⠦ ⠧ ⠇ ⠏"
else
  OK="[ok]" WARN="[!]" BAD="[x]" BOLD="" AMBER="" LINE="" DIM="" RESET=""
  # shellcheck disable=SC1003 # the last frame is a backslash
  FRAMES='| / - \'
fi
spinning=0

# step <mark> <name> <detail>: how a step ended.
step() {
  printf '%s %s%-11s%s %s\n' "$1" "$BOLD" "$2" "$RESET" "$3"
}

# The line that joins two steps, on a terminal.
joint() {
  if [ "$tty" = 1 ]; then printf '%s│%s\n' "$LINE" "$RESET"; fi
}

# A command to type: amber in color, in backticks without.
cmd() {
  if [ "$fancy" = 1 ]; then
    printf '%s%s%s' "$AMBER" "$1" "$RESET"
  else
    # shellcheck disable=SC2016 # backticks, not a command
    printf '`%s`' "$1"
  fi
}

# A path as the person knows it: ~ for their home.
pretty() {
  case "$1" in
    "$HOME"/*) printf '~%s' "${1#"$HOME"}" ;;
    *) printf '%s' "$1" ;;
  esac
}

stop_spinner() {
  if [ "$spinning" = 1 ]; then
    printf '\r\033[K\033[?25h'
    spinning=0
  fi
}

fail() {
  stop_spinner
  printf '%s %s\n' "$BAD" "$*" >&2
  exit 1
}

# spin <pid> <name> <detail>: an animated mark while <pid> runs; returns its exit status.
spin() {
  spin_pid=$1 spin_name=$2 spin_detail=$3
  if [ "$tty" = 1 ]; then
    spinning=1
    printf '\033[?25l'
    # shellcheck disable=SC2086 # the frames are words
    set -- $FRAMES
    n=$#
    i=0
    frame=""
    while kill -0 "$spin_pid" 2> /dev/null; do
      i=$((i % n + 1))
      eval "frame=\${$i}"
      printf '\r%s%s%s %s%-11s%s %s%s%s' "$AMBER" "$frame" "$RESET" "$BOLD" "$spin_name" \
        "$RESET" "$DIM" "$spin_detail" "$RESET"
      sleep 0.1 2> /dev/null || sleep 1
    done
    stop_spinner
  fi
  wait "$spin_pid"
}

# --- the work ----------------------------------------------------------------------------

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
        echo "curl or wget is needed to download the CLI" >&2
        return 1
      fi
      ;;
    *://*)
      echo "only https:// downloads are allowed: $1" >&2
      return 1
      ;;
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

# The size of a file for people: 2.6 MB.
size_of() {
  wc -c < "$1" | awk '{ printf "%.1f MB", $1 / 1048576 }'
}

# Adds the PATH line to <file> once (an existing identical line counts).
add_line() {
  file=$1 line=$2
  if [ -f "$file" ] && grep -qxF "$line" "$file"; then
    return 0
  fi
  mkdir -p "$(dirname "$file")" || return 1
  printf '\n%s\n%s\n' "$PATH_MARK" "$line" >> "$file" || return 1
  added="${added:+$added, }$(pretty "$file")"
}

# Puts <dir> on the PATH in the startup files of the shells this person uses.
modify_path() {
  case ":${PATH:-}:" in
    *":$dir:"*)
      step "$OK" PATH "$(pretty "$dir") is already on it"
      return 0
      ;;
  esac
  if [ "${SPENDBRANCH_NO_MODIFY_PATH:-}" = 1 ]; then
    step "$WARN" PATH "not changed (SPENDBRANCH_NO_MODIFY_PATH): add $(pretty "$dir") yourself"
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
  added=""

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
  if [ -n "$added" ]; then
    step "$OK" PATH "added to $added"
  else
    step "$OK" PATH "already in your shell files"
  fi
  path_changed=1
}

# Runs `spendbranch setup` when there is a terminal to answer it.
run_setup() {
  if [ "${SPENDBRANCH_NO_SETUP:-}" = 1 ]; then
    printf '\nNext: run %s to connect this device.\n' "$(cmd "spendbranch setup")"
    return 0
  fi
  # Under `curl | sh` the standard input is the pipe: setup reads the terminal instead.
  if ! (exec < /dev/tty) 2> /dev/null; then
    printf '\nNext: run %s in a terminal to connect this device.\n' "$(cmd "spendbranch setup")"
    return 0
  fi
  joint
  "$dir/spendbranch" setup < /dev/tty
  setup_code=$?
  if [ "$setup_code" -ne 0 ]; then
    printf '\n%s The CLI is installed; finish connecting with %s.\n' "$WARN" \
      "$(cmd "spendbranch setup")"
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

  tmp=$(mktemp -d 2> /dev/null || mktemp -d -t spendbranch) || fail "cannot create a temporary folder"
  trap 'rm -rf "$tmp"' EXIT
  trap 'stop_spinner; exit 1' INT TERM HUP

  printf '%sSpendBranch CLI %s%s %s· %s%s\n' "$AMBER" "$version" "$RESET" "$DIM" "$target" "$RESET"
  if [ "$(id -u)" -eq 0 ]; then
    printf '%srunning as root: installing for root, in %s%s\n' "$DIM" "$dir" "$RESET"
  fi
  printf '\n'

  # Both files in the background, the spinner in front; what curl said is kept for the error.
  (fetch "$base/$archive" "$tmp/$archive" && fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS") \
    2> "$tmp/fetch.err" &
  if ! spin $! Download "$archive"; then
    why=$(tail -n 1 "$tmp/fetch.err" 2> /dev/null)
    case "$why" in
      *://*) fail "download failed: $why" ;;
      *) fail "download failed: $base/$archive${why:+ ($why)}" ;;
    esac
  fi
  step "$OK" Download "$archive · $(size_of "$tmp/$archive")"
  joint

  want=$(expected_sum "$tmp/SHA256SUMS" "$archive")
  [ -n "$want" ] || fail "SHA256SUMS has no line for $archive; nothing was installed"
  got=$(sha256_of "$tmp/$archive") || exit 1
  [ "$got" = "$want" ] ||
    fail "checksum mismatch for $archive (expected $want, got $got); nothing was installed"
  step "$OK" Verify "checksum verified (SHA-256)"
  joint

  tar -xzf "$tmp/$archive" -C "$tmp" || fail "could not extract $archive"
  new="$tmp/spendbranch-$version-$target/spendbranch"
  [ -f "$new" ] || fail "$archive does not hold the program; nothing was installed"
  chmod 755 "$new"
  "$new" --version > /dev/null 2>&1 ||
    fail "the downloaded program does not run on this system; nothing was installed"

  mkdir -p "$dir" || fail "cannot create $dir"
  # Copy beside the target, then rename: atomic, and safe while the old program runs.
  staged="$dir/.spendbranch.new.$$"
  if ! { cp "$new" "$staged" && chmod 755 "$staged" && mv -f "$staged" "$dir/spendbranch"; }; then
    rm -f "$staged"
    fail "cannot write $dir/spendbranch"
  fi
  step "$OK" Install "$(pretty "$dir/spendbranch") · $("$dir/spendbranch" --version)"
  joint

  path_changed=0
  modify_path
  found=$(command -v spendbranch 2> /dev/null || true)
  if [ -n "$found" ] && [ "$found" != "$dir/spendbranch" ]; then
    step "$WARN" PATH "another spendbranch comes first: $found (remove it, or put $(pretty "$dir") first)"
  fi
  setup_code=0
  run_setup
  if [ "$path_changed" = 1 ]; then
    # shellcheck disable=SC2016 # $PATH is for the person to type
    printf '%sOpen a new terminal to use spendbranch (or run: export PATH="%s:$PATH").%s\n' \
      "$DIM" "$dir" "$RESET"
  fi
  # A setup that did not finish is this run's result: the CLI stays installed.
  exit "$setup_code"
}

main "$@"
