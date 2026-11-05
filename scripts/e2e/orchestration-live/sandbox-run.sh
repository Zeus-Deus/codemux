#!/usr/bin/env bash
# Run one command inside a live root's sandbox, in the foreground, with stdio
# passed through, so it can host an MCP stdio server or run an outside agent.
#
#   sandbox-run.sh [--root DIR] [--cwd DIR] [--timeout SEC] [--env KEY=VALUE]… -- CMD ARGS…
#
# The command gets the same mounts, environment allow-list and isolation as
# serve (lib.sh sandbox_args / sandbox_env), so it reaches only the isolated
# control socket and the credential binds; isolation-check.sh proved the real
# sockets are out of reach. It runs in a PID namespace of its own, beside
# serve's, so Codemux treats it as an outside process, not one of its agents.
#
# There is no --die-with-parent: that SIGKILLs a provider mid-refresh (README,
# "Shutdown never SIGKILLs a provider"). The inner `timeout -s TERM` (default
# 600 s) is the only stop; callers end a stdio server by closing its stdin.
set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

usage="usage: sandbox-run.sh [--root DIR] [--cwd DIR] [--timeout SEC] [--env KEY=VALUE]... -- CMD ARGS..."
root="" cwd="" seconds=600
extra=()
while [ $# -gt 0 ]; do
  case "$1" in
    --root) root="${2:?$usage}"; shift 2 ;;
    --cwd) cwd="${2:?$usage}"; shift 2 ;;
    --timeout) seconds="${2:?$usage}"; shift 2 ;;
    --env)
      case "${2:-}" in [A-Za-z_]*=*) extra+=("$2") ;; *) fail "--env needs KEY=VALUE ($usage)" ;; esac
      shift 2 ;;
    --) shift; break ;;
    *) fail "unknown argument $1 ($usage)" ;;
  esac
done
[ $# -gt 0 ] || fail "no command ($usage)"
case "$seconds" in '' | *[!0-9]*) fail "--timeout needs whole seconds ($usage)" ;; esac
load_env "$root"
cwd="${cwd:-$ROOT}"
[ -d "$cwd" ] || fail "--cwd $cwd is not a directory"

SANDBOX_CWD="$cwd" in_sandbox /usr/bin/env "${extra[@]}" timeout -s TERM "$seconds" "$@"
