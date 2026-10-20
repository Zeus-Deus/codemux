# Shared helpers for the live orchestration harness. Sourced, not executed.
#
# Everything a live run touches lives under one root directory
# (~/.cache/codemux-agents/orch-v2/live/<ts>/ by default; never /tmp, whose
# per-user quota is shared by every agent and has corrupted SQLite before).
# `up.sh` writes `<root>/env`; every other script loads it with `load_env`.

HARNESS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$HARNESS_DIR/../../.." && pwd)"
LIVE_BASE="${ORCH_LIVE_BASE:-$HOME/.cache/codemux-agents/orch-v2/live}"
# Where the sandbox sees the user's real ~/.claude. Only Claude's credential
# storage looks there (CLAUDE_SECURESTORAGE_CONFIG_DIR); the sandbox's own
# ~/.claude is private. See README.md "Credentials".
CLAUDE_AUTH_MOUNT=/run/orch-live/claude-auth

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

# Top-level entries Claude's credential storage creates in its storage dir
# (checked in the CLI binary, 2.1.299): the credentials file, the
# `<name>.tmp.<8 hex>` files its atomic writes rename into place, and the
# cross-process refresh lock (a directory) with its owner record. These stay
# writable in the sandbox; everything else in the real ~/.claude is covered.
claude_auth_entry() {
  case "$1" in
    .credentials.json|.credentials.json.tmp.*) return 0 ;;
    .oauth_refresh.lock|.oauth_refresh.lock.owner|.oauth_refresh.lock.owner.tmp.*) return 0 ;;
  esac
  return 1
}

# bwrap arguments for the sandbox the serve process (and every provider CLI it
# spawns) runs in. Allow-list: the host is read-only, /dev, /proc, /tmp,
# /var/tmp and /run are private, HOME is private, and the only writable host
# paths are the run root and the provider credentials. See README.md
# "Isolation" and "Credentials". Prints one argument per line.
sandbox_args() {
  local real_home="$HOME" resolv entry name
  printf '%s\n' \
    --ro-bind / / \
    --dev /dev --tmpfs /dev/shm \
    --proc /proc \
    --tmpfs /tmp --tmpfs /var/tmp \
    --tmpfs /run \
    --unshare-pid --unshare-ipc
  # /etc/resolv.conf usually points into /run (systemd-resolved's stub);
  # re-expose just that directory so DNS keeps working.
  resolv="$(readlink -f /etc/resolv.conf 2>/dev/null || true)"
  case "$resolv" in /run/*) printf '%s\n' --ro-bind "$(dirname "$resolv")" "$(dirname "$resolv")" ;; esac
  [ -n "${SANDBOX_CWD:-}" ] && printf '%s\n' --chdir "$SANDBOX_CWD"
  # A private, persistent HOME at the real path, so CLIs find ~/.claude and
  # ~/.codex where they expect them and restarts keep provider sessions.
  printf '%s\n' --bind "$ROOT/home" "$real_home"
  # Re-expose what lives under the real HOME and the run needs.
  printf '%s\n' --bind "$ROOT" "$ROOT"
  printf '%s\n' --ro-bind "$REPO_DIR" "$REPO_DIR"
  [ -d "$real_home/.local/share/mise" ] && \
    printf '%s\n' --ro-bind "$real_home/.local/share/mise" "$real_home/.local/share/mise"
  # Claude: the REAL ~/.claude directory at $CLAUDE_AUTH_MOUNT, never copied.
  # Claude persists a refreshed token with temp-file + rename and guards the
  # refresh with a lock directory next to it, so the storage dir itself must
  # be shared (a file bind would fail the rename with EBUSY and lose the new
  # refresh token). Every existing entry other than the credential entries is
  # covered read-only and empty; symlinks can't be mounted over, and down.sh
  # checks they are unchanged.
  if [ -d "$real_home/.claude" ]; then
    printf '%s\n' --bind "$real_home/.claude" "$CLAUDE_AUTH_MOUNT"
    for entry in "$real_home/.claude"/* "$real_home/.claude"/.[!.]* "$real_home/.claude"/..?*; do
      [ -e "$entry" ] || [ -L "$entry" ] || continue
      name="${entry##*/}"
      claude_auth_entry "$name" && continue
      if [ -L "$entry" ]; then
        continue
      elif [ -d "$entry" ]; then
        printf '%s\n' --ro-bind "$ROOT/empty/dir" "$CLAUDE_AUTH_MOUNT/$name"
      else
        printf '%s\n' --ro-bind "$ROOT/empty/file" "$CLAUDE_AUTH_MOUNT/$name"
      fi
    done
  fi
  # Codex: a private ~/.codex (from $ROOT/home) with only auth.json bound in,
  # read-write. Codex rewrites auth.json in place (truncate + write), which a
  # file bind-mount supports.
  [ -f "$real_home/.codex/auth.json" ] && \
    printf '%s\n' --bind "$real_home/.codex/auth.json" "$real_home/.codex/auth.json"
  return 0
}

# Environment for processes inside the sandbox. Allow-list only: no DISPLAY /
# WAYLAND_DISPLAY / DBUS_SESSION_BUS_ADDRESS, and none of the caller's
# CODEMUX_* or CLAUDE* variables (a nested CLAUDECODE=1 alone makes Claude
# refuse to run). ORCH_LIVE_RUN_ID marks this run's processes for down.sh.
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
    "CLAUDE_SECURESTORAGE_CONFIG_DIR=$CLAUDE_AUTH_MOUNT" \
    "CODEMUX_CLAUDE_SIDECAR_PATH=$SIDECAR" \
    "CODEMUX_E2E_LOCAL_REMOTE=1" \
    "CODEMUX_DISABLE_PTY_DAEMON=1" \
    "DISABLE_AUTOUPDATER=1" \
    "ORCH_LIVE_RUN_ID=${RUN_ID:-}"
}

# Run a command inside the sandbox (foreground).
in_sandbox() {
  local -a args envs
  mapfile -t args < <(sandbox_args)
  mapfile -t envs < <(sandbox_env)
  bwrap "${args[@]}" -- /usr/bin/env -i "${envs[@]}" "$@"
}

# Start a command in the sandbox in the background, off the caller's terminal.
# $1=log file, $2…=command. Sets SERVE_PID/SERVE_START, bwrap's outer
# process, which exits as soon as the command does, and INIT_PID/INIT_START,
# the PID namespace's init (bwrap's --info-fd "child-pid"). The init keeps
# reaping orphans and only exits once every process in the namespace has
# exited on its own (so namespace teardown never kills a live provider); it is
# what "the sandbox is running" means and what stop_sandbox waits for.
# ORCH_LIVE_RUN_ID on bwrap itself marks both for down.sh.
launch_sandbox() {
  local log="$1" info="$ROOT/logs/sandbox-info.json" i
  shift
  local -a bwrap_args sandbox_envs
  mapfile -t bwrap_args < <(sandbox_args)
  mapfile -t sandbox_envs < <(sandbox_env)
  mkdir -p "$ROOT/logs"
  : >"$info"
  ORCH_LIVE_RUN_ID="$RUN_ID" setsid bwrap --info-fd 3 "${bwrap_args[@]}" -- /usr/bin/env -i "${sandbox_envs[@]}" \
    "$@" >>"$log" 2>&1 </dev/null 3>"$info" &
  SERVE_PID=$!
  SERVE_START="$(proc_start "$SERVE_PID" || true)"
  INIT_PID="" INIT_START=""
  # bwrap writes the info before it runs the command; it never comes if
  # bwrap fails during setup.
  for i in $(seq 1 40); do
    INIT_PID="$(jq -r '."child-pid" // empty' "$info" 2>/dev/null || true)"
    [ -n "$INIT_PID" ] && break
    proc_running "$SERVE_PID" || { INIT_PID="$(jq -r '."child-pid" // empty' "$info" 2>/dev/null || true)"; break; }
    python3 -c 'import time; time.sleep(0.25)'
  done
  [ -z "$INIT_PID" ] || INIT_START="$(proc_start "$INIT_PID" || true)"
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

# Field 22 of /proc/<pid>/stat: start time in clock ticks since boot. With the
# PID it identifies a process uniquely, so a reused PID never matches.
proc_start() {
  local stat
  stat="$(cat "/proc/$1/stat" 2>/dev/null)" || return 1
  stat="${stat##*) }" # drop "pid (comm) "; comm may hold spaces or parens
  # shellcheck disable=SC2086
  set -- $stat
  echo "${20}"
}

# Whether a PID names a running process (exists and is not a zombie).
proc_running() {
  local stat
  [ -n "${1:-}" ] || return 1
  stat="$(cat "/proc/$1/stat" 2>/dev/null)" || return 1
  case "${stat##*) }" in Z*|X*) return 1 ;; esac
}

# Whether a PID carries this run's ORCH_LIVE_RUN_ID in its environment. No
# pipe: under pipefail, `grep -q` exiting early can SIGPIPE the writer and
# turn a match into a spurious "not ours", which made a running sandbox look
# stopped.
has_run_marker() {
  [ -n "${RUN_ID:-}" ] || return 1
  grep -qzxF "ORCH_LIVE_RUN_ID=$RUN_ID" "/proc/$1/environ" 2>/dev/null
}

# The process that stands for the sandbox: its PID namespace's init, or
# bwrap's outer process if bwrap never reported one (it failed during setup).
sandbox_pid() { echo "${INIT_PID:-${SERVE_PID:-}}"; }

# The sandbox this root started is still running: sandbox_pid is still the
# same process (recorded start time, not a zombie) and carries the run
# marker. Prints why not on stderr if the PID now names another process.
sandbox_alive() {
  local pid start
  pid="$(sandbox_pid)"
  if [ -n "${INIT_PID:-}" ]; then start="${INIT_START:-}"; else start="${SERVE_START:-}"; fi
  proc_running "$pid" || return 1
  if [ "$(proc_start "$pid")" != "$start" ] || ! has_run_marker "$pid"; then
    echo "pid $pid is alive but is not the sandbox this root started (start time or run marker differ)" >&2
    return 2
  fi
}

# The `codemux serve` process inside the sandbox (host PID), or empty.
serve_process() {
  local child
  for child in $(descendants "$(sandbox_pid)"); do
    if [ "$(readlink -f "/proc/$child/exe" 2>/dev/null)" = "$BIN" ] && has_run_marker "$child"; then
      echo "$child"
      return
    fi
  done
}

wait_sandbox_gone() { # $1=seconds
  local i
  for i in $(seq 1 $(($1 * 4))); do
    sandbox_alive 2>/dev/null || return 0
    python3 -c 'import time; time.sleep(0.25)'
  done
  return 1
}

# Stop the sandbox. $1=graceful|hard. Graceful: SIGTERM serve, the same path
# as quitting the app. Hard (crash simulation): SIGKILL the serve process
# alone. Either way provider CLIs are then left to exit on their own (stdin
# EOF once serve is gone) and only get SIGTERM if they linger past
# ORCH_LIVE_STOP_GRACE seconds (default 30). Nothing else is ever SIGKILLed:
# Codex rewrites auth.json in place, and a kill mid-refresh would leave the
# user's real credentials truncated. The sandbox counts as stopped only once
# its PID namespace's init has exited, i.e. every process inside is gone.
# Returns non-zero, with the survivors listed, if it outlives all of that.
stop_sandbox() {
  local mode="$1" grace="${ORCH_LIVE_STOP_GRACE:-30}" pid serve child
  sandbox_alive || { [ $? = 2 ] && return 1; note "sandbox not running (pid $(sandbox_pid))"; return 0; }
  pid="$(sandbox_pid)"
  serve="$(serve_process)"
  if [ -n "$serve" ]; then
    if [ "$mode" = hard ]; then
      kill -KILL "$serve" 2>/dev/null || true
      note "SIGKILLed serve (pid $serve) only"
    else
      kill -TERM "$serve" 2>/dev/null || true
    fi
  fi
  if wait_sandbox_gone "$grace"; then return 0; fi
  note "sandbox still running $grace s after serve stopped; SIGTERM to its remaining processes"
  for child in $(descendants "$pid" | tac); do
    kill -TERM "$child" 2>/dev/null || true
  done
  wait_sandbox_gone $((grace / 2 > 1 ? grace / 2 : 1)) && return 0
  echo "FAIL: sandbox (init pid $pid) survived; still running (not SIGKILLed on purpose):" >&2
  for child in $(descendants "$pid"); do
    echo "  $child $(readlink -f "/proc/$child/exe" 2>/dev/null)" >&2
  done
  return 1
}

# ── Real-credential checks (down.sh) ─────────────────────────────────────

# Snapshot of the real ~/.claude top level: one line per entry, symlinks with
# their target. down.sh compares against it.
claude_real_snapshot() {
  local entry name
  [ -d "$HOME/.claude" ] || return 0
  for entry in "$HOME/.claude"/* "$HOME/.claude"/.[!.]* "$HOME/.claude"/..?*; do
    [ -e "$entry" ] || [ -L "$entry" ] || continue
    name="${entry##*/}"
    if [ -L "$entry" ]; then printf 'L\t%s\t%s\n' "$name" "$(readlink "$entry")"; else printf 'E\t%s\n' "$name"; fi
  done | LC_ALL=C sort
}

# The user's real provider credential files down.sh checks.
real_credential_files() {
  printf '%s\n' "$HOME/.codex/auth.json" "$HOME/.claude/.credentials.json"
}

# up.sh records which of them exist, so down.sh can tell a file that was
# never there (skipped) from one that vanished during the run (a failure).
record_real_credentials() {
  local file
  real_credential_files | while IFS= read -r file; do
    if [ -e "$file" ]; then printf '%s\n' "$file"; fi
  done >"$ROOT/credentials.present"
}

# Fails loudly if a real credential file that existed at up.sh is missing or
# is not an intact JSON object, or if the real ~/.claude gained an entry (or
# lost or changed a symlink) since up.sh, other than Claude's own credential
# entries. Never restores anything.
check_real_credentials() {
  local rc=0 snap_rc=0 file before after line
  [ -f "$ROOT/credentials.present" ] || note "$ROOT/credentials.present missing; cannot tell whether a credential file vanished"
  while IFS= read -r file; do
    if [ ! -e "$file" ]; then
      if grep -qxF "$file" "$ROOT/credentials.present" 2>/dev/null; then
        echo "FAIL: $file existed when up.sh ran and is gone now — the user's real login may be broken; do NOT restore it from a copy, re-login instead" >&2
        rc=1
      fi
      continue
    fi
    if [ -s "$file" ] && jq -e 'type == "object" and length > 0' "$file" >/dev/null 2>&1; then
      pass "$file is intact (non-empty JSON object)"
    else
      echo "FAIL: $file is empty or not valid JSON — the user's real login may be broken; do NOT restore it from a copy, re-login instead" >&2
      rc=1
    fi
  done < <(real_credential_files)
  before="$ROOT/claude-real.snapshot"
  if [ -f "$before" ]; then
    after="$(claude_real_snapshot)"
    while IFS= read -r line; do
      [ -n "$line" ] || continue
      claude_auth_entry "$(printf '%s' "$line" | cut -f2)" && continue
      echo "FAIL: real ~/.claude changed during the run: + $line (made by the sandbox or by a host Claude; inspect it)" >&2
      snap_rc=1
    done < <(LC_ALL=C comm -13 "$before" <(printf '%s\n' "$after"))
    while IFS= read -r line; do
      [ -n "$line" ] || continue
      case "$line" in L*) ;; *) continue ;; esac
      echo "FAIL: real ~/.claude symlink removed or changed during the run: - $line" >&2
      snap_rc=1
    done < <(LC_ALL=C comm -23 "$before" <(printf '%s\n' "$after"))
    [ "$snap_rc" = 0 ] && pass "real ~/.claude has no new or changed top-level entries besides Claude's credential entries"
  fi
  [ "$rc" = 0 ] && [ "$snap_rc" = 0 ]
}
