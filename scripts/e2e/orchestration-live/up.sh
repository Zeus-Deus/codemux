#!/usr/bin/env bash
# Bring up an isolated, real Codemux instance for live orchestration tests.
#
#   up.sh [root]
#
# Creates (or reuses, for restart tests) a root under
# ~/.cache/codemux-agents/orch-v2/live/<ts>/, runs `codemux serve --scope
# loopback` inside a bwrap sandbox (read-only host, private HOME, /tmp and
# /run; provider credentials are the user's real files, bind-mounted, never
# copied), proves that isolation, seeds a git repo as a workspace, mints a
# pairing URL and writes everything the other scripts need to <root>/env.
# Prints PAIR_URL=… and ROOT=… on success. If startup fails or is
# interrupted, the sandbox it started is stopped again.
#
# Inputs (build them first; see README.md):
#   src-tauri/target/debug/codemux   debug build (release builds are refused)
#   dist/                            `npm run build` (debug serve loads it)
#   sidecar/claude-agent/dist/codemux-claude-sidecar   `bun run build`
#
# Env knobs: ORCH_LIVE_ROOT (root to use), ORCH_LIVE_PORT (fixed port, or
# a comma list tried in order),
# ORCH_LIVE_SKIP_CLI_PREFLIGHT=1 (skip the paid `claude -p` check).
set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

for tool in bwrap jq sqlite3 strings ss git setsid curl getent comm tac; do
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
case "$ROOT" in /tmp/*|/var/tmp/*) fail "refusing a root under /tmp (shared quota)";; esac
REUSE=0
if [ -f "$ROOT/env" ]; then
  REUSE=1
  # shellcheck disable=SC1091
  if (. "$ROOT/env"; sandbox_alive 2>/dev/null); then
    fail "$ROOT already has a running sandbox; run down.sh first"
  fi
fi
mkdir -p "$ROOT"/{config,data,cache,state,runtime,home,repo,shots,logs,bin,empty/dir}
chmod 700 "$ROOT/runtime"
# Read-only stand-ins mounted over the real ~/.claude's non-credential entries.
[ -f "$ROOT/empty/file" ] || : >"$ROOT/empty/file"
chmod 444 "$ROOT/empty/file"; chmod 555 "$ROOT/empty/dir"
# A fresh id per start: down.sh only signals processes that carry it.
RUN_ID="$(cat /proc/sys/kernel/random/uuid)"
SERVE_PID="" SERVE_START="" PORT="" BASE_URL="" PAIR_URL="" WORKSPACE_ID=""

# Shutdown metadata, written as soon as a sandbox exists so down.sh (and the
# failure trap below) can always stop it.
write_env() {
  cat >"$ROOT/env.tmp" <<EOF
# Written by up.sh $(date -Is). Sourced by down.sh, assert.sh, drive.mjs, run.mjs.
ROOT='$ROOT'
REPO_DIR='$REPO_DIR'
BIN='$BIN'
SIDECAR='$SIDECAR'
RUN_ID='$RUN_ID'
SERVE_PID='$SERVE_PID'
SERVE_START='$SERVE_START'
PORT='$PORT'
BASE_URL='$BASE_URL'
PAIR_URL='$PAIR_URL'
WORKSPACE_ID='$WORKSPACE_ID'
REPO='$ROOT/repo'
DB='$ROOT/config/codemux-dev/codemux.db'
BROWSER_SESSION='orch-live-$(basename "$ROOT")'
EOF
  mv "$ROOT/env.tmp" "$ROOT/env"
}

STARTED=0
on_exit() {
  local rc=$?
  [ "$STARTED" = 1 ] && return 0
  [ -n "$SERVE_PID" ] || return 0
  echo "-- startup did not finish (exit $rc); stopping the sandbox it started" >&2
  stop_sandbox graceful || echo "FAIL: could not stop sandbox pid $SERVE_PID; run down.sh $ROOT" >&2
}
trap on_exit EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

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
mkdir -p "$ROOT/home/.codex" "$ROOT/home/.claude"
# Mount point for the real auth.json; never holds credentials itself.
[ -e "$ROOT/home/.codex/auth.json" ] || : >"$ROOT/home/.codex/auth.json"
[ -f "$ROOT/home/.codex/config.toml" ] || : >"$ROOT/home/.codex/config.toml"
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

# down.sh compares the real ~/.claude against this.
claude_real_snapshot >"$ROOT/claude-real.snapshot"

# ── Pre-flight ───────────────────────────────────────────────────────────
preflight_log="$ROOT/logs/preflight.log"
: >"$preflight_log"

# Isolation: host paths, desktop services and the user's real Codemux
# sockets are out of reach; every writable mount is one of ours.
probe="orch-live-probe-$RUN_ID"
gitdir="$(git -C "$REPO_DIR" rev-parse --path-format=absolute --git-common-dir 2>/dev/null || true)"
ro_paths=(/ /usr /etc "$REPO_DIR")
[ -n "$gitdir" ] && ro_paths+=("$gitdir")
[ -n "$gitdir" ] && [ "$(readlink -f "$gitdir")" != "$gitdir" ] && ro_paths+=("$(readlink -f "$gitdir")")
writable="$ROOT:$HOME:$HOME/.codex/auth.json:$CLAUDE_AUTH_MOUNT:/tmp:/var/tmp:/run:/dev/:/proc/"
iso_out="$(in_sandbox env ORCH_LIVE_WRITABLE="$writable" \
  bash "$HARNESS_DIR/isolation-check.sh" "$(id -u)" "$probe" "$CLAUDE_AUTH_MOUNT" "${ro_paths[@]}" 2>&1)" \
  && iso_rc=0 || iso_rc=$?
printf '%s\n' "$iso_out" | tee -a "$preflight_log"
[ "$iso_rc" = 0 ] || fail "sandbox isolation check failed (see above)"
for dir in /tmp /var/tmp; do
  [ ! -e "$dir/$probe" ] || { rm -f "$dir/$probe"; fail "a file written to the sandbox's $dir reached the host's"; }
  pass "the sandbox's $dir is not the host's"
done
# The profile dirs the CLIs see are the private ones (only single entries of
# the real ones are bound in, below).
for dir in "$HOME/.claude" "$HOME/.codex"; do
  [ -d "$dir" ] || continue
  inside="$(in_sandbox stat -c '%d:%i' "$dir" 2>>"$preflight_log")" || fail "sandbox cannot stat $dir"
  [ "$inside" != "$(stat -c '%d:%i' "$dir")" ] || fail "the sandbox's $dir is the real one"
  [ "$inside" = "$(stat -c '%d:%i' "$ROOT/home/${dir##*/}")" ] || fail "the sandbox's $dir is not $ROOT/home/${dir##*/}"
  pass "the sandbox's $dir is the private $ROOT/home/${dir##*/}"
done

# Credentials: the sandbox sees the user's real files (same device and
# inode: bind-mounted, not copied), and Claude's rename-based refresh can land
# in its storage dir.
same_file() { # $1=host path $2=path inside the sandbox
  local host inside
  host="$(stat -c '%d:%i' "$1")"
  inside="$(in_sandbox stat -c '%d:%i' "$2" 2>>"$preflight_log")" || fail "sandbox cannot stat $2 (see $preflight_log)"
  [ "$host" = "$inside" ] || fail "$2 inside the sandbox is not the host's $1 ($inside vs $host)"
  pass "sandbox sees the real $1 at $2 (bind-mounted, inode $host)"
}
CLAUDE_CREDS=0 CODEX_CREDS=0
if [ -f "$HOME/.claude/.credentials.json" ]; then
  same_file "$HOME/.claude/.credentials.json" "$CLAUDE_AUTH_MOUNT/.credentials.json"; CLAUDE_CREDS=1
fi
if [ -f "$HOME/.codex/auth.json" ]; then same_file "$HOME/.codex/auth.json" "$HOME/.codex/auth.json"; CODEX_CREDS=1; fi
if [ "$CLAUDE_CREDS" = 1 ]; then
  rprobe=".orch-live-rename-probe-$$"
  in_sandbox sh -c 'echo probe >"$1/$2.tmp" && mv "$1/$2.tmp" "$1/$2"' _ "$CLAUDE_AUTH_MOUNT" "$rprobe" \
    2>>"$preflight_log" || fail "rename inside the sandbox's Claude storage dir failed (see $preflight_log)"
  [ -f "$HOME/.claude/$rprobe" ] || fail "a rename inside the sandbox's Claude storage dir did not reach the host"
  rm -f "$HOME/.claude/$rprobe"
  pass "a temp-file + rename in the Claude storage dir reaches the host (token refresh path)"
fi
if [ "${ORCH_LIVE_SKIP_CLI_PREFLIGHT:-0}" != 1 ] && [ "$REUSE" = 0 ]; then
  if [ "$CLAUDE_CREDS" = 1 ] && [ -e "$ROOT/bin/claude" ]; then
    out="$(in_sandbox timeout 120 sh -c 'cd "$1" && exec claude -p "Reply with exactly: PREFLIGHT_OK" --model claude-haiku-4-5' _ "$ROOT/repo" \
      2>>"$preflight_log" || true)"
    echo "claude: $out" >>"$preflight_log"
    case "$out" in *PREFLIGHT_OK*) pass "claude -p works inside the sandbox (credentials via CLAUDE_SECURESTORAGE_CONFIG_DIR)";;
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
# Picking a free port and binding it can race with another process, so the
# bind is verified (serve's own status, and the listener's owner) and retried
# on another port.
port_candidates() {
  if [ -n "${ORCH_LIVE_PORT:-}" ]; then tr ',' '\n' <<<"$ORCH_LIVE_PORT"; return; fi
  local taken start i p
  taken="$(ss -ltnH 2>/dev/null | awk '{print $4}' | sed 's/.*://' | sort -u)"
  start=$((4400 + RANDOM % 100))
  for i in $(seq 0 99); do
    p=$((4400 + (start - 4400 + i) % 100))
    grep -qx "$p" <<<"$taken" || echo "$p"
  done | head -5
}

launch_serve() { # $1=port
  # cwd = the seed repo, so the instance's default workspace is the repo.
  local -a bwrap_args sandbox_envs
  mapfile -t bwrap_args < <(SANDBOX_CWD="$ROOT/repo" sandbox_args)
  mapfile -t sandbox_envs < <(sandbox_env)
  # setsid: off the caller's terminal. --unshare-pid: the sandbox's init
  # waits for every process inside before it exits. ORCH_LIVE_RUN_ID on
  # bwrap itself lets down.sh recognise it.
  ORCH_LIVE_RUN_ID="$RUN_ID" setsid bwrap "${bwrap_args[@]}" -- /usr/bin/env -i "${sandbox_envs[@]}" \
    "$BIN" serve --scope loopback --port "$1" \
    >>"$ROOT/logs/serve.log" 2>&1 </dev/null &
  SERVE_PID=$!
  SERVE_START="$(proc_start "$SERVE_PID" || true)"
  PORT="$1"
  BASE_URL="http://127.0.0.1:$PORT"
  write_env
}

# 0 = up and listening on $PORT; 1 = failed (reason on stdout).
serve_ready() {
  local status serve
  for _ in $(seq 1 120); do
    cmx status '{}' >/dev/null 2>&1 && break
    kill -0 "$SERVE_PID" 2>/dev/null || { echo "serve exited"; return 1; }
    sleep 0.5
  done
  cmx status '{}' >/dev/null 2>&1 || { echo "control socket never came up"; return 1; }
  [ -S "$ROOT/runtime/codemux-dev.sock" ] || fail "control socket not under $ROOT/runtime"
  [ -e "$ROOT/config/codemux-dev/codemux.db" ] || fail "store not under $ROOT/config"
  for _ in $(seq 1 60); do
    status="$(cmx web_remote_status '{}' | jq -c '.data // .' 2>/dev/null || true)"
    if [ -n "$(jq -r '.bind_error // empty' <<<"$status" 2>/dev/null)" ]; then
      echo "bind failed: $(jq -r .bind_error <<<"$status")"; return 1
    fi
    [ "$(jq -r '.running' <<<"$status" 2>/dev/null)" = true ] && [ "$(jq -r '.port' <<<"$status")" = "$PORT" ] && break
    sleep 0.5
  done
  serve="$(serve_process)"
  [ -n "$serve" ] || { echo "serve process not found in the sandbox"; return 1; }
  # The port must be served by OUR serve, not by whoever else may hold it.
  ss -ltnpH "sport = :$PORT" 2>/dev/null | grep -q "pid=$serve," \
    || { echo "port $PORT is not held by serve (pid $serve)"; return 1; }
  curl -fsS "$BASE_URL/api/health" >/dev/null 2>&1 || { echo "web remote not answering on $PORT"; return 1; }
}

UP=0
for candidate in $(port_candidates); do
  log_offset="$(stat -c %s "$ROOT/logs/serve.log" 2>/dev/null || echo 0)"
  launch_serve "$candidate"
  if why="$(serve_ready)"; then UP=1; break; fi
  kill -0 "$SERVE_PID" 2>/dev/null || why="serve exited ($why)"
  note "serve on port $candidate: $why"
  stop_sandbox graceful || fail "could not stop the failed sandbox (pid $SERVE_PID)"
  SERVE_PID="" SERVE_START=""
  write_env
  # Only a lost port race is worth another port.
  case "$why" in "bind failed"*|"port $candidate is not held"*) continue ;; esac
  if tail -c +"$((log_offset + 1))" "$ROOT/logs/serve.log" | grep -qiE 'address (already )?in use|EADDRINUSE'; then continue; fi
  tail -30 "$ROOT/logs/serve.log" >&2
  fail "serve did not come up (see $ROOT/logs/serve.log)"
done
[ "$UP" = 1 ] || fail "serve could not bind any candidate port"
pass "serve up (sandbox pid $SERVE_PID, port $PORT)"

# ── Workspace + pairing ─────────────────────────────────────────────────────
WORKSPACE_ID="$(cmx get_app_state '{}' | jq -r --arg p "$ROOT/repo" \
  '(.data // .) | .workspaces[]? | select(.cwd==$p or .project_root==$p) | .workspace_id' | head -1)"
if [ -z "$WORKSPACE_ID" ] || [ "$WORKSPACE_ID" = null ]; then
  WORKSPACE_ID="$(cmx create_workspace "{\"path\":\"$ROOT/repo\"}" | jq -re '.data.workspace_id // .workspace_id')"
fi
[ -n "$WORKSPACE_ID" ] || fail "workspace creation failed"
pass "workspace $WORKSPACE_ID at $ROOT/repo"

# The minted URL names the machine's preferred endpoint (e.g. a tailnet
# name) even though serve only listens on loopback; keep just the token.
PAIR_TOKEN="$(cmx web_remote_pair '{"name":"orch-live"}' | jq -re '(.data // .).token')" \
  || fail "pairing failed"
PAIR_URL="$BASE_URL/#pair=$PAIR_TOKEN"

write_env
STARTED=1
echo "ROOT=$ROOT"
echo "PAIR_URL=$PAIR_URL"
