#!/usr/bin/env bash
#
# Tests for the `tauri build` hook modes of scripts/build-codemux-remote.sh
# (--stub and --stage-tauri-build). Each case runs the script from a scratch
# repo with fake cargo artifacts, so nothing is compiled. Run with:
#   bash scripts/build-codemux-remote.test.sh

set -uo pipefail

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
TRIPLE=x86_64-unknown-linux-gnu
PASS=0
FAIL=0

new_repo() {
  repo=$(mktemp -d)
  mkdir -p "$repo/scripts" "$repo/src-tauri/binaries"
  cp "$SCRIPT_DIR/build-codemux-remote.sh" "$repo/scripts/"
}

artifact() { # <relative target path> <content> <touch date>
  mkdir -p "$(dirname "$repo/src-tauri/target/$1")"
  printf '%s' "$2" > "$repo/src-tauri/target/$1"
  touch -d "$3" "$repo/src-tauri/target/$1"
}

hook() { # <mode flag> [debug]
  (unset CARGO_BUILD_TARGET CARGO_TARGET_DIR
   TAURI_ENV_TARGET_TRIPLE=$TRIPLE TAURI_ENV_DEBUG=${2:-false} \
     bash "$repo/scripts/build-codemux-remote.sh" "$1") > "$repo/log" 2>&1
}

check() { # <label> <expected staged content, or FAIL>
  local staged
  if hook --stub && hook --stage-tauri-build "${debug:-false}"; then
    staged=$(cat "$repo/src-tauri/binaries/codemux-remote-$TRIPLE")
    # Only one file may match the bundle glob, or a stale stub ships too.
    if [ "$(ls "$repo/src-tauri/binaries" | grep -c '^codemux-remote-')" != 1 ]; then staged="extra glob match"; fi
  else
    staged=FAIL
  fi
  if [ "$staged" = "$2" ]; then
    PASS=$((PASS + 1))
  else
    FAIL=$((FAIL + 1))
    echo "FAIL: $1 (expected '$2', got '$staged')"
    sed 's/^/  /' "$repo/log"
  fi
  rm -rf "$repo"
}

new_repo; artifact release/codemux-remote native '2026-01-02'
check 'default build stages target/release' native

new_repo; artifact "$TRIPLE/release/codemux-remote" explicit '2026-01-02'
check 'explicit --target on a clean checkout' explicit

new_repo
artifact release/codemux-remote stale-native '2026-01-01'
artifact "$TRIPLE/release/codemux-remote" current-explicit '2026-01-02'
check 'explicit --target beside a stale native build' current-explicit

new_repo
artifact "$TRIPLE/release/codemux-remote" stale-explicit '2026-01-01'
artifact release/codemux-remote current-native '2026-01-02'
check 'default build beside a stale explicit-target build' current-native

new_repo; artifact debug/codemux-remote debug-build '2026-01-02'; artifact release/codemux-remote release-build '2026-01-03'
debug=true check 'tauri build --debug stages target/debug' debug-build

new_repo
check 'no artifact fails the build' FAIL

echo "build-codemux-remote: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
