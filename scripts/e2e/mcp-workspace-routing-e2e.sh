#!/usr/bin/env bash
# E2E: MCP tools/call workspace routing → agent browser session binding.
#
# Reproduces the shared `codemux mcp` child scenario (no CODEMUX_WORKSPACE_ID
# env, cwd pointing at the wrong place) and asserts that a `_meta`
# "codemux/workspace_id" on tools/call binds the agent browser session to the
# intended workspace, overriding both the cwd fallback and the env var.
#
# Isolation: the serve process and every CLI invocation below run with their
# own XDG_CONFIG_HOME / XDG_DATA_HOME / XDG_RUNTIME_DIR under a throwaway
# root (the app resolves its store through `dirs::config_dir()` /
# `dirs::data_dir()` and its control socket through XDG_RUNTIME_DIR), so
# neither the release store (~/.config/codemux) nor the dev store
# (~/.config/codemux-dev, which `npm run tauri:dev` shares) is read or
# written. Only a DEBUG build is accepted as a second guard: its socket and
# store names differ from the release build's.
#
# Usage: scripts/e2e/mcp-workspace-routing-e2e.sh [path-to-debug-codemux]
set -euo pipefail

for tool in jq strings ss; do
  command -v "$tool" >/dev/null 2>&1 || { echo "FAIL: missing required tool: $tool" >&2; exit 1; }
done

BIN="${1:-src-tauri/target/debug/codemux}"
BIN="$(readlink -f "$BIN" 2>/dev/null || echo "$BIN")"
ROOT="$(mktemp -d /tmp/codemux-mcp-routing-e2e.XXXXXX)"
WS_A="$ROOT/ws-a"
WS_B="$ROOT/ws-b"
SERVE_PID=""
SERVE_LOG="$ROOT/serve.log"

# Every codemux invocation from here on (serve, `json`, the `mcp` child)
# sees only the throwaway root.
export XDG_CONFIG_HOME="$ROOT/config"
export XDG_DATA_HOME="$ROOT/data"
export XDG_STATE_HOME="$ROOT/state"
export XDG_CACHE_HOME="$ROOT/cache"
export XDG_RUNTIME_DIR="$ROOT/run"
mkdir -p "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_CACHE_HOME" "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"

fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "PASS: $*"; }

cleanup() {
  # Close what we opened (browser sessions, then the workspaces themselves)
  # while the serve is still answering, then stop only the serve process we
  # started ourselves. Every step is best-effort so a dead serve can't wedge
  # teardown.
  if [ -n "$SERVE_PID" ] && kill -0 "$SERVE_PID" 2>/dev/null; then
    for ws in "${WSID_A:-}" "${WSID_B:-}"; do
      [ -n "$ws" ] || continue
      timeout 15 "$BIN" json browser_automation \
        "{\"workspace_id\":\"$ws\",\"action\":{\"kind\":\"close\"}}" >/dev/null 2>&1 || true
      timeout 15 "$BIN" json close_workspace "{\"workspace_id\":\"$ws\"}" >/dev/null 2>&1 || true
    done
    kill "$SERVE_PID" 2>/dev/null || true
    for _ in $(seq 1 20); do kill -0 "$SERVE_PID" 2>/dev/null || break; sleep 0.25; done
    kill -9 "$SERVE_PID" 2>/dev/null || true
  fi
  rm -rf "$ROOT"
}
trap cleanup EXIT

[ -x "$BIN" ] || fail "debug binary not found at $BIN (build with: cargo build --manifest-path src-tauri/Cargo.toml)"
# Refuse anything that doesn't look like a debug build — a release binary
# uses different socket/store names, and the XDG isolation above is the only
# thing standing between this test and a live instance.
# (the socket name is assembled at runtime, so match the debug-only
# "codemux-dev" basename constant rather than the joined filename; the
# `|| true` absorbs strings' SIGPIPE when grep stops at the first match,
# which pipefail would otherwise misread as "not found")
DEBUG_MARKER=$(strings -a "$BIN" 2>/dev/null | grep -c -m1 'codemux-dev' || true)
[ "$DEBUG_MARKER" = "1" ] \
  || fail "$BIN does not look like a debug build; refusing to run"

# Pick a free loopback port for the web-remote listener the serve binds.
PORT="${E2E_SERVE_PORT:-}"
if [ -z "$PORT" ]; then
  for candidate in $(seq 4477 4577); do
    if ! ss -ltn 2>/dev/null | grep -q ":$candidate "; then PORT="$candidate"; break; fi
  done
fi
[ -n "$PORT" ] || fail "no free port in 4477-4577"

mkdir -p "$WS_A" "$WS_B"

# ── Boot an isolated headless backend ──────────────────────────────────────
# The pty-daemon would outlive the serve process; this test never opens a
# terminal, so suppress it entirely.
CODEMUX_DISABLE_PTY_DAEMON=1 "$BIN" serve --scope loopback --port "$PORT" >"$SERVE_LOG" 2>&1 &
SERVE_PID=$!
for _ in $(seq 1 40); do
  if "$BIN" json status '{}' >/dev/null 2>&1; then break; fi
  kill -0 "$SERVE_PID" 2>/dev/null || { cat "$SERVE_LOG" >&2; fail "serve exited early"; }
  sleep 0.5
done
"$BIN" json status '{}' >/dev/null 2>&1 || fail "control socket never came up"
pass "headless debug backend up (pid $SERVE_PID, port $PORT)"
# Prove the isolation actually took: the store and socket must live under
# our root, not the user's.
[ -S "$XDG_RUNTIME_DIR/codemux-dev.sock" ] || fail "control socket not under $XDG_RUNTIME_DIR"
[ -e "$XDG_CONFIG_HOME/codemux-dev/codemux.db" ] || fail "sqlite store not under $XDG_CONFIG_HOME"
pass "store and socket isolated under $ROOT"

# ── Two workspaces ─────────────────────────────────────────────────────────
WSID_A=$("$BIN" json create_workspace "{\"path\":\"$WS_A\"}" | jq -re '.data.workspace_id // .workspace_id')
WSID_B=$("$BIN" json create_workspace "{\"path\":\"$WS_B\"}" | jq -re '.data.workspace_id // .workspace_id')
[ -n "$WSID_A" ] && [ -n "$WSID_B" ] || fail "workspace creation failed"
pass "workspaces created: A=$WSID_A ($WS_A), B=$WSID_B ($WS_B)"

# ── Drive a `codemux mcp` child exactly like the shared registry child ─────
# cwd = ws-a dir, env WSID unset (case 1) or pointing at ws-a (case 2), so
# every legacy signal says "workspace A". The _meta must win and route to B.
mcp_call() { # $1=cwd $2=env_wsid("" for unset) $3=params_json
  # Drive the child over a fifo so stdin can be closed the moment the
  # tools/call response lands, instead of holding it open on a timer.
  local cwd="$1" env_wsid="$2" params="$3"
  local fifo="$ROOT/mcp-in.$RANDOM" out="$ROOT/mcp-out.$RANDOM" pid
  mkfifo "$fifo"
  (
    cd "$cwd"
    if [ -n "$env_wsid" ]; then
      CODEMUX_WORKSPACE_ID="$env_wsid" timeout 90 "$BIN" mcp <"$fifo" >"$out" 2>/dev/null &
    else
      env -u CODEMUX_WORKSPACE_ID timeout 90 "$BIN" mcp <"$fifo" >"$out" 2>/dev/null &
    fi
    echo $! >"$out.pid"
  )
  exec 3>"$fifo" # rendezvous with the child's stdin open
  printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"e2e","version":"0"}}}' >&3
  printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}' >&3
  printf '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":%s}\n' "$params" >&3
  for _ in $(seq 1 160); do
    grep -q '"id":2' "$out" 2>/dev/null && break
    sleep 0.5
  done
  exec 3>&- # EOF -> child shuts down
  pid=$(cat "$out.pid")
  for _ in $(seq 1 20); do kill -0 "$pid" 2>/dev/null || break; sleep 0.25; done
  kill "$pid" 2>/dev/null || true
  rm -f "$fifo" "$out.pid"
  grep -m1 '"id":2' "$out" || fail "no tools/call response from codemux mcp child"
}

session_ws_of() { # $1=workspace_id → prints matching session workspace_id or empty
  "$BIN" json get_app_state '{}' \
    | jq -r --arg ws "$1" '(.data // .) | .agent_browser_sessions[]? | select(.workspace_id==$ws) | .workspace_id' \
    | head -1
}

# Case 1 — regression baseline: no env, no _meta, cwd=ws-a → cwd fallback → A.
mcp_call "$WS_A" "" \
  '{"name":"browser_navigate","arguments":{"url":"http://127.0.0.1:9/nope"}}' >/dev/null
[ "$(session_ws_of "$WSID_A")" = "$WSID_A" ] || fail "case 1: cwd fallback did not bind to workspace A"
[ -z "$(session_ws_of "$WSID_B")" ] || fail "case 1: unexpected session for workspace B"
pass "case 1: cwd fallback still binds to A (baseline)"

# Case 2 — the fix: env AND cwd both say A, _meta says B → must bind to B.
mcp_call "$WS_A" "$WSID_A" \
  "{\"name\":\"browser_navigate\",\"arguments\":{\"url\":\"http://127.0.0.1:9/nope\"},\"_meta\":{\"codemux/workspace_id\":\"$WSID_B\"}}" >/dev/null
[ "$(session_ws_of "$WSID_B")" = "$WSID_B" ] || fail "case 2: _meta workspace id did not override env+cwd (session for B missing)"
pass "case 2: _meta codemux/workspace_id routes the session to B over env+cwd"

# Case 3 — a stale/unknown _meta id must not reach the browser layer (it
# would mint a phantom `ws-<id>` session); the control layer drops it and
# falls through to the cwd hint (ws-a). The handler logs the id it settled
# on, so the serve log is the witness.
mcp_call "$WS_A" "" \
  '{"name":"browser_navigate","arguments":{"url":"http://127.0.0.1:9/nope"},"_meta":{"codemux/workspace_id":"ws-does-not-exist"}}' >/dev/null
grep -q 'workspace_id=ws-does-not-exist' "$SERVE_LOG" \
  && fail "case 3: unknown _meta id was forwarded to the browser handler"
[ "$(grep -c "workspace_id=$WSID_A cwd_resolved=true" "$SERVE_LOG")" -ge 2 ] \
  || fail "case 3: unknown _meta id did not fall through to the cwd hint"
[ -z "$(session_ws_of "ws-does-not-exist")" ] || fail "case 3: phantom session recorded"
pass "case 3: unknown _meta workspace id is dropped in favour of the cwd hint"

echo "ALL PASS"
