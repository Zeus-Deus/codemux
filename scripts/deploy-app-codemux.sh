#!/bin/bash
# Pull and deploy the hosted Codemux client from the latest GitHub release.
# Intended for /home/work/app-codemux/deploy-pull.sh on the production VPS.
set -euo pipefail

APP_DIR="${CODEMUX_DEPLOY_APP_DIR:-/home/work/app-codemux}"
HTML_DIR="$APP_DIR/html"
BACKUP_DIR="$APP_DIR/backups"
STATE_FILE="$APP_DIR/deployed-release"
LOG_FILE="$APP_DIR/deploy.log"
LOCK_FILE="$APP_DIR/deploy.lock"
LATEST_RELEASE_URL="${CODEMUX_DEPLOY_LATEST_URL:-https://github.com/Zeus-Deus/codemux/releases/latest}"
DOWNLOAD_BASE_URL="${CODEMUX_DEPLOY_DOWNLOAD_BASE_URL:-https://github.com/Zeus-Deus/codemux/releases/download}"
PUBLIC_URL="${CODEMUX_DEPLOY_PUBLIC_URL:-https://app.codemux.org}"
API_HEALTH_URL="${CODEMUX_DEPLOY_API_HEALTH_URL:-https://api.codemux.org/health}"
DRY_RUN=false
REQUESTED_TAG="${CODEMUX_DEPLOY_RELEASE_TAG:-}"

usage() {
  echo "Usage: $0 [--check] [--tag vX.Y.Z]" >&2
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --check)
      DRY_RUN=true
      shift
      ;;
    --tag)
      [ "$#" -ge 2 ] || { usage; exit 2; }
      REQUESTED_TAG="$2"
      shift 2
      ;;
    *)
      usage
      exit 2
      ;;
  esac
done

case "$APP_DIR" in
  /*) ;;
  *) echo "ERROR: app directory must be absolute: $APP_DIR" >&2; exit 1 ;;
esac
[ "$APP_DIR" != "/" ] || { echo "ERROR: refusing to use / as the app directory" >&2; exit 1; }
[ -d "$HTML_DIR" ] || { echo "ERROR: hosted client directory not found: $HTML_DIR" >&2; exit 1; }

log() {
  local line
  line="$(date -u +%FT%TZ) $*"
  echo "$line"
  echo "$line" >> "$LOG_FILE"
}

exec 9>>"$LOCK_FILE"
if ! flock -n 9; then
  exit 0
fi

if [ -n "$REQUESTED_TAG" ]; then
  tag="$REQUESTED_TAG"
else
  effective_url="$(
    curl -fsSIL --retry 3 --connect-timeout 10 --max-time 30 \
      -o /dev/null -w '%{url_effective}' "$LATEST_RELEASE_URL"
  )"
  tag="${effective_url%/}"
  tag="${tag##*/}"
fi

if [[ ! "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$ ]]; then
  log "refusing invalid release tag: $tag"
  exit 1
fi

if [ -f "$STATE_FILE" ] && [ "$(sed -n '1p' "$STATE_FILE")" = "$tag" ]; then
  exit 0
fi

version="${tag#v}"
archive="codemux-web-${version}.tar.gz"
checksum="$archive.sha256"
release_url="$DOWNLOAD_BASE_URL/$tag"
work_dir="$(mktemp -d "$APP_DIR/.deploy-work.XXXXXX")"

cleanup() {
  case "$work_dir" in
    "$APP_DIR"/.deploy-work.*) rm -rf -- "$work_dir" ;;
  esac
}
trap cleanup EXIT

if ! curl -fsSL --retry 3 --connect-timeout 10 --max-time 300 \
  "$release_url/$archive" -o "$work_dir/$archive" 2>/dev/null; then
  if $DRY_RUN; then
    log "hosted client asset is not available for $tag yet"
  fi
  exit 0
fi
if ! curl -fsSL --retry 3 --connect-timeout 10 --max-time 30 \
  "$release_url/$checksum" -o "$work_dir/$checksum"; then
  log "checksum is not available for hosted client $tag"
  exit 1
fi

read -r expected_sum expected_name extra < "$work_dir/$checksum" || true
if [[ ! "$expected_sum" =~ ^[0-9a-fA-F]{64}$ ]] ||
  [ "$expected_name" != "$archive" ] || [ -n "${extra:-}" ]; then
  log "invalid checksum file for hosted client $tag"
  exit 1
fi
actual_sum="$(sha256sum "$work_dir/$archive" | awk '{print $1}')"
if [ "$actual_sum" != "$expected_sum" ]; then
  log "checksum mismatch for hosted client $tag"
  exit 1
fi

tar -tzf "$work_dir/$archive" > "$work_dir/archive-contents.txt"
invalid_path=false
while IFS= read -r raw_path; do
  path="${raw_path#./}"
  case "$path" in
    ""|.) ;;
    /*|../*|*/../*|*/..) invalid_path=true ;;
  esac
done < "$work_dir/archive-contents.txt"
if $invalid_path; then
  log "archive for hosted client $tag contains an unsafe path"
  exit 1
fi
if ! tar -tvzf "$work_dir/$archive" | awk '
  { type = substr($1, 1, 1); if (type != "-" && type != "d") exit 1 }
'; then
  log "archive for hosted client $tag contains a link or special file"
  exit 1
fi

stage_dir="$work_dir/stage"
mkdir -p "$stage_dir"
tar --no-same-owner --no-same-permissions -xzf "$work_dir/$archive" -C "$stage_dir"

wasm_js="iroh-wasm/$tag/iroh_wasm.js"
wasm_binary="iroh-wasm/$tag/iroh_wasm_bg.wasm"
test -s "$stage_dir/index.html" || { log "hosted client $tag has no index.html"; exit 1; }
test -s "$stage_dir/$wasm_js" || { log "hosted client $tag has no versioned WASM glue"; exit 1; }
test -s "$stage_dir/$wasm_binary" || { log "hosted client $tag has no versioned WASM binary"; exit 1; }

wasm_magic="$(od -An -tx1 -N4 "$stage_dir/$wasm_binary" | tr -d '[:space:]')"
[ "$wasm_magic" = "0061736d" ] || { log "hosted client $tag has an invalid WASM binary"; exit 1; }

entry_path="$(sed -n 's#.*src="/\([^"]*\.js\)".*#\1#p' "$stage_dir/index.html" | sed -n '1p')"
[ -n "$entry_path" ] && test -s "$stage_dir/$entry_path" || {
  log "hosted client $tag has a missing JavaScript entry"
  exit 1
}
wasm_loader_asset="$(grep -Rl '/iroh-wasm' "$stage_dir/assets" | sed -n '1p' || true)"
if [ -z "$wasm_loader_asset" ] || ! grep -Fq "$tag" "$wasm_loader_asset"; then
  log "hosted client $tag does not reference its versioned WASM glue"
  exit 1
fi

if $DRY_RUN; then
  log "validated hosted client $tag (dry run; no files changed)"
  exit 0
fi

if [ "${CODEMUX_DEPLOY_SKIP_LIVE_CHECKS:-0}" != "1" ]; then
  if ! curl -fsS --connect-timeout 10 --max-time 20 "$API_HEALTH_URL" |
    grep -qE '"status"[[:space:]]*:[[:space:]]*"ok"'; then
    log "API health check failed; leaving hosted client unchanged"
    exit 1
  fi
fi

mkdir -p "$BACKUP_DIR"
timestamp="$(date -u +%Y%m%d_%H%M%S)"
backup="$BACKUP_DIR/html-before-${tag}-${timestamp}.tar.gz"
backup_tmp="$backup.tmp"
tar -C "$APP_DIR" -czf "$backup_tmp" html
mv -f "$backup_tmp" "$backup"

# Copy immutable/versioned files first and retain previous generations. The
# entry point is promoted last with a same-filesystem rename, so a page load
# sees either the complete old deployment or the complete new deployment.
while IFS= read -r -d '' entry; do
  cp -a -- "$entry" "$HTML_DIR/"
done < <(find "$stage_dir" -mindepth 1 -maxdepth 1 ! -name index.html -print0)

index_tmp="$HTML_DIR/.index.html.${tag}.new"
install -m 0644 "$stage_dir/index.html" "$index_tmp"
mv -f "$index_tmp" "$HTML_DIR/index.html"

verify_live() {
  [ "${CODEMUX_DEPLOY_SKIP_LIVE_CHECKS:-0}" = "1" ] && return 0

  curl -fsS --connect-timeout 10 --max-time 30 \
    "$PUBLIC_URL/index.html?release=$tag" -o "$work_dir/live-index.html" || return 1
  cmp -s "$stage_dir/index.html" "$work_dir/live-index.html" || return 1

  curl -fsS --connect-timeout 10 --max-time 60 \
    "$PUBLIC_URL/$entry_path" -o "$work_dir/live-entry.js" || return 1
  cmp -s "$stage_dir/$entry_path" "$work_dir/live-entry.js" || return 1

  curl -fsS --connect-timeout 10 --max-time 120 \
    -D "$work_dir/wasm-headers.txt" \
    "$PUBLIC_URL/$wasm_binary" -o "$work_dir/live.wasm" || return 1
  grep -qiE '^content-type:[[:space:]]*application/wasm([[:space:]]|;|$)' \
    "$work_dir/wasm-headers.txt" || return 1
  cmp -s "$stage_dir/$wasm_binary" "$work_dir/live.wasm" || return 1
}

if ! verify_live; then
  tar -C "$APP_DIR" -xzf "$backup"
  log "live verification failed for $tag; restored $backup"
  exit 1
fi

state_tmp="$APP_DIR/.deployed-release.${tag}.new"
echo "$tag" > "$state_tmp"
mv -f "$state_tmp" "$STATE_FILE"
log "deployed hosted client $tag; rollback: $backup"
