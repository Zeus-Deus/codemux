#!/usr/bin/env bash
# Stop the instance up.sh started, and the harness's browser session, then
# check the user's real credentials are intact.
#
#   down.sh [--hard] [root]
#
# Default: SIGTERM the codemux serve process inside the sandbox (graceful
# shutdown, the same path as quitting the app). --hard: SIGKILL the serve
# process alone (crash simulation for restart scenarios). Either way the
# provider CLIs are left to exit on their own once serve is gone and only get
# SIGTERM if they linger; nothing else is ever SIGKILLed (see stop_sandbox in
# lib.sh). The sandbox counts as stopped once its PID namespace's init has
# exited, which happens only after every process inside has. Only processes that match the recorded PID, start time and run
# marker are signalled. Logs, the store and provider sessions stay under the
# root, so `up.sh <root>` restarts on them.
#
# Exits non-zero if the sandbox would not stop, if a real credential file that
# existed at up.sh is gone or no longer a non-empty JSON object, or if the real ~/.claude gained entries
# other than Claude's own credential files during the run. It never repairs
# or restores anything.
set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

HARD=0
if [ "${1:-}" = "--hard" ]; then HARD=1; shift; fi
load_env "${1:-}"

if [ -n "${BROWSER_SESSION:-}" ] && command -v agent-browser >/dev/null 2>&1; then
  agent-browser --session "$BROWSER_SESSION" close >/dev/null 2>&1 || true
fi

rc=0
if [ "$HARD" = 1 ]; then mode=hard; else mode=graceful; fi
if stop_sandbox "$mode"; then
  pass "sandbox stopped ($mode)"
else
  rc=1
fi
check_real_credentials || rc=1
exit "$rc"
