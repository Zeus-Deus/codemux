#!/usr/bin/env bash
# ToS boundary static check.
#
# The sidecar MUST NOT:
#
#   1. Read credential files like `.claude.json` or `~/.anthropic/`.
#   2. Make HTTP requests to Anthropic domains.
#   3. Spawn the `claude` binary directly — all inference must go
#      through `@anthropic-ai/claude-agent-sdk`'s `query()`.
#   4. Peek at Anthropic credentials via environment variables.
#
# Rule 3 has exactly one exception: `src/auth-probe.ts` is allowed to
# spawn `claude --version` and `claude auth status`. That file (and
# that file only) is allow-listed below.
#
# Exit code: 0 if all checks pass, 1 otherwise. Intended to run on
# every `bun test` and as a standalone CI step.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SRC_DIR="$(cd "$SCRIPT_DIR/.." && pwd)/src"

# File whose `spawn claude` / `exec claude` patterns are legitimate
# (auth probes). Any OTHER file matching those patterns is a
# violation.
SPAWN_ALLOWLIST=(
  "$SRC_DIR/auth-probe.ts"
)

FAIL=0

# Build a `grep -v` expression that excludes allow-listed files.
exclude_allowlist() {
  local pattern matches filtered
  pattern="$1"
  matches="$2"
  filtered="$matches"
  for allowed in "${SPAWN_ALLOWLIST[@]}"; do
    filtered=$(echo "$filtered" | grep -vF "$allowed" || true)
  done
  echo "$filtered"
}

report() {
  local description="$1"
  local matches="$2"
  if [ -n "$matches" ]; then
    echo "FAIL: $description"
    echo "$matches" | sed 's/^/    /'
    FAIL=1
  fi
}

# --- Rule 1: no credential file reads --------------------------------------

check_credentials() {
  local matches
  matches=$(grep -rn -E '\.claude\.json' "$SRC_DIR" || true)
  report "Sidecar references .claude.json (ToS boundary violation)" "$matches"

  matches=$(grep -rn -E '\.anthropic/' "$SRC_DIR" || true)
  report "Sidecar references ~/.anthropic/ directory" "$matches"
}

# --- Rule 2: no Anthropic API URLs -----------------------------------------

check_api_urls() {
  local matches
  matches=$(grep -rn -E 'api\.anthropic\.com' "$SRC_DIR" || true)
  report "Sidecar references api.anthropic.com" "$matches"

  matches=$(grep -rn -E 'anthropic\.com' "$SRC_DIR" || true)
  report "Sidecar references anthropic.com" "$matches"
}

# --- Rule 3: no direct claude spawns outside auth-probe.ts ----------------

check_claude_spawns() {
  local matches filtered
  matches=$(grep -rn -E "(spawn|exec|execFile|execSync|spawnSync|fork).*['\"]claude['\"]" "$SRC_DIR" || true)
  filtered=$(exclude_allowlist "claude spawn" "$matches")
  report "Sidecar spawns 'claude' outside auth-probe.ts (must go through SDK)" "$filtered"
}

# --- Rule 4: no Anthropic credential env-var reads -------------------------

check_env_peeks() {
  local matches
  matches=$(grep -rn -E 'ANTHROPIC_API_KEY' "$SRC_DIR" || true)
  report "Sidecar reads ANTHROPIC_API_KEY (SDK handles auth)" "$matches"

  matches=$(grep -rn -E 'CLAUDE_CODE_OAUTH_TOKEN' "$SRC_DIR" || true)
  report "Sidecar reads CLAUDE_CODE_OAUTH_TOKEN" "$matches"
}

check_credentials
check_api_urls
check_claude_spawns
check_env_peeks

if [ $FAIL -ne 0 ]; then
  echo ""
  echo "ToS boundary check FAILED. See docs/features/agent-chat.md for the rules."
  echo "Forbidden patterns must not appear in the sidecar except inside the"
  echo "explicit auth-probe allowlist in this script."
  exit 1
fi

echo "ToS boundary check passed."
