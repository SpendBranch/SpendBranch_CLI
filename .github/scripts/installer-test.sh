#!/bin/sh
# End-to-end test of install/install.sh: installer-test.sh <binary> <target>
# Packages <binary> the way the release does, then installs it from that folder into throwaway
# HOMEs. Never touches the real HOME. Run by the CI on Linux and macOS.
set -eu

bin=$1 target=$2
root=$(cd "$(dirname "$0")/../.." && pwd)
script="$root/install/install.sh"
version="0.0.0-test"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

dist="$work/dist"
sh "$root/.github/scripts/package.sh" "$bin" "$version" "$target" "$dist"
(cd "$dist" && if command -v sha256sum > /dev/null; then sha256sum -- * ; else shasum -a 256 -- * ; fi > "$work/SHA256SUMS") && mv "$work/SHA256SUMS" "$dist/"

failures=0
check() {
  if eval "$2"; then
    echo "ok   - $1"
  else
    echo "FAIL - $1"
    failures=$((failures + 1))
  fi
}

# run_installer <home> [VAR=value ...]: runs the script piped into sh, as `curl | sh` does.
run_installer() {
  home=$1
  shift
  mkdir -p "$home"
  env -i HOME="$home" SHELL=/bin/bash PATH=/usr/bin:/bin:/usr/sbin:/sbin \
    SPENDBRANCH_DOWNLOAD_URL="$dist" SPENDBRANCH_VERSION="$version" "$@" \
    sh < "$script" > "$home/out.txt" 2>&1
}

# shellcheck disable=SC2016 # the line the installer writes, unexpanded
line='export PATH="$HOME/.local/bin:$PATH"'

# A first install, then the same again: one program, one PATH line per file.
h="$work/a"
mkdir -p "$h" && : > "$h/.bashrc"
check "installs and the program runs" "run_installer '$h' && '$h/.local/bin/spendbranch' --version > /dev/null"
check "verifies the checksum" "grep -q 'checksum verified' '$h/out.txt'"
check "without a terminal, setup is only suggested" "grep -q \"next: run 'spendbranch setup' in a terminal\" '$h/out.txt'"
check "a second install works" "run_installer '$h' && '$h/.local/bin/spendbranch' --version > /dev/null"
check "the profile file has the PATH line once" "[ \"\$(grep -cxF '$line' '$h/.profile')\" = 1 ]"
check "the bashrc file has the PATH line once" "[ \"\$(grep -cxF '$line' '$h/.bashrc')\" = 1 ]"
check "no zsh file for a bash user without one" "[ ! -e '$h/.zshrc' ]"
check "the new PATH finds the program" "env -i HOME='$h' PATH=/usr/bin:/bin sh -c '. \"\$HOME/.profile\"; command -v spendbranch' | grep -qx '$h/.local/bin/spendbranch'"
check "no temporary file is left beside it" "[ -z \"\$(ls -A '$h/.local/bin' | grep -v '^spendbranch\$')\" ]"

# zsh and fish users get their own files.
h="$work/b"
mkdir -p "$h/.config/fish"
check "a zsh user gets ~/.zshrc" "run_installer '$h' SHELL=/bin/zsh && grep -qxF '$line' '$h/.zshrc'"
check "a fish user gets conf.d/spendbranch.fish" "grep -qxF \"fish_add_path '$h/.local/bin'\" '$h/.config/fish/conf.d/spendbranch.fish'"

# Opting out of PATH changes and setup.
h="$work/c"
check "SPENDBRANCH_NO_MODIFY_PATH leaves the shell files alone" "run_installer '$h' SPENDBRANCH_NO_MODIFY_PATH=1 SPENDBRANCH_NO_SETUP=1 && [ ! -e '$h/.profile' ] && [ -x '$h/.local/bin/spendbranch' ]"
check "SPENDBRANCH_NO_SETUP only says the next step" "grep -q \"next: run 'spendbranch setup' to connect\" '$h/out.txt'"

# A folder of its own, with a space and a quote in its name.
h="$work/d"
odd="$work/d/my bin's"
check "installs into SPENDBRANCH_INSTALL_DIR" "run_installer '$h' \"SPENDBRANCH_INSTALL_DIR=$odd\" && [ -x \"$odd/spendbranch\" ]"
check "that folder is on the new PATH, quoted" "env -i HOME='$h' PATH=/usr/bin:/bin sh -c '. \"\$HOME/.profile\"; command -v spendbranch' | grep -qxF \"$odd/spendbranch\""

# Refusals: nothing is installed.
h="$work/e"
cp -R "$dist" "$work/bad"
awk '{ print "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855", $2 }' "$dist/SHA256SUMS" > "$work/bad/SHA256SUMS"
check "a wrong checksum is refused" "! run_installer '$h' SPENDBRANCH_DOWNLOAD_URL='$work/bad'"
check "and says so" "grep -q 'checksum mismatch' '$h/out.txt'"
check "and installs nothing" "[ ! -e '$h/.local/bin/spendbranch' ] && [ ! -e '$h/.profile' ]"
check "a missing SHA256SUMS line is refused" "printf '' > '$work/bad/SHA256SUMS' && ! run_installer '$h' SPENDBRANCH_DOWNLOAD_URL='$work/bad' && [ ! -e '$h/.local/bin/spendbranch' ]"
check "a version that is not one is refused" "! run_installer '$h' 'SPENDBRANCH_VERSION=1.0;id' && grep -q 'not a version' '$h/out.txt'"
check "an http:// address is refused" "! run_installer '$h' SPENDBRANCH_DOWNLOAD_URL=http://example.com && grep -q 'only https://' '$h/out.txt'"
check "without a version the repository copy stops" "! run_installer '$h' SPENDBRANCH_VERSION= && grep -q 'no version' '$h/out.txt'"

echo
if [ "$failures" -ne 0 ]; then
  echo "$failures check(s) failed"
  exit 1
fi
echo "all installer checks passed"
