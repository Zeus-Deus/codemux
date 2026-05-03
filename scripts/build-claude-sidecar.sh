#!/bin/bash
# Build the claude-agent sidecar for the current target and stage the
# compiled binary at src-tauri/binaries/codemux-claude-sidecar-<triple>
# so Tauri's `externalBin` can bundle it into the installer / AppImage.
#
# Called by scripts/copy-sidecars.sh (which wraps every sidecar we
# bundle), by the release workflow, and optionally during `cargo test`
# for local dev.

set -eu

if ! command -v bun >/dev/null 2>&1; then
  echo "[build-claude-sidecar] ERROR: Bun is required to build the sidecar."
  echo "[build-claude-sidecar] Install from https://bun.sh and re-run."
  exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
SIDECAR_DIR="$REPO_ROOT/sidecar/claude-agent"
BINDIR="$REPO_ROOT/src-tauri/binaries"

mkdir -p "$BINDIR"

TARGET="${CARGO_BUILD_TARGET:-$(rustc -vV | grep host | cut -d' ' -f2)}"

# Map the Rust target triple to a Bun --target and the resulting dist
# filename.
case "$TARGET" in
  x86_64-unknown-linux-gnu)
    BUN_TARGET="bun-linux-x64"
    DIST_NAME="codemux-claude-sidecar-linux-x64"
    ;;
  aarch64-unknown-linux-gnu)
    BUN_TARGET="bun-linux-arm64"
    DIST_NAME="codemux-claude-sidecar-linux-arm64"
    ;;
  x86_64-apple-darwin)
    BUN_TARGET="bun-darwin-x64"
    DIST_NAME="codemux-claude-sidecar-darwin-x64"
    ;;
  aarch64-apple-darwin)
    BUN_TARGET="bun-darwin-arm64"
    DIST_NAME="codemux-claude-sidecar-darwin-arm64"
    ;;
  x86_64-pc-windows-msvc|x86_64-pc-windows-gnu)
    BUN_TARGET="bun-windows-x64"
    DIST_NAME="codemux-claude-sidecar-windows-x64.exe"
    ;;
  *)
    echo "[build-claude-sidecar] Unknown target: $TARGET — skipping"
    exit 0
    ;;
esac

(cd "$SIDECAR_DIR" && bun install --frozen-lockfile 2>/dev/null || bun install)
(cd "$SIDECAR_DIR" && bun build --compile \
  --target="$BUN_TARGET" \
  --outfile="dist/$DIST_NAME" \
  src/main.ts)

SRC="$SIDECAR_DIR/dist/$DIST_NAME"
case "$TARGET" in
  *windows*) DST="$BINDIR/codemux-claude-sidecar-$TARGET.exe" ;;
  *)         DST="$BINDIR/codemux-claude-sidecar-$TARGET" ;;
esac

cp "$SRC" "$DST"
chmod +x "$DST"
echo "[build-claude-sidecar] staged $DST"
