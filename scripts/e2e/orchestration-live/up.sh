#!/usr/bin/env bash
# Bring up an isolated, real Codemux instance for live orchestration tests.
#
#   up.sh [root]
#
# Creates (or reuses, for restart tests) a root under
# ~/.cache/codemux-agents/orch-v2/live/<ts>/, runs `codemux serve --scope
# loopback` inside a bwrap sandbox whose HOME is private but whose provider
# credentials are the user's real files (bind-mounted, never copied), seeds a
# git repo as a workspace, mints a pairing URL and writes everything the other
# scripts need to <root>/env. Prints PAIR_URL=… and ROOT=… on success.
#
# Inputs (build them first; see README.md):
#   src-tauri/target/debug/codemux   debug build (release builds are refused)
#   dist/                            `npm run build` (debug serve loads it)
#   sidecar/claude-agent/dist/codemux-claude-sidecar   `bun run build`
#
# Env knobs: ORCH_LIVE_ROOT (root to use), ORCH_LIVE_PORT (fixed port),
# ORCH_LIVE_SKIP_CLI_PREFLIGHT=1 (skip the paid `claude -p` check).
set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

for tool in bwrap jq sqlite3 strings ss git setsid curl; do
  command -v "$tool" >/dev/null 2>&1 || fail "missing required tool: $tool"
done

BIN="$(readlink -f "${ORCH_LIVE_BIN:-$REPO_DIR/src-tauri/target/debug/codemux}")"
SIDECAR="${CODEMUX_CLAUDE_SIDECAR_PATH:-$REPO_DIR/sidecar/claude-agent/dist/codemux-claude-sidecar}"
[ -x "$BIN" ] || fail "debug binary not found at $BIN (~/.cache/codemux-agents/cargo-slot cargo build -j 2 --manifest-path src-tauri/Cargo.toml)"
# A release binary uses the real store and socket names; the XDG isolation is
# the only thing between this run and the user's live instance. The `|| true`
# absorbs strings' SIGPIPE under pipefail.
[ "$(strings -a "$BIN" 2>/dev/null | grep -c -m1 'codemux-dev' || true)" = "1" ] \
  || fail "$BIN does not look like a debug build; refusing to run"
[ -f "$REPO_DIR/dist/index.html" ] || fail "dist/ missing (npm run build)"
[ -x "$SIDECAR" ] && [ -s "$SIDECAR" ] \
  || fail "Claude sidecar missing at $SIDECAR (cd sidecar/claude-agent && bun install && bun run build)"

# ── Root ────────────────────────────────────────────────────────────────
ROOT="${1:-${ORCH_LIVE_ROOT:-$LIVE_BASE/$(date +%Y%m%d-%H%M%S)}}"
mkdir -p "$ROOT"
ROOT="$(readlink -f "$ROOT")"
case "$ROOT" in /tmp/*) fail "refusing a root under /tmp (shared quota)";; esac
REUSE=0
if [ -f "$ROOT/env" ]; then
  REUSE=1
  # shellcheck disable=SC1091
  OLD_PID="$(. "$ROOT/env"; echo "${SERVE_PID:-}")"
  if [ -n "$OLD_PID" ] && kill -0 "$OLD_PID" 2>/dev/null; then
    fail "$ROOT already has a running serve (pid $OLD_PID); run down.sh first"
  fi
fi
mkdir -p "$ROOT"/{config,data,cache,state,runtime,home,repo,shots,logs,bin}
chmod 700 "$ROOT/runtime"

# Local mode for the instance: no Codemux account, same as a user who chose
# "Continue without an account".
mkdir -p "$ROOT/config/codemux-dev/chatgpt"
[ -f "$ROOT/config/codemux-dev/chatgpt/state.json" ] \
  || echo '{"local_workbench":true}' >"$ROOT/config/codemux-dev/chatgpt/state.json"

# CLI symlinks (see resolve_cli).
for cli in claude codex node npm npx; do
  target="$(resolve_cli "$cli" || true)"
  if [ -n "$target" ]; then ln -sfn "$target" "$ROOT/bin/$cli"; else note "$cli not found; skipped"; fi
done
case "$(readlink -f "$ROOT/bin/claude" 2>/dev/null)" in
  "$HOME"/.local/share/mise/*|/usr/*|"") ;;
  *) note "claude resolves outside ~/.local/share/mise and /usr; add a --ro-bind for it in lib.sh" ;;
esac

# Private HOME contents.
[ -f "$ROOT/home/.gitconfig" ] || cat >"$ROOT/home/.gitconfig" <<'EOF'
[user]
	name = Orchestration Live Test
	email = orch-live@example.invalid
[init]
	defaultBranch = main
EOF
mkdir -p "$ROOT/home/.codex"
# Mount point for the real auth.json; never holds credentials itself.
[ -e "$ROOT/home/.codex/auth.json" ] || : >"$ROOT/home/.codex/auth.json"
[ -f "$ROOT/home/.codex/config.toml" ] || : >"$ROOT/home/.codex/config.toml"
for sub in $CLAUDE_OVERLAY_DIRS; do mkdir -p "$ROOT/claude-overlay/$sub"; done
[ -f "$ROOT/claude-overlay/settings.json" ] || echo '{}' >"$ROOT/claude-overlay/settings.json"
[ -f "$ROOT/claude-overlay/settings.local.json" ] || echo '{}' >"$ROOT/claude-overlay/settings.local.json"
[ -f "$ROOT/claude-overlay/CLAUDE.md" ] || : >"$ROOT/claude-overlay/CLAUDE.md"
# Claude's per-user state file lives in HOME itself; start it empty-but-valid
# so the CLI doesn't go looking for the user's backups.
[ -f "$ROOT/home/.claude.json" ] || echo '{"hasCompletedOnboarding":true}' >"$ROOT/home/.claude.json"

# Seed repo: one commit, used as the workspace every scenario starts in.
if [ ! -d "$ROOT/repo/.git" ]; then
  git -C "$ROOT/repo" init -q -b main
  printf '# Orchestration live test repo\n' >"$ROOT/repo/README.md"
  git -C "$ROOT/repo" -c user.name="Orchestration Live Test" \
    -c user.email=orch-live@example.invalid add README.md
  git -C "$ROOT/repo" -c user.name="Orchestration Live Test" \
    -c user.email=orch-live@example.invalid commit -q -m "chore: seed"
fi

# ── Credential pre-flight ────────────────────────────────────────────────
# Proves the sandbox sees the user's real credential files (same device and
# inode: bind-mounted, not copied) and that Claude's rename-based refresh can
# land in ~/.claude. Then a real CLI round trip, unless skipped.
preflight_log="$ROOT/logs/preflight.log"
: >"$preflight_log"
same_file() { # $1=path → PASS if the sandbox sees the host's inode
  local host inside
  host="$(stat -c '%d:%i' "$1")"
  inside="$(in_sandbox stat -c '%d:%i' "$1" 2>>"$preflight_log")" || fail "sandbox cannot stat $1 (see $preflight_log)"
  [ "$host" = "$inside" ] || fail "$1 inside the sandbox is not the host file ($inside vs $host)"
  pass "sandbox sees the real $1 (bind-mounted, inode $host)"
}
CLAUDE_CREDS=0 CODEX_CREDS=0
if [ -f "$HOME/.claude/.credentials.json" ]; then same_file "$HOME/.claude/.credentials.json"; CLAUDE_CREDS=1; fi
if [ -f "$HOME/.codex/auth.json" ]; then same_file "$HOME/.codex/auth.json"; CODEX_CREDS=1; fi
if [ "$CLAUDE_CREDS" = 1 ]; then
  probe=".orch-live-rename-probe-$$"
  in_sandbox sh -c "echo probe >\"\$HOME/.claude/$probe.tmp\" && mv \"\$HOME/.claude/$probe.tmp\" \"\$HOME/.claude/$probe\"" \
    2>>"$preflight_log" || fail "rename inside the sandbox's ~/.claude failed (see $preflight_log)"
  [ -f "$HOME/.claude/$probe" ] || fail "a rename inside the sandbox's ~/.claude did not reach the host"
  rm -f "$HOME/.claude/$probe"
  pass "a temp-file + rename in ~/.claude reaches the host (Claude token refresh path)"
fi
if [ "${ORCH_LIVE_SKIP_CLI_PREFLIGHT:-0}" != 1 ] && [ "$REUSE" = 0 ]; then
  if [ "$CLAUDE_CREDS" = 1 ] && [ -e "$ROOT/bin/claude" ]; then
    out="$(in_sandbox timeout 120 sh -c 'cd "$1" && exec claude -p "Reply with exactly: PREFLIGHT_OK" --model claude-haiku-4-5' _ "$ROOT/repo" \
      2>>"$preflight_log" || true)"
    echo "claude: $out" >>"$preflight_log"
    case "$out" in *PREFLIGHT_OK*) pass "claude -p works inside the sandbox";;
      *) fail "claude -p inside the sandbox did not answer (see $preflight_log)";; esac
  fi
  if [ "$CODEX_CREDS" = 1 ] && [ -e "$ROOT/bin/codex" ]; then
    out="$(in_sandbox timeout 60 codex login status 2>&1 || true)"
    echo "codex: $out" >>"$preflight_log"
    case "$out" in *"Logged in"*) pass "codex is signed in inside the sandbox";;
      *) note "codex login status inside the sandbox: $out (Codex scenarios will fail)";; esac
  fi
fi

# ── Serve ─────────────────────────────────────────────────────────────────
PORT="${ORCH_LIVE_PORT:-}"
if [ -z "$PORT" ]; then
  for candidate in $(seq 4400 4499); do
    if ! ss -ltnH 2>/dev/null | awk '{print $4}' | grep -q ":$candidate\$"; then PORT="$candidate"; break; fi
  done
fi
[ -n "$PORT" ] || fail "no free port in 4400-4499"

# cwd = the seed repo, so the instance's default workspace is the repo.
mapfile -t BWRAP_ARGS < <(SANDBOX_CWD="$ROOT/repo" sandbox_args)
mapfile -t SANDBOX_ENV < <(sandbox_env)
# setsid: the sandbox gets its own process group, so down.sh can stop exactly
# this tree. --unshare-pid: when the sandbox's init exits, the kernel kills
# every provider CLI it still holds.
setsid bwrap "${BWRAP_ARGS[@]}" -- /usr/bin/env -i "${SANDBOX_ENV[@]}" \
  "$BIN" serve --scope loopback --port "$PORT" \
  >>"$ROOT/logs/serve.log" 2>&1 </dev/null &
SERVE_PID=$!
echo "$SERVE_PID" >"$ROOT/serve.pid"

for _ in $(seq 1 120); do
  if cmx status '{}' >/dev/null 2>&1; then break; fi
  kill -0 "$SERVE_PID" 2>/dev/null || { tail -30 "$ROOT/logs/serve.log" >&2; fail "serve exited early"; }
  sleep 0.5
done
cmx status '{}' >/dev/null 2>&1 || fail "control socket never came up (see $ROOT/logs/serve.log)"
[ -S "$ROOT/runtime/codemux-dev.sock" ] || fail "control socket not under $ROOT/runtime"
[ -e "$ROOT/config/codemux-dev/codemux.db" ] || fail "store not under $ROOT/config"
for _ in $(seq 1 60); do
  curl -fsS "http://127.0.0.1:$PORT/api/health" >/dev/null 2>&1 && break
  sleep 0.5
done
curl -fsS "http://127.0.0.1:$PORT/api/health" >/dev/null 2>&1 || fail "web remote not answering on $PORT"
pass "serve up (pid $SERVE_PID, port $PORT)"

# ── Workspace + pairing ─────────────────────────────────────────────────────
WORKSPACE_ID="$(cmx get_app_state '{}' | jq -r --arg p "$ROOT/repo" \
  '(.data // .) | .workspaces[]? | select(.cwd==$p or .project_root==$p) | .workspace_id' | head -1)"
if [ -z "$WORKSPACE_ID" ] || [ "$WORKSPACE_ID" = null ]; then
  WORKSPACE_ID="$(cmx create_workspace "{\"path\":\"$ROOT/repo\"}" | jq -re '.data.workspace_id // .workspace_id')"
fi
[ -n "$WORKSPACE_ID" ] || fail "workspace creation failed"
pass "workspace $WORKSPACE_ID at $ROOT/repo"

BASE_URL="http://127.0.0.1:$PORT"
# The minted URL names the machine's preferred endpoint (e.g. a tailnet
# name) even though serve only listens on loopback; keep just the token.
PAIR_TOKEN="$(cmx web_remote_pair '{"name":"orch-live"}' | jq -re '(.data // .).token')" \
  || fail "pairing failed"
PAIR_URL="$BASE_URL/#pair=$PAIR_TOKEN"

cat >"$ROOT/env" <<EOF
# Written by up.sh $(date -Is). Sourced by down.sh, assert.sh, drive.mjs, run.mjs.
ROOT='$ROOT'
REPO_DIR='$REPO_DIR'
BIN='$BIN'
SIDECAR='$SIDECAR'
SERVE_PID='$SERVE_PID'
PORT='$PORT'
BASE_URL='$BASE_URL'
PAIR_URL='$PAIR_URL'
WORKSPACE_ID='$WORKSPACE_ID'
REPO='$ROOT/repo'
DB='$ROOT/config/codemux-dev/codemux.db'
BROWSER_SESSION='orch-live-$(basename "$ROOT")'
EOF
echo "ROOT=$ROOT"
echo "PAIR_URL=$PAIR_URL"
