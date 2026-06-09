#!/usr/bin/env bash
# End-to-end harness for issue #78: headless daemon worktree creation
# runs setup scripts + worktree includes (desktop parity).
#
# Stands up a clean containerized host (no desktop, no dev environment),
# mounts the built `codemux-remote` binary, seeds a git repo with a
# committed `.codemux/config.json` setup script and a gitignored `.env`,
# starts `codemux-remote serve`, then drives the REAL authed HTTP tool
# surface (`tools/call worktree_create`) exactly like the MCP bridge
# does — and asserts on the container's filesystem that:
#   1. the worktree was created at the canonical path,
#   2. the setup script ran with CODEMUX_BRANCH/CODEMUX_PORT injected,
#   3. the gitignored .env was copied from the main checkout.
#
# Requires: docker, python3, and a debug build of codemux-remote
# (`cargo build --manifest-path src-tauri/Cargo.toml --bin codemux-remote`).
# Run from the repo root:  bash scripts/e2e/daemon-worktree-setup-e2e.sh
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BIN="$REPO_ROOT/src-tauri/target/debug/codemux-remote"
IMAGE="${CMX_WT_E2E_IMAGE:-archlinux:latest}"
CONTAINER="codemux-worktree-setup-e2e"

log() { printf '\n\033[1;36m[e2e]\033[0m %s\n' "$*"; }
err() { printf '\n\033[1;31m[e2e:ERROR]\033[0m %s\n' "$*" >&2; }

cleanup() {
  docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
}
trap cleanup EXIT

command -v docker >/dev/null || { err "docker not found"; exit 1; }
[ -x "$BIN" ] || { err "codemux-remote not built at $BIN"; exit 1; }

log "starting clean host container ($IMAGE)"
docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
docker run -d --name "$CONTAINER" \
  -v "$BIN":/usr/local/bin/codemux-remote:ro \
  "$IMAGE" sleep infinity >/dev/null

log "installing git + runtime libs in the container"
# The codemux-remote bin target shares the desktop crate, so the debug
# binary dynamically links gtk/webkit even though serve never uses them.
docker exec "$CONTAINER" pacman -Sy --noconfirm git gtk3 webkit2gtk-4.1 >/dev/null 2>&1

log "seeding repo with setup config + gitignored .env"
docker exec -i "$CONTAINER" bash -eu -o pipefail <<'SEED'
git config --global user.email t@e.com
git config --global user.name T
git config --global init.defaultBranch main
mkdir -p /root/repo/.codemux
cd /root/repo
git init -q
# Single-quoted heredoc below keeps $CODEMUX_* literal for the daemon.
cat > .codemux/config.json <<'CFG'
{ "setup": ["printf '%s\n%s\n' \"$CODEMUX_BRANCH\" \"$CODEMUX_PORT\" > setup-ran.txt"] }
CFG
printf '.env\n' > .gitignore
printf 'SECRET=docker-host\n' > .env
printf 'x\n' > f.txt
git add .
git commit -qm init
SEED

log "starting codemux-remote serve"
docker exec -d "$CONTAINER" bash -c \
  'mkdir -p /root/state && codemux-remote serve --state-dir /root/state > /root/state/serve.log 2>&1'

log "waiting for manifest"
for i in $(seq 1 50); do
  if docker exec "$CONTAINER" test -f /root/state/manifest.json; then break; fi
  sleep 0.2
done
docker exec "$CONTAINER" test -f /root/state/manifest.json || {
  err "daemon never wrote manifest"; docker exec "$CONTAINER" cat /root/state/serve.log || true; exit 1;
}

MANIFEST="$(docker exec "$CONTAINER" cat /root/state/manifest.json)"
ENDPOINT="$(printf '%s' "$MANIFEST" | python3 -c 'import sys,json;print(json.load(sys.stdin)["endpoint"])')"
SECRET="$(printf '%s' "$MANIFEST" | python3 -c 'import sys,json;print(json.load(sys.stdin)["secret"])')"
log "daemon live at $ENDPOINT"

log "calling tools/call worktree_create over authed HTTP"
RESPONSE="$(docker exec "$CONTAINER" curl -sf -X POST "$ENDPOINT/tools/call" \
  -H "Authorization: Bearer $SECRET" \
  -H 'Content-Type: application/json' \
  -d '{"name":"worktree_create","arguments":{"repo_path":"/root/repo","branch":"feature/provision","base":"main"}}')"
printf '%s\n' "$RESPONSE" | python3 -m json.tool

WS_PATH="$(printf '%s' "$RESPONSE" | python3 -c 'import sys,json;print(json.load(sys.stdin)["data"]["workspace"]["path"])')"
CONFIGURED="$(printf '%s' "$RESPONSE" | python3 -c 'import sys,json;print(json.load(sys.stdin)["data"]["setup"]["configured"])')"
PORT="$(printf '%s' "$RESPONSE" | python3 -c 'import sys,json;print(json.load(sys.stdin)["data"]["setup"]["port"])')"
[ "$CONFIGURED" = "True" ] || { err "setup.configured should be true"; exit 1; }

log "worktree at $WS_PATH — polling for setup marker"
MARKER="$WS_PATH/setup-ran.txt"
for i in $(seq 1 100); do
  if docker exec "$CONTAINER" test -f "$MARKER"; then break; fi
  sleep 0.2
done
docker exec "$CONTAINER" test -f "$MARKER" || {
  err "setup script never ran"; docker exec "$CONTAINER" cat /root/state/serve.log || true; exit 1;
}

GOT_BRANCH="$(docker exec "$CONTAINER" sed -n 1p "$MARKER")"
GOT_PORT="$(docker exec "$CONTAINER" sed -n 2p "$MARKER")"
GOT_ENV="$(docker exec "$CONTAINER" cat "$WS_PATH/.env")"

[ "$GOT_BRANCH" = "feature/provision" ] || { err "CODEMUX_BRANCH wrong: $GOT_BRANCH"; exit 1; }
[ "$GOT_PORT" = "$PORT" ] || { err "CODEMUX_PORT mismatch: $GOT_PORT vs $PORT"; exit 1; }
[ "$GOT_ENV" = "SECRET=docker-host" ] || { err ".env not copied: $GOT_ENV"; exit 1; }

log "PASS — daemon-created worktree was provisioned (setup script ran with branch=$GOT_BRANCH port=$GOT_PORT, .env copied)"
