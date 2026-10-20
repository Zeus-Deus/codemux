#!/usr/bin/env bash
# Stop the instance up.sh started, and the harness's browser session.
#
#   down.sh [--hard] [root]
#
# Default: SIGTERM the codemux serve process inside the sandbox (graceful
# shutdown, the same path as quitting the app), wait up to 30 s, then stop
# the sandbox's process group. --hard: SIGKILL the process group at once
# (crash simulation for restart scenarios). Only the PID recorded in
# <root>/serve.pid and its descendants are ever signalled. Logs, the store and
# provider sessions stay under the root, so `up.sh <root>` restarts on them.
set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

HARD=0
if [ "${1:-}" = "--hard" ]; then HARD=1; shift; fi
load_env "${1:-}"

if [ -n "${BROWSER_SESSION:-}" ] && command -v agent-browser >/dev/null 2>&1; then
  agent-browser --session "$BROWSER_SESSION" close >/dev/null 2>&1 || true
fi

PID="$(cat "$ROOT/serve.pid" 2>/dev/null || echo "${SERVE_PID:-}")"
if [ -z "$PID" ] || ! kill -0 "$PID" 2>/dev/null; then
  note "serve not running (pid ${PID:-none})"
  exit 0
fi
# Guard against PID reuse: the recorded PID must still be our bwrap.
[ "$(readlink -f "/proc/$PID/exe" 2>/dev/null)" = "$(readlink -f "$(command -v bwrap)")" ] \
  || fail "pid $PID is no longer the sandbox this root started; not signalling it"
PGID="$(ps -o pgid= -p "$PID" | tr -d ' ')"
[ "$PGID" = "$PID" ] || fail "pid $PID does not lead its process group ($PGID); not signalling it"

wait_gone() { # $1=seconds
  local i
  for i in $(seq 1 $(($1 * 4))); do
    kill -0 "$PID" 2>/dev/null || return 0
    python3 -c 'import time; time.sleep(0.25)'
  done
  return 1
}

if [ "$HARD" = 0 ]; then
  SERVE=""
  for child in $(descendants "$PID"); do
    if [ "$(readlink -f "/proc/$child/exe" 2>/dev/null)" = "$BIN" ]; then SERVE="$child"; break; fi
  done
  if [ -n "$SERVE" ]; then
    kill -TERM "$SERVE" 2>/dev/null || true
    wait_gone 30 && { pass "serve stopped gracefully"; exit 0; }
    note "serve did not exit within 30 s; stopping the sandbox group"
  fi
  kill -TERM -- "-$PGID" 2>/dev/null || true
  wait_gone 10 && { pass "sandbox stopped"; exit 0; }
fi
kill -KILL -- "-$PGID" 2>/dev/null || true
wait_gone 10 || fail "sandbox pid $PID survived SIGKILL"
if [ "$HARD" = 1 ]; then pass "sandbox killed (hard)"; else pass "sandbox killed"; fi
