# Shared helpers for the live orchestration harness. Sourced, not executed.
#
# Everything a live run touches lives under one root directory
# (~/.cache/codemux-agents/orch-v2/live/<ts>/ by default; never /tmp, whose
# per-user quota is shared by every agent and has corrupted SQLite before).
# `up.sh` writes `<root>/env`; every other script loads it with `load_env`.

HARNESS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$HARNESS_DIR/../../.." && pwd)"
LIVE_BASE="${ORCH_LIVE_BASE:-$HOME/.cache/codemux-agents/orch-v2/live}"
# ~/.claude subdirectories replaced by run-local dirs inside the sandbox:
# transcripts, per-PID session registry (sandbox PIDs would collide with host
# PIDs), backups, and the user's own plugins/skills (determinism).
CLAUDE_OVERLAY_DIRS="projects sessions todos file-history session-env shell-snapshots tasks backups plugins skills"

fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "PASS: $*"; }
note() { echo "-- $*"; }

# Resolve the root a command acts on: an explicit argument, $ORCH_LIVE_ROOT,
# or the newest root under $LIVE_BASE.
resolve_root() {
  local root="${1:-${ORCH_LIVE_ROOT:-}}"
  if [ -z "$root" ]; then
    root="$(ls -1d "$LIVE_BASE"/2* 2>/dev/null | sort | tail -1 || true)"
  fi
  [ -n "$root" ] && [ -d "$root" ] || fail "no live root found (pass one, or set ORCH_LIVE_ROOT)"
  readlink -f "$root"
}

load_env() {
  ROOT="$(resolve_root "${1:-}")"
  [ -f "$ROOT/env" ] || fail "$ROOT/env missing; run up.sh first"
  # shellcheck disable=SC1091
  . "$ROOT/env"
}

# The isolated XDG layout. The serve process and every `codemux json` call
# must agree on it: the control socket lives in XDG_RUNTIME_DIR and the store
# in XDG_CONFIG_HOME.
xdg_env() {
  printf '%s\n' \
    "XDG_CONFIG_HOME=$ROOT/config" \
    "XDG_DATA_HOME=$ROOT/data" \
    "XDG_CACHE_HOME=$ROOT/cache" \
    "XDG_STATE_HOME=$ROOT/state" \
    "XDG_RUNTIME_DIR=$ROOT/runtime"
}

# `codemux json <cmd> <args>` against the isolated instance. `env -i` keeps
# this shell's own CODEMUX_*/CLAUDE_* variables (which point at the instance
# the calling agent runs in) from leaking into the call.
cmx() {
  local -a envs
  mapfile -t envs < <(xdg_env)
  # The CLI's debug banner goes to stderr; keep it out of the caller's output.
  env -i HOME="$HOME" PATH="/usr/bin:/bin" "${envs[@]}" "$BIN" json "$@" 2>>"$ROOT/logs/cli.log"
}

# Real CLI binaries, resolved once on the host. The ~/.local/bin entries are
# mise shims that run `mise use -g`, which would write into the isolated XDG
# config and try to install tools there, so the sandbox gets symlinks to the
# installed binaries instead.
resolve_cli() { # $1=name → absolute path or empty
  local name="$1" path=""
  if command -v mise >/dev/null 2>&1; then
    path="$(mise which "$name" 2>/dev/null || true)"
  fi
  [ -n "$path" ] || path="$(command -v "$name" 2>/dev/null || true)"
  [ -n "$path" ] && readlink -f "$path"
}

# bwrap arguments for the sandbox the serve process (and every provider CLI it
# spawns) runs in. See README.md "Credentials" for why each mount is shaped
# the way it is. Prints one argument per line.
sandbox_args() {
  local real_home="$HOME"
  printf '%s\n' --dev-bind / / --proc /proc --unshare-pid
  [ -n "${SANDBOX_CWD:-}" ] && printf '%s\n' --chdir "$SANDBOX_CWD"
  # A private, persistent HOME at the real path, so CLIs find ~/.claude and
  # ~/.codex where they expect them and restarts keep provider sessions.
  printf '%s\n' --bind "$ROOT/home" "$real_home"
  # Re-expose what lives under the real HOME and the run needs.
  printf '%s\n' --bind "$ROOT" "$ROOT"
  printf '%s\n' --ro-bind "$REPO_DIR" "$REPO_DIR"
  [ -d "$real_home/.local/share/mise" ] && \
    printf '%s\n' --ro-bind "$real_home/.local/share/mise" "$real_home/.local/share/mise"
  # Claude: the REAL ~/.claude directory, read-write, never copied. Claude
  # persists a refreshed token with temp-file + rename, and a rename onto a
  # file bind-mount fails with EBUSY: the new refresh token would be lost and
  # the user's real login revoked. Binding the directory keeps the rename and
  # Claude's own cross-process write lock working. Run-specific state is
  # overlaid with dirs under the root so nothing test-made lands in the
  # user's history, and user settings/hooks/instructions don't leak in.
  if [ -d "$real_home/.claude" ]; then
    printf '%s\n' --bind "$real_home/.claude" "$real_home/.claude"
    local sub
    for sub in $CLAUDE_OVERLAY_DIRS; do
      [ -d "$real_home/.claude/$sub" ] || continue
      printf '%s\n' --bind "$ROOT/claude-overlay/$sub" "$real_home/.claude/$sub"
    done
    for sub in settings.json settings.local.json CLAUDE.md; do
      local dest="$real_home/.claude/$sub"
      [ -e "$dest" ] || continue
      if [ -L "$dest" ]; then
        # bwrap cannot mount onto a symlink. A link into the rest of the real
        # HOME already dangles under the private HOME; otherwise cover the
        # file it points at.
        dest="$(readlink -f "$dest")"
        case "$dest" in "$real_home"/.claude/*|"$REPO_DIR"/*|"$ROOT"/*) ;; "$real_home"/*) continue ;; esac
      fi
      printf '%s\n' --ro-bind "$ROOT/claude-overlay/$sub" "$dest"
    done
  fi
  # Codex: an isolated ~/.codex (from $ROOT/home) with only auth.json bound
  # in, read-write. Codex rewrites auth.json in place (truncate + write), which
  # a file bind-mount supports.
  [ -f "$real_home/.codex/auth.json" ] && \
    printf '%s\n' --bind "$real_home/.codex/auth.json" "$real_home/.codex/auth.json"
  return 0
}

# Environment for processes inside the sandbox. Allow-list only: no DISPLAY /
# WAYLAND_DISPLAY (never an X server), and none of the caller's CODEMUX_* or
# CLAUDE* variables (a nested CLAUDECODE=1 alone makes Claude refuse to run).
sandbox_env() {
  xdg_env
  printf '%s\n' \
    "HOME=$HOME" \
    "USER=${USER:-$(id -un)}" \
    "LOGNAME=${USER:-$(id -un)}" \
    "LANG=${LANG:-C.UTF-8}" \
    "TERM=xterm-256color" \
    "SHELL=/bin/bash" \
    "PATH=$ROOT/bin:/usr/local/bin:/usr/bin:/bin" \
    "CODEMUX_CLAUDE_SIDECAR_PATH=$SIDECAR" \
    "CODEMUX_E2E_LOCAL_REMOTE=1" \
    "CODEMUX_DISABLE_PTY_DAEMON=1" \
    "DISABLE_AUTOUPDATER=1"
}

# Run a command inside the sandbox (foreground).
in_sandbox() {
  local -a args envs
  mapfile -t args < <(sandbox_args)
  mapfile -t envs < <(sandbox_env)
  bwrap "${args[@]}" -- /usr/bin/env -i "${envs[@]}" "$@"
}

# Every live descendant of a PID, children first-level then deeper. Walks
# /proc/<pid>/task/*/children, so it only ever names processes we started.
descendants() {
  local pid="$1" child tid
  for tid in /proc/"$pid"/task/*; do
    [ -r "$tid/children" ] || continue
    for child in $(cat "$tid/children" 2>/dev/null); do
      echo "$child"
      descendants "$child"
    done
  done
}
