#!/usr/bin/env bash
# Self-test for the harness's teardown and real-credential checks. Needs no
# provider, login or Codemux build: a stand-in serve runs in the real sandbox
# (lib.sh's sandbox_args/launch_sandbox) and leaves a stubborn child behind,
# the way a provider CLI outlives serve. HOME points at a scratch directory
# with fake credential files, so the user's real ones are never touched.
#
#   tests/teardown-test.sh
#
# Checks:
#   1. The recorded sandbox PID is the PID namespace's init, which outlives
#      bwrap's outer process while an orphaned child is still running.
#   2. stop_sandbox --hard waits for that child and stops it with SIGTERM,
#      never SIGKILL (the child logs the signal and exits cleanly).
#   3. A child that ignores SIGTERM makes stop_sandbox fail and list it,
#      still alive; it is not SIGKILLed by the harness or the kernel.
#   4. check_real_credentials fails when a credential file that existed at
#      up.sh disappears, and skips one that never existed.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
for tool in bwrap jq python3 setsid; do
  command -v "$tool" >/dev/null 2>&1 || { echo "SKIP: $tool not installed"; exit 0; }
done

# Scratch space next to the live roots, never /tmp (shared quota).
BASE="${ORCH_LIVE_TEST_BASE:-$HOME/.cache/codemux-agents/orch-v2/tests}"
mkdir -p "$BASE"
T="$(mktemp -d "$BASE/teardown.XXXXXX")"
export HOME="$T/home"
mkdir -p "$HOME/.codex" "$HOME/.claude"

# shellcheck source=../lib.sh
. "$HERE/../lib.sh"
export ORCH_LIVE_STOP_GRACE=4

FAILED=0
ok() { echo "ok   - $*"; }
bad() { echo "FAIL - $*" >&2; FAILED=1; }
check() { # $1=description, rest=command
  local what="$1"
  shift
  if "$@"; then ok "$what"; else bad "$what"; fi
}
nap() { python3 -c "import time; time.sleep($1)"; }
wait_for() { # $1=seconds, rest=command
  local i n=$(($1 * 10))
  shift
  for i in $(seq 1 "$n"); do "$@" && return 0; nap 0.1; done
  return 1
}

# A child that records the signals it gets. term-exits: exits 0.5 s after
# SIGTERM (a provider flushing state). ignores-term: exits only once
# <log>.release exists. Neither exits on stdin EOF.
cat >"$T/child.py" <<'EOF'
import os, signal, sys, time
log, mode = sys.argv[1], sys.argv[2]
def note(msg):
    with open(log, "a") as f:
        f.write(msg + "\n")
def on_term(sig, _):
    note("got SIGTERM")
    if mode == "term-exits":
        time.sleep(0.5)
        note("exit after SIGTERM")
        sys.exit(0)
signal.signal(signal.SIGTERM, on_term)
note("start")
deadline = time.monotonic() + 120
while time.monotonic() < deadline:
    if os.path.exists(log + ".release"):
        note("released")
        sys.exit(0)
    time.sleep(0.1)
note("timed out")
EOF
# Stand-in serve: start the child, then wait to be stopped.
cat >"$T/serve.sh" <<'EOF'
python3 -I "$1" "$2" "$3" </dev/null >/dev/null 2>&1 &
trap 'exit 0' TERM
while :; do sleep 0.1; done
EOF

# One run root per case, laid out like up.sh's.
new_root() {
  ROOT="$T/root-$1"
  mkdir -p "$ROOT"/{home,logs,bin,runtime,empty/dir}
  : >"$ROOT/empty/file"
  # A private copy of bash, so serve_process can tell the stand-in serve
  # from everything else by its executable.
  BIN="$ROOT/bin/fake-serve"
  cp "$(readlink -f "$(command -v bash)")" "$BIN"
  SIDECAR=/nonexistent
  RUN_ID="$(cat /proc/sys/kernel/random/uuid)"
  CHILD_LOG="$ROOT/child.log"
}
child_pid() { # host PID of the stand-in's child, or empty
  local p
  for p in $(descendants "$(sandbox_pid)"); do
    case "$(readlink -f "/proc/$p/exe" 2>/dev/null)" in */python3*) echo "$p"; return ;; esac
  done
}
logged() { grep -qxF "$1" "$CHILD_LOG" 2>/dev/null; }
is_ns_init() { [ "$(awk '/^NSpid:/ {print $NF}' "/proc/$1/status" 2>/dev/null)" = 1 ]; }
parent_of() { awk '/^PPid:/ {print $2}' "/proc/$1/status" 2>/dev/null; }

STARTED_ROOTS=()
cleanup() {
  local r
  # Release any child still waiting, then let its sandbox wind down.
  for r in "${STARTED_ROOTS[@]}"; do : >"$r/child.log.release"; done
  for r in "${STARTED_ROOTS[@]}"; do
    ROOT="$r"
    # shellcheck disable=SC1091
    . "$r/pids"
    wait_sandbox_gone 10 || echo "WARN: test sandbox $INIT_PID still running; left $T in place" >&2
  done
  for r in "${STARTED_ROOTS[@]}"; do (ROOT="$r"; . "$r/pids"; sandbox_alive 2>/dev/null) && return; done
  rm -rf "$T"
}
trap cleanup EXIT

start_case() { # $1=name $2=child mode
  new_root "$1"
  launch_sandbox "$ROOT/logs/serve.log" "$BIN" "$T/serve.sh" "$T/child.py" "$CHILD_LOG" "$2"
  printf "SERVE_PID='%s' SERVE_START='%s' INIT_PID='%s' INIT_START='%s' RUN_ID='%s'\n" \
    "$SERVE_PID" "$SERVE_START" "$INIT_PID" "$INIT_START" "$RUN_ID" >"$ROOT/pids"
  STARTED_ROOTS+=("$ROOT")
  wait_for 10 logged start || { bad "$1: child never started (see $ROOT/logs/serve.log)"; exit 1; }
}

# ── 1 + 2: hard stop, child exits on SIGTERM ─────────────────────────────
echo "# hard stop with an orphaned child that exits on SIGTERM"
start_case hard term-exits
check "init pid is recorded and differs from bwrap's outer pid" \
  test -n "$INIT_PID" -a "$INIT_PID" != "$SERVE_PID"
check "init pid is pid 1 of the sandbox's PID namespace" is_ns_init "$INIT_PID"
check "init is the outer bwrap's child" test "$(parent_of "$INIT_PID")" = "$SERVE_PID"
check "serve_process finds the stand-in serve below the init" \
  test "$(readlink -f "/proc/$(serve_process)/exe")" = "$BIN"
CHILD="$(child_pid)"
check "the child runs inside the sandbox" test -n "$CHILD"

stop_sandbox hard >"$ROOT/stop.out" 2>&1 &
STOPPER=$!
# The outer process goes with serve; the sandbox must still count as running.
if wait_for 5 eval '! proc_running "$SERVE_PID"'; then
  ok "bwrap's outer process exits with serve"
  check "the child is still running after the outer process exited" proc_running "$CHILD"
  check "sandbox_alive still reports the sandbox (tracks the init)" sandbox_alive
else
  bad "bwrap's outer process did not exit with serve"
fi
if wait "$STOPPER"; then ok "stop_sandbox hard returned success"; else bad "stop_sandbox hard failed: $(cat "$ROOT/stop.out")"; fi
check "the child got SIGTERM" logged "got SIGTERM"
check "the child exited cleanly after SIGTERM (not SIGKILLed)" logged "exit after SIGTERM"
check "the child is gone when stop_sandbox returns" eval '! proc_running "$CHILD"'
check "the init is gone when stop_sandbox returns" eval '! proc_running "$INIT_PID"'
grep -q "SIGTERM to its remaining processes" "$ROOT/stop.out" \
  && ok "stop_sandbox waited the grace period before SIGTERM" || bad "no grace-period note in stop output"

# ── 3: graceful stop, child ignores SIGTERM ──────────────────────────────
echo "# graceful stop with a child that ignores SIGTERM"
start_case stubborn ignores-term
CHILD="$(child_pid)"
if stop_sandbox graceful >"$ROOT/stop.out" 2>&1; then
  bad "stop_sandbox succeeded although a process survived: $(cat "$ROOT/stop.out")"
else
  ok "stop_sandbox fails when a process survives"
fi
check "the failure lists the survivor" grep -q "^  $CHILD " "$ROOT/stop.out"
check "the child got SIGTERM" logged "got SIGTERM"
check "the child is still alive (never SIGKILLed)" proc_running "$CHILD"
check "the sandbox still counts as running" sandbox_alive
: >"$CHILD_LOG.release"
check "the sandbox ends once the child exits on its own" wait_sandbox_gone 10
check "the child exited on its own" logged "released"

# ── 4: credential checks ─────────────────────────────────────────────────
echo "# real-credential checks (fake HOME)"
new_root creds
echo '{"tokens":{"access_token":"fake"}}' >"$HOME/.codex/auth.json"
rm -f "$HOME/.claude/.credentials.json"
claude_real_snapshot >"$ROOT/claude-real.snapshot"
record_real_credentials
check "credentials.present lists only the file that exists" \
  test "$(cat "$ROOT/credentials.present")" = "$HOME/.codex/auth.json"
check "check passes; the never-present Claude file is skipped" eval 'check_real_credentials >"$ROOT/check.out" 2>&1'
rm "$HOME/.codex/auth.json"
if check_real_credentials >"$ROOT/check.out" 2>&1; then
  bad "check passed although auth.json vanished during the run"
else
  ok "check fails when a file present at up.sh vanished"
fi
check "the failure names the vanished file" grep -q "auth.json existed when up.sh ran and is gone now" "$ROOT/check.out"
echo '' >"$HOME/.codex/auth.json"
check "check still fails on an empty credential file" eval '! check_real_credentials >/dev/null 2>&1'

if [ "$FAILED" = 0 ]; then echo "PASS: teardown-test"; else echo "FAIL: teardown-test" >&2; exit 1; fi
