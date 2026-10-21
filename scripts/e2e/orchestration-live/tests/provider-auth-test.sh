#!/usr/bin/env bash
# Exercise atomic auth mounts with synthetic credentials, never a real login.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BASE="${ORCH_LIVE_TEST_BASE:-$HOME/.cache/codemux-agents/orch-v2/tests}"
mkdir -p "$BASE"
T="$(mktemp -d "$BASE/provider-auth.XXXXXX")"
export HOME="$T/real-home" ORCH_LIVE_PROVIDERS=claude,codex,opencode,cursor,grok,hermes
. "$HERE/../lib.sh"
ROOT="$T/run" RUN_ID=synthetic-auth SIDECAR=/usr/bin/true
mkdir -p "$ROOT"/{home,config/cursor,data/opencode,runtime,private,empty/dir,logs}
: >"$ROOT/empty/file"
for source in "$HOME/.local/share/opencode" "$HOME/.config/cursor" "$HOME/.grok" "$HOME/.hermes"; do
  mkdir -p "$source/history"
  echo '{"fixture":"initial"}' >"$source/auth.json"
  echo host-config >"$source/config.yaml"
  echo host-history >"$source/history/entry"
done
extra_auth_snapshot >"$ROOT/extra-auth.snapshot"
record_real_credentials
for pair in "opencode|$HOME/.local/share/opencode|$ROOT/data/opencode" "cursor|$HOME/.config/cursor|$ROOT/config/cursor" "grok|$HOME/.grok|$HOME/.grok" "hermes|$HOME/.hermes|$HOME/.hermes"; do
  IFS='|' read -r provider source target <<<"$pair"
  inside="$(in_sandbox stat -c '%d:%i' "$target/auth.json")"
  [ "$inside" = "$(stat -c '%d:%i' "$source/auth.json")" ] || fail "$provider auth is not the same file"
  in_sandbox sh -c 'printf '\''{"fixture":"refreshed"}\n'\'' >"$1/auth.json.tmp.test"; mv "$1/auth.json.tmp.test" "$1/auth.json"; echo private-config >"$1/config.yaml"; echo private-history >"$1/history/entry"' _ "$target"
  jq -e '.fixture=="refreshed"' "$source/auth.json" >/dev/null
  [ "$(cat "$source/config.yaml")" = host-config ] || fail "$provider changed the host config"
  [ "$(cat "$source/history/entry")" = host-history ] || fail "$provider changed the host history"
  pass "$provider atomic auth refresh reaches the original while config/history stay private"
done
check_real_credentials
pass provider-auth-test
