#!/usr/bin/env bash
# Read-only assertion helpers against a live root.
#
#   assert.sh sql "<query>"                       # sqlite3 -readonly, JSON rows
#   assert.sh conversation <thread_id> [limit]    # `codemux json conversation_read`
#   assert.sh reply <thread_id> <regex>           # an assistant message matches
#   assert.sh no-text <text>                      # text absent from DB and logs
#
# Root: $ORCH_LIVE_ROOT or the newest root. Exit status is the verdict;
# stdout carries the evidence (JSON) for run.mjs to record.
set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
load_env

sql() {
  # mode=ro + -readonly: never write the live store, even by accident.
  sqlite3 -readonly -json "file:$DB?mode=ro" "$1"
}

conversation() { # $1=thread_id [$2=limit]
  cmx conversation_read \
    "$(jq -nc --arg ws "$WORKSPACE_ID" --arg c "$1" --argjson l "${2:-50}" \
      '{workspace_id:$ws, conversation_id:$c, limit:$l}')"
}

cmd="${1:-}"; shift || true
case "$cmd" in
  sql) sql "$1" ;;
  conversation) conversation "$1" "${2:-50}" ;;
  reply)
    page="$(conversation "$1" 50)"
    echo "$page" | jq -e '(.ok // true) == true and ((.data // .) | has("messages"))' >/dev/null \
      || { echo "$page"; fail "conversation_read failed"; }
    match="$(echo "$page" | jq -c --arg re "$2" '
      [(.data // .) | .messages[]? | select(.role == "assistant")
        | select((.content // "") | test($re))] | first // empty')"
    [ -n "$match" ] || { echo "$page" | head -c 4000; echo; fail "no assistant message matching /$2/ in $1"; }
    echo "$match"
    ;;
  no-text)
    hits="$(grep -rlaF -- "$1" "$ROOT/config" "$ROOT/data" "$ROOT/logs" 2>/dev/null || true)"
    [ -z "$hits" ] || { echo "$hits"; fail "found '$1' in the files above"; }
    echo '"absent"'
    ;;
  *) sed -n '2,12p' "$0"; exit 2 ;;
esac
