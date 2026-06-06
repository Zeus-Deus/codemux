#!/usr/bin/env bash
# Verification tier (a) for OpenCode conversation sync (issue #16): run the
# real sync against a GENUINELY-RUN opencode session (real model turns), and
# prove the pushed session RESUMES ON THE HOST with full conversation context.
#
# Unlike opencode-sync-e2e.sh (which seeds the laptop by importing a bundle),
# this:
#   1. runs `opencode` for real in an isolated-but-authenticated laptop dir to
#      create a session that knows a secret code (never touches the real DB);
#   2. stands up a Docker SSH host with opencode + the user's auth copied in;
#   3. runs the env-gated `opencode_real_roundtrip` test, which pushes the real
#      session, does a real `opencode --session <id>` turn ON THE HOST that must
#      recall the secret, pulls the continuation back, and checks the host's
#      unrelated session is untouched.
#
# Uses a FREE opencode model (`opencode/*-free`) so it costs nothing. Requires:
# docker, sqlite3, an authenticated `opencode` (auth.json/account.json), a real
# OpenCode DB to source an unrelated session from. Run from the repo root.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
IMAGE="codemux-opencode-e2e"
CONTAINER="codemux-opencode-real-run"
PORT="${CMX_OC_E2E_PORT:-2299}"
SSH_ALIAS="cmx-oc-real-e2e"
WORK="$(mktemp -d /tmp/cmx-oc-real.XXXXXX)"
SSH_CONFIG="${HOME}/.ssh/config"
MARKER_BEGIN="# >>> codemux-opencode-real-e2e >>>"
MARKER_END="# <<< codemux-opencode-real-e2e <<<"
REAL_DB="${HOME}/.local/share/opencode/opencode.db"
REAL_DATA="${HOME}/.local/share/opencode"
OPENCODE_BIN="$(command -v opencode)"
MODEL="${CMX_OC_E2E_MODEL:-opencode/deepseek-v4-flash-free}"
SECRET="ZEBRA-7731"

log() { printf '\n\033[1;36m[real-e2e]\033[0m %s\n' "$*"; }
err() { printf '\n\033[1;31m[real-e2e:ERROR]\033[0m %s\n' "$*" >&2; }

cleanup() {
  log "cleanup"
  docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
  if [ -f "$SSH_CONFIG" ] && grep -qF "$MARKER_BEGIN" "$SSH_CONFIG"; then
    sed -i "/$MARKER_BEGIN/,/$MARKER_END/d" "$SSH_CONFIG"
  fi
  rm -rf "$WORK"   # wipes the copied auth + ephemeral key
}
trap cleanup EXIT

command -v docker >/dev/null || { err "docker not found"; exit 1; }
[ -n "$OPENCODE_BIN" ] || { err "opencode not on PATH"; exit 1; }
[ -f "$REAL_DATA/auth.json" ] || { err "no opencode auth at $REAL_DATA/auth.json"; exit 1; }

LAPTOP_XDG="$WORK/laptop"
CONFIG="$WORK/config"
LOCAL_WS="$WORK/ws"
mkdir -p "$LAPTOP_XDG/opencode" "$CONFIG/opencode" "$LOCAL_WS"
# Isolated laptop DB, but with auth copied so real turns work. The real DB is
# NEVER touched.
cp "$REAL_DATA/auth.json" "$REAL_DATA/account.json" "$LAPTOP_XDG/opencode/" 2>/dev/null || \
  cp "$REAL_DATA/auth.json" "$LAPTOP_XDG/opencode/"

# ── 1. Create a genuinely-run session that knows the secret (real model turn).
log "creating a genuinely-run OpenCode session (model: $MODEL)"
( cd "$LOCAL_WS" && XDG_DATA_HOME="$LAPTOP_XDG" XDG_CONFIG_HOME="$CONFIG" \
    "$OPENCODE_BIN" run --pure -m "$MODEL" \
    "Remember this secret code for later: $SECRET. Reply with just: OK" >/dev/null 2>&1 )
SID_A="$(sqlite3 -readonly "$LAPTOP_XDG/opencode/opencode.db" \
  "SELECT id FROM session WHERE directory='$LOCAL_WS' AND parent_id IS NULL ORDER BY time_updated DESC LIMIT 1;")"
[ -n "$SID_A" ] || { err "no session created on the laptop"; exit 1; }
A_MSGS="$(sqlite3 -readonly "$LAPTOP_XDG/opencode/opencode.db" "SELECT COUNT(*) FROM message WHERE session_id='$SID_A';")"
log "laptop session A=$SID_A ($A_MSGS msgs, knows secret $SECRET)"

# ── 2. An unrelated session B sourced from the real DB (read-only export).
SID_B="$(sqlite3 -readonly "$REAL_DB" \
  "SELECT session_id FROM message GROUP BY session_id HAVING COUNT(*)>=8 ORDER BY COUNT(*) DESC LIMIT 1;")"
[ -n "$SID_B" ] || { err "no unrelated session available in the real DB"; exit 1; }
"$OPENCODE_BIN" export "$SID_B" > "$WORK/unrelated.json" 2>/dev/null
log "unrelated host session B=$SID_B"

# ── 3. Build + run the Docker SSH host with opencode + auth.
log "building/launching Docker SSH host (opencode + auth)"
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
docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
docker run -d --name "$CONTAINER" -p "127.0.0.1:${PORT}:22" "$IMAGE" >/dev/null

# Copy auth into the container so the host can do a real model turn (removed
# with the container at cleanup; never baked into the image layers).
docker exec "$CONTAINER" mkdir -p /home/tester/.local/share/opencode
docker cp "$LAPTOP_XDG/opencode/auth.json" "$CONTAINER:/home/tester/.local/share/opencode/auth.json"
[ -f "$LAPTOP_XDG/opencode/account.json" ] && \
  docker cp "$LAPTOP_XDG/opencode/account.json" "$CONTAINER:/home/tester/.local/share/opencode/account.json"
docker exec "$CONTAINER" chown -R tester:tester /home/tester/.local

# ssh-config alias so bare `ssh <alias>` works (production shells `ssh <target>`).
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
  ssh -o ConnectTimeout=2 "$SSH_ALIAS" 'echo ready' 2>/dev/null | grep -q ready && break
  sleep 1; [ "$i" = 30 ] && { err "sshd never came up"; exit 1; }
done

# Seed the unrelated session B on the host (different dir from the workspace).
ssh "$SSH_ALIAS" "cat > /tmp/cmx-oc-real-unrelated.json" < "$WORK/unrelated.json"
ssh "$SSH_ALIAS" "bash -lc 'mkdir -p /tmp/cmx-oc-real-unrelated && cd /tmp/cmx-oc-real-unrelated && opencode import /tmp/cmx-oc-real-unrelated.json'" >/dev/null 2>&1
log "host opencode: $(ssh "$SSH_ALIAS" 'bash -lc "opencode --version"' 2>/dev/null); seeded unrelated B"

# ── 4. Run the real-session round-trip test.
export CODEMUX_E2E_SSH_HOST="$SSH_ALIAS"
export CMX_OC_E2E_REAL=1
export CMX_OC_E2E_LAPTOP_XDG="$LAPTOP_XDG"
export XDG_CONFIG_HOME="$CONFIG"   # keep test-spawned opencode off the user's MCP config
export CMX_OC_E2E_LOCAL_WS="$LOCAL_WS"
export CMX_OC_E2E_REMOTE_WS="/tmp/cmx-oc-real-ws"
export CMX_OC_E2E_SID_A="$SID_A"
export CMX_OC_E2E_SID_B="$SID_B"
export CMX_OC_E2E_MODEL="$MODEL"
export CMX_OC_E2E_SECRET="$SECRET"

log "running cargo integration test (opencode_real_roundtrip)"
cargo test --manifest-path "$REPO_ROOT/src-tauri/Cargo.toml" \
  --test opencode_real_roundtrip -- --nocapture --test-threads=1

log "PASS — genuinely-run OpenCode session pushed, resumed-on-host with context, and pulled back"
