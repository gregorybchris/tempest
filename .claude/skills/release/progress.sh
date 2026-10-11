#!/bin/bash
# Prints a grid of the release's 15 executables (5 targets x 3 binaries) and their build state.
#   progress.sh MARKER LOG [INTERVAL_SECONDS]
# MARKER is a file touched when the builds started: an archive in target/distrib counts as built
# only if it is newer, since target/distrib still holds the last release's archives. LOG is the
# build script's output; it holds "== <target>" lines, "FAILED <target>" and "ALL-DONE". With an
# interval, prints a grid every INTERVAL seconds until the builds end, and once more at the end.
marker=$1 log=$2 interval=${3:-0}
cd "$(dirname "$0")/../../.." || exit 1
targets="aarch64-apple-darwin x86_64-apple-darwin aarch64-unknown-linux-gnu x86_64-unknown-linux-gnu x86_64-pc-windows-msvc"
binaries="tempest tracker relay"

archive() { # target binary
  local ext=tar.xz name
  [[ $1 == *windows* ]] && ext=zip
  case $2 in
    tempest) name=tempest ;;
    tracker) name=tempest-tracker-cli ;;
    relay) name=tempest-relay-cli ;;
  esac
  echo "target/distrib/$name-$1.$ext"
}

grid() {
  local current done=0 failed binary building elapsed row t b file cell
  current=$(grep '^== ' "$log" | tail -1 | cut -c4-)
  failed=$(grep '^FAILED ' "$log" | tail -1 | cut -c8-)
  # The binary being compiled now, from the main file of the crate rustc is working on.
  building=$(ps -axo command | grep -o 'crates/\(tracker-cli\|relay-cli\|tempest\)/src/main.rs' | head -1)
  case $building in
    *tracker-cli*) building=tracker ;; *relay-cli*) building=relay ;; *tempest*) building=tempest ;; *) building= ;;
  esac
  elapsed=$(( $(date +%s) - $(stat -f %m "$marker") ))
  rows=""
  for t in $targets; do
    row=$(printf '%-27s' "$t")
    for b in $binaries; do
      file=$(archive "$t" "$b")
      if [[ -f $file && $file -nt $marker ]]; then
        cell=✅; done=$((done + 1))
      elif [[ $t == "$failed" ]]; then
        cell=❌
      elif [[ $t == "$current" && $b == "$building" ]]; then
        cell=🔨
      else
        cell=⏳
      fi
      row+="$cell $b  "
    done
    rows+="$row"$'\n'
  done
  printf 'Release builds: %d/15 built, %dm elapsed\n%s' "$done" $((elapsed / 60)) "$rows"
}

if [[ $interval -eq 0 ]]; then grid; exit; fi
while ! grep -q '^ALL-DONE\|^FAILED' "$log"; do
  grid; echo
  sleep "$interval"
done
grid
grep '^ALL-DONE\|^FAILED' "$log"
