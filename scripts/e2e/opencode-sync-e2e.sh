#!/usr/bin/env bash
# End-to-end harness for OpenCode conversation sync across push/pull (issue #16).
#
# Stands up a Docker container as a real SSH host with `opencode` installed,
# seeds an isolated "laptop" OpenCode DB plus an unrelated session on the host,
# then runs the env-gated `opencode_sync_roundtrip` integration test which
# drives the REAL `ssh::sync_opencode_session` / `pull_opencode_session`
# functions through a 3-cycle push→continue→pull→continue round-trip and
# asserts the host's unrelated session is never clobbered.
#
# Requires: docker, sqlite3, python3, an `opencode` binary on PATH, and a real
# OpenCode DB (~/.local/share/opencode/opencode.db) to source fixture sessions
# from (read-only). Run from the repo root.
#
# Usage:  bash scripts/e2e/opencode-sync-e2e.sh
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
IMAGE="codemux-opencode-e2e"
CONTAINER="codemux-opencode-e2e-run"
PORT="${CMX_OC_E2E_PORT:-2299}"
SSH_ALIAS="cmx-oc-e2e"
WORK="$(mktemp -d /tmp/cmx-oc-e2e.XXXXXX)"
SSH_CONFIG="${HOME}/.ssh/config"
MARKER_BEGIN="# >>> codemux-opencode-e2e >>>"
MARKER_END="# <<< codemux-opencode-e2e <<<"
REAL_DB="${HOME}/.local/share/opencode/opencode.db"
OPENCODE_BIN="$(command -v opencode)"

log() { printf '\n\033[1;36m[e2e]\033[0m %s\n' "$*"; }
err() { printf '\n\033[1;31m[e2e:ERROR]\033[0m %s\n' "$*" >&2; }

cleanup() {
  log "cleanup"
  docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
  # Strip our ssh config block.
  if [ -f "$SSH_CONFIG" ] && grep -qF "$MARKER_BEGIN" "$SSH_CONFIG"; then
    sed -i "/$MARKER_BEGIN/,/$MARKER_END/d" "$SSH_CONFIG"
  fi
  rm -rf "$WORK"
}
trap cleanup EXIT

command -v docker >/dev/null || { err "docker not found"; exit 1; }
[ -n "$OPENCODE_BIN" ] || { err "opencode not on PATH"; exit 1; }
[ -f "$REAL_DB" ] || { err "no OpenCode DB at $REAL_DB to source fixtures from"; exit 1; }

# ── 1. Pick two distinct real sessions with messages: A (sync target), B (unrelated).
log "selecting fixture sessions from $REAL_DB"
mapfile -t SIDS < <(sqlite3 -readonly "$REAL_DB" \
  "SELECT session_id FROM message GROUP BY session_id HAVING COUNT(*) >= 8 \
   ORDER BY COUNT(*) DESC LIMIT 2;")
SID_A="${SIDS[0]:-}"
SID_B="${SIDS[1]:-}"
[ -n "$SID_A" ] && [ -n "$SID_B" ] || { err "need >=2 sessions with >=8 messages in the DB"; exit 1; }
log "session A=$SID_A  unrelated B=$SID_B"

# ── 2. Build fixture bundles (read-only export of the real DB).
log "exporting fixture bundles"
"$OPENCODE_BIN" export "$SID_A" > "$WORK/full.json" 2>/dev/null
"$OPENCODE_BIN" export "$SID_B" > "$WORK/unrelated.json" 2>/dev/null
python3 - "$WORK/full.json" "$WORK/short.json" <<'PY'
import json, sys
full = json.load(open(sys.argv[1]))
short = dict(info=full["info"], messages=full["messages"][:4])
json.dump(short, open(sys.argv[2], "w"))
print(f"full={len(full['messages'])} short={len(short['messages'])} msgs")
PY

# ── 3. Ephemeral SSH key + Docker build context.
log "building Docker SSH host image (opencode binary copied in)"
ssh-keygen -t ed25519 -N '' -f "$WORK/key" -q
cp "$OPENCODE_BIN" "$WORK/opencode"
cp "$WORK/key.pub" "$WORK/authorized_keys"
cat > "$WORK/Dockerfile" <<'DOCKER'
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends openssh-server ca-certificates \
    && rm -rf /var/lib/apt/lists/* && mkdir -p /run/sshd
RUN useradd -m -s /bin/bash tester && mkdir -p /home/tester/.ssh && chmod 700 /home/tester/.ssh
COPY opencode /usr/local/bin/opencode
RUN chmod +x /usr/local/bin/opencode
COPY authorized_keys /home/tester/.ssh/authorized_keys
RUN chown -R tester:tester /home/tester/.ssh && chmod 600 /home/tester/.ssh/authorized_keys
EXPOSE 22
CMD ["/usr/sbin/sshd","-D","-e"]
DOCKER
docker build -q -t "$IMAGE" "$WORK" >/dev/null

# ── 4. Run container, wire up an ssh-config alias so bare `ssh <alias>` works
#       (the production code shells `ssh <target>` with no -p / -i).
docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
docker run -d --name "$CONTAINER" -p "127.0.0.1:${PORT}:22" "$IMAGE" >/dev/null
mkdir -p "${HOME}/.ssh"; touch "$SSH_CONFIG"; chmod 600 "$SSH_CONFIG"
sed -i "/$MARKER_BEGIN/,/$MARKER_END/d" "$SSH_CONFIG" 2>/dev/null || true
cat >> "$SSH_CONFIG" <<CFG
$MARKER_BEGIN
Host $SSH_ALIAS
    HostName 127.0.0.1
    Port $PORT
    User tester
    IdentityFile $WORK/key
    IdentitiesOnly yes
    StrictHostKeyChecking no
    UserKnownHostsFile /dev/null
    LogLevel ERROR
$MARKER_END
CFG

log "waiting for sshd"
for i in $(seq 1 30); do
  if ssh -o ConnectTimeout=2 "$SSH_ALIAS" 'echo ready' 2>/dev/null | grep -q ready; then break; fi
  sleep 1
  [ "$i" = 30 ] && { err "sshd never came up"; docker logs "$CONTAINER" | tail; exit 1; }
done
log "remote opencode (via login shell): $(ssh "$SSH_ALIAS" 'bash -lc "opencode --version"' 2>/dev/null)"

# ── 5. Run the real integration test against the container.
export CODEMUX_E2E_SSH_HOST="$SSH_ALIAS"
export CMX_OC_E2E_LAPTOP_XDG="$WORK/laptop-xdg"
export CMX_OC_E2E_LOCAL_WS="$WORK/local-ws"
export CMX_OC_E2E_REMOTE_WS="/tmp/cmx-oc-e2e-ws"
export CMX_OC_E2E_BUNDLE_SHORT="$WORK/short.json"
export CMX_OC_E2E_BUNDLE_FULL="$WORK/full.json"
export CMX_OC_E2E_BUNDLE_UNRELATED="$WORK/unrelated.json"
export CMX_OC_E2E_SID_A="$SID_A"
export CMX_OC_E2E_SID_B="$SID_B"
mkdir -p "$CMX_OC_E2E_LAPTOP_XDG" "$CMX_OC_E2E_LOCAL_WS"

log "running cargo integration test (opencode_sync_roundtrip)"
cargo test --manifest-path "$REPO_ROOT/src-tauri/Cargo.toml" \
  --test opencode_sync_roundtrip -- --nocapture --test-threads=1

log "PASS — OpenCode conversation sync round-trip verified end-to-end"
