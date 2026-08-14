#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
DEPLOY_SCRIPT="$SCRIPT_DIR/deploy-app-codemux.sh"
TEST_ROOT="$(mktemp -d)"
trap 'rm -rf "$TEST_ROOT"' EXIT

APP_DIR="$TEST_ROOT/app-codemux"
RELEASES_DIR="$TEST_ROOT/releases/download"
mkdir -p "$APP_DIR/html/assets" "$RELEASES_DIR"
echo "old index" > "$APP_DIR/html/index.html"
echo "keep me" > "$APP_DIR/html/assets/old.js"

make_release() {
  local tag="$1"
  local version="${tag#v}"
  local release_dir="$RELEASES_DIR/$tag"
  local stage="$TEST_ROOT/stage-$tag"
  local archive="codemux-web-${version}.tar.gz"

  mkdir -p "$release_dir" "$stage/assets" "$stage/iroh-wasm/$tag"
  echo '<script type="module" src="/assets/app.js"></script>' > "$stage/index.html"
  echo "const wasm = \"/iroh-wasm/$tag/iroh_wasm.js\";" > "$stage/assets/app.js"
  echo "wasm glue $tag" > "$stage/iroh-wasm/$tag/iroh_wasm.js"
  printf '\0asm\1\0\0\0' > "$stage/iroh-wasm/$tag/iroh_wasm_bg.wasm"
  tar -C "$stage" -czf "$release_dir/$archive" .
  (cd "$release_dir" && sha256sum "$archive" > "$archive.sha256")
}

run_deploy() {
  CODEMUX_DEPLOY_APP_DIR="$APP_DIR" \
  CODEMUX_DEPLOY_DOWNLOAD_BASE_URL="file://$RELEASES_DIR" \
  CODEMUX_DEPLOY_SKIP_LIVE_CHECKS=1 \
  CODEMUX_DEPLOY_BACKUP_KEEP="${DEPLOY_BACKUP_KEEP:-5}" \
    "$DEPLOY_SCRIPT" --tag "$1"
}

make_release v1.2.3
run_deploy v1.2.3

grep -q 'assets/app.js' "$APP_DIR/html/index.html"
grep -q 'keep me' "$APP_DIR/html/assets/old.js"
test -s "$APP_DIR/html/iroh-wasm/v1.2.3/iroh_wasm.js"
test "$(sed -n '1p' "$APP_DIR/deployed-release")" = "v1.2.3"
test "$(find "$APP_DIR/backups" -type f -name 'html-before-v1.2.3-*.tar.gz' | wc -l)" -eq 1

# An idempotent poll must not create another backup.
run_deploy v1.2.3
test "$(find "$APP_DIR/backups" -type f -name 'html-before-v1.2.3-*.tar.gz' | wc -l)" -eq 1

# A corrupt release must fail before changing the live tree or state.
make_release v1.2.4
echo 'not the real checksum  codemux-web-1.2.4.tar.gz' > \
  "$RELEASES_DIR/v1.2.4/codemux-web-1.2.4.tar.gz.sha256"
if run_deploy v1.2.4; then
  echo "FAIL: corrupt checksum was accepted" >&2
  exit 1
fi
test "$(sed -n '1p' "$APP_DIR/deployed-release")" = "v1.2.3"
grep -q 'assets/app.js' "$APP_DIR/html/index.html"

# A release without a hosted client asset must say so instead of exiting quietly.
mkdir -p "$RELEASES_DIR/v1.2.5"
run_deploy v1.2.5
grep -q 'hosted client asset is not available for v1.2.5' "$APP_DIR/deploy.log"
test "$(sed -n '1p' "$APP_DIR/deployed-release")" = "v1.2.3"

# Backups are pruned to the retention limit.
make_release v1.2.6
make_release v1.2.7
DEPLOY_BACKUP_KEEP=2 run_deploy v1.2.6
DEPLOY_BACKUP_KEEP=2 run_deploy v1.2.7
test "$(find "$APP_DIR/backups" -type f -name 'html-before-*.tar.gz' | wc -l)" -eq 2
test "$(find "$APP_DIR/backups" -type f -name 'html-before-v1.2.3-*.tar.gz' | wc -l)" -eq 0

echo "deploy-app-codemux tests passed"
