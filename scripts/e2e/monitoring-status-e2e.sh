#!/usr/bin/env bash
# End-to-end harness for the provider-agnostic "Monitoring" agent status.
#
# Drives the real control surface — no display, no GUI, no provider. Boots the
# debug `codemux` binary in headless `serve` mode against a throwaway
# HOME/XDG root, then exercises the whole manual-monitoring path over the
# control socket:
#
#   1. create_workspace          → a workspace in a temp git repo
#   2. monitor_start (+ reason)  → resolves the workspace's active pane
#   3. get_app_state             → that pane reports "monitoring", and the
#                                  reason is carried in `manual_monitors`
#   4. monitor_status            → agrees with the snapshot
#   5. monitor_stop              → the status and the flag are both gone
#   6. monitor_start/stop again  → idempotent, and `changed` is honest
#
# Requires: a debug build (`cargo build --manifest-path src-tauri/Cargo.toml`,
# built automatically if missing), python3 (JSON assertions), git, and a free
# TCP port for the web-remote listener `serve` binds (override with
# CMX_MON_E2E_PORT). No network access is needed.
#
# Isolation: HOME, XDG_RUNTIME_DIR, XDG_CONFIG_HOME, XDG_DATA_HOME and
# XDG_STATE_HOME all point into a mktemp dir that is removed on exit, so the
# real ~/.codemux, the real layout.json, and any running Codemux instance are
# never touched. The control socket lands inside that temp XDG_RUNTIME_DIR,
# which is also what keeps `serve`'s "already running" guard from tripping on
# the developer's own desktop instance.
#
# Usage:  bash scripts/e2e/monitoring-status-e2e.sh
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BIN="${REPO_ROOT}/src-tauri/target/debug/codemux"
PORT="${CMX_MON_E2E_PORT:-47731}"
WORK="$(mktemp -d /tmp/cmx-monitoring-e2e.XXXXXX)"
SERVE_PID=""

log() { printf '\n\033[1;36m[e2e]\033[0m %s\n' "$*"; }
err() { printf '\n\033[1;31m[e2e:ERROR]\033[0m %s\n' "$*" >&2; }
ok()  { printf '\033[1;32m  ✓\033[0m %s\n' "$*"; }

cleanup() {
  if [ -n "$SERVE_PID" ] && kill -0 "$SERVE_PID" 2>/dev/null; then
    kill "$SERVE_PID" 2>/dev/null || true
    wait "$SERVE_PID" 2>/dev/null || true
  fi
  rm -rf "$WORK"
}
trap cleanup EXIT

command -v python3 >/dev/null || { err "python3 not found"; exit 1; }
command -v git >/dev/null || { err "git not found"; exit 1; }

# ── 1. Build the debug binary if it isn't there already.
if [ ! -x "$BIN" ]; then
  log "building the debug codemux binary (CARGO_BUILD_JOBS=2)"
  CARGO_BUILD_JOBS=2 cargo build --manifest-path "${REPO_ROOT}/src-tauri/Cargo.toml"
fi
[ -x "$BIN" ] || { err "no debug binary at $BIN"; exit 1; }

# ── 2. Isolated environment. Everything Codemux reads or writes is redirected
#       into $WORK, including the control socket.
export HOME="$WORK/home"
export XDG_RUNTIME_DIR="$WORK/run"
export XDG_CONFIG_HOME="$WORK/config"
export XDG_DATA_HOME="$WORK/data"
export XDG_STATE_HOME="$WORK/state"
export XDG_CACHE_HOME="$WORK/cache"
mkdir -p "$HOME" "$XDG_RUNTIME_DIR" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" \
         "$XDG_STATE_HOME" "$XDG_CACHE_HOME"
chmod 700 "$XDG_RUNTIME_DIR"

# A throwaway git repo for the workspace, so nothing points at a real checkout.
PROJECT="$WORK/project"
mkdir -p "$PROJECT"
git -C "$PROJECT" init -q
git -C "$PROJECT" config user.email e2e@example.invalid
git -C "$PROJECT" config user.name "e2e"
printf 'monitoring e2e\n' > "$PROJECT/README.md"
git -C "$PROJECT" add README.md
git -C "$PROJECT" -c commit.gpgsign=false commit -qm "seed"

# ── 3. Boot the headless backend. `serve` runs the FULL backend (state store,
#       control server, background loops) with no window — the same command
#       surface the desktop app exposes.
log "booting headless backend on port $PORT (isolated HOME=$HOME)"
cd "$PROJECT"
"$BIN" serve --scope loopback --port "$PORT" > "$WORK/serve.log" 2>&1 &
SERVE_PID=$!

# The socket basename differs between debug and release builds, so glob for it.
SOCK=""
for _ in $(seq 1 120); do
  SOCK="$(find "$XDG_RUNTIME_DIR" -maxdepth 1 -name 'codemux*.sock' -print -quit 2>/dev/null || true)"
  [ -n "$SOCK" ] && break
  kill -0 "$SERVE_PID" 2>/dev/null || { err "serve exited early"; cat "$WORK/serve.log"; exit 1; }
  sleep 0.5
done
[ -n "$SOCK" ] || { err "control socket never appeared"; cat "$WORK/serve.log"; exit 1; }
ok "control socket at $SOCK"

# `codemux json <command> <params>` is the generic socket round-trip.
call() {
  local command="$1" params="${2:-{\}}"
  "$BIN" json "$command" "$params" 2>/dev/null
}

# assert_json <label> <json> <python-expr over `d` (the parsed response)>
assert_json() {
  local label="$1" json="$2" expr="$3"
  LABEL="$label" EXPR="$expr" python3 -c '
import json, os, sys
d = json.load(sys.stdin)
label, expr = os.environ["LABEL"], os.environ["EXPR"]
if not d.get("ok"):
    sys.exit("%s: control command failed: %s" % (label, d.get("error")))
if not eval(expr, {"d": d, "data": d.get("data") or {}}):
    sys.exit("%s: assertion failed: %s\n%s" % (label, expr, json.dumps(d, indent=2)))
' <<<"$json" || { err "$label"; exit 1; }
  ok "$label"
}

# ── 4. Create a workspace in the temp project.
log "creating a workspace"
CREATE="$(call create_workspace "{\"path\":\"$PROJECT\"}")"
assert_json "workspace created" "$CREATE" 'data.get("workspace_id")'
WS_ID="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["data"]["workspace_id"])' <<<"$CREATE")"
ok "workspace_id=$WS_ID"

# ── 5. monitor_start — the provider-agnostic signal. No pane_id given, so the
#       backend resolves the workspace's active pane, exactly as an agent
#       calling `codemux monitor start` from inside that pane would.
log "monitor_start"
START="$(call monitor_start "{\"workspace_id\":\"$WS_ID\",\"reason\":\"watching CI\"}")"
assert_json "monitor_start acknowledged" "$START" \
  'data.get("monitoring") is True and data.get("reason") == "watching CI" and data.get("changed") is True'
PANE_ID="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["data"]["pane_id"])' <<<"$START")"
ok "pane_id=$PANE_ID"

# ── 6. The snapshot the sidebar reads must agree — this is the real contract,
#       not the command's own echo.
log "get_app_state reflects the monitoring status"
STATE="$(call get_app_state)"
assert_json "pane reports monitoring" "$STATE" \
  "data['pane_statuses'].get('$PANE_ID') == 'monitoring'"
assert_json "reason is carried for the chat bar" "$STATE" \
  "data.get('manual_monitors', {}).get('$PANE_ID') == 'watching CI'"

log "monitor_status agrees with the snapshot"
assert_json "monitor_status reports monitoring + manual" "$(call monitor_status "{\"pane_id\":\"$PANE_ID\"}")" \
  'data.get("monitoring") is True and data.get("manual") is True and data.get("reason") == "watching CI"'

# Re-asserting the same reason is not a change (the emit-skipping guard).
assert_json "re-asserting the same reason reports no change" \
  "$(call monitor_start "{\"pane_id\":\"$PANE_ID\",\"reason\":\"watching CI\"}")" \
  'data.get("changed") is False'

# ── 7. monitor_stop — the status and the flag both clear.
log "monitor_stop"
assert_json "monitor_stop acknowledged" "$(call monitor_stop "{\"pane_id\":\"$PANE_ID\"}")" \
  'data.get("monitoring") is False and data.get("changed") is True'

STATE="$(call get_app_state)"
assert_json "monitoring status is gone" "$STATE" \
  "data['pane_statuses'].get('$PANE_ID') is None"
assert_json "manual flag is gone" "$STATE" \
  "data.get('manual_monitors', {}).get('$PANE_ID') is None"

assert_json "a second stop is an idempotent no-op" \
  "$(call monitor_stop "{\"pane_id\":\"$PANE_ID\"}")" \
  'data.get("changed") is False'

# ── 8. And it can be turned back on — no latched state left behind.
log "start/stop round-trips cleanly a second time"
assert_json "monitor_start again" "$(call monitor_start "{\"pane_id\":\"$PANE_ID\"}")" \
  'data.get("changed") is True and data.get("reason") is None'
assert_json "monitoring again, with no reason this time" "$(call get_app_state)" \
  "data['pane_statuses'].get('$PANE_ID') == 'monitoring' and data.get('manual_monitors', {}).get('$PANE_ID') is None"
assert_json "monitor_stop again" "$(call monitor_stop "{\"pane_id\":\"$PANE_ID\"}")" \
  'data.get("changed") is True'

log "PASS — the provider-agnostic monitoring path works end to end"
