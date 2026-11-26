#!/usr/bin/env bash
# Build Codemux's authenticated browser sidecar from a pinned upstream source.
# Tauri requires binaries/agent-browser-<target> before it starts packaging.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SOURCE_COMMIT="0a923358ea989d7602f0d06507ad3447c62ffe9b" # upstream v0.44.0
AUTH_PATCH="$ROOT/scripts/patches/agent-browser-v0.44-codemux.patch"
PIPE_PATCH="$ROOT/scripts/patches/agent-browser-v0.44-cdp-pipe.patch"
TARGET="${CARGO_BUILD_TARGET:-$(rustc -vV | sed -n 's/^host: //p')}"

case "$TARGET" in
  x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu|x86_64-apple-darwin|aarch64-apple-darwin|x86_64-pc-windows-msvc) ;;
  *) echo "Unsupported browser sidecar target: $TARGET" >&2; exit 1 ;;
esac

SUFFIX=""
case "$TARGET" in *windows*) SUFFIX=".exe" ;; esac
DEST="$ROOT/src-tauri/binaries/agent-browser-$TARGET$SUFFIX"
BUILD_ROOT="$ROOT/src-tauri/target/codemux-agent-browser"
STAMP="$DEST.codemux-stamp"
hash_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | cut -d' ' -f1
  else
    echo "A SHA-256 utility is required to verify the browser sidecar" >&2
    return 1
  fi
}
AUTH_PATCH_HASH="$(hash_file "$AUTH_PATCH")"
PIPE_PATCH_HASH="$(hash_file "$PIPE_PATCH")"
mkdir -p "$ROOT/src-tauri/binaries" "$BUILD_ROOT"

# Reusing a staged binary requires both the reviewed patch identity and the
# exact bytes recorded by this build. An npm binary never satisfies the stamp.
if [ -s "$DEST" ] && [ -f "$STAMP" ] &&
   [ "$(cat "$STAMP")" = "$SOURCE_COMMIT $AUTH_PATCH_HASH $PIPE_PATCH_HASH $(hash_file "$DEST")" ] &&
   [ "${CODEMUX_BROWSER_RUN_PIPE_TESTS:-0}" != 1 ] &&
   [ "$("$DEST" --version)" = "agent-browser 0.44.0+codemux.2" ]; then
  exit 0
fi

SOURCE_DIR="$(mktemp -d "$BUILD_ROOT/source.XXXXXX")"
trap 'rm -rf -- "$SOURCE_DIR"' EXIT
git -C "$SOURCE_DIR" init -q
git -C "$SOURCE_DIR" fetch -q --depth=1 https://github.com/vercel-labs/agent-browser.git "$SOURCE_COMMIT"
git -C "$SOURCE_DIR" checkout -q --detach FETCH_HEAD
if [ "$(git -C "$SOURCE_DIR" rev-parse HEAD)" != "$SOURCE_COMMIT" ]; then
  echo "Browser sidecar source commit mismatch" >&2
  exit 1
fi
git -C "$SOURCE_DIR" apply --check "$AUTH_PATCH"
git -C "$SOURCE_DIR" apply "$AUTH_PATCH"
git -C "$SOURCE_DIR" apply --check "$PIPE_PATCH"
git -C "$SOURCE_DIR" apply "$PIPE_PATCH"

# The native smoke checks exercise a real browser, while these patched-source
# tests cover cancellation, bounded startup retries, and child reaping.
# Opting in must still run them when a stamped binary is already available.
if [ "${CODEMUX_BROWSER_RUN_PIPE_TESTS:-0}" = 1 ]; then
  TMPDIR="${CODEMUX_BROWSER_BUILD_TMPDIR:-${TMPDIR:-$BUILD_ROOT}}" \
    CARGO_TARGET_DIR="$BUILD_ROOT/target" \
    cargo test -j 2 --locked --profile ci --target "$TARGET" \
    --manifest-path "$SOURCE_DIR/cli/Cargo.toml" \
    --bin agent-browser pipe_ -- --test-threads=2
fi

TMPDIR="${CODEMUX_BROWSER_BUILD_TMPDIR:-${TMPDIR:-$BUILD_ROOT}}" \
  CARGO_TARGET_DIR="$BUILD_ROOT/target" \
  cargo build -j 2 --locked --profile ci --target "$TARGET" \
  --manifest-path "$SOURCE_DIR/cli/Cargo.toml"

BUILT="$BUILD_ROOT/target/$TARGET/ci/agent-browser$SUFFIX"
if [ ! -s "$BUILT" ] || [ "$("$BUILT" --version)" != "agent-browser 0.44.0+codemux.2" ]; then
  echo "Pipe-enabled browser sidecar build is missing or has the wrong version" >&2
  exit 1
fi
cp "$BUILT" "$DEST"
chmod +x "$DEST"
printf '%s %s %s %s\n' "$SOURCE_COMMIT" "$AUTH_PATCH_HASH" "$PIPE_PATCH_HASH" "$(hash_file "$DEST")" > "$STAMP"
