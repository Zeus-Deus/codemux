#!/bin/bash
# Copy the agent-browser native binary to src-tauri/binaries/ for Tauri sidecar bundling.
# Tauri expects: binaries/{name}-{target_triple}
# Called by beforeDevCommand/beforeBuildCommand or CI.

set -e

BINDIR="src-tauri/binaries"
mkdir -p "$BINDIR"

# Detect target triple
TARGET="${CARGO_BUILD_TARGET:-$(rustc -vV | grep host | cut -d' ' -f2)}"

# Map target triple to agent-browser binary name
case "$TARGET" in
  x86_64-unknown-linux-gnu)  AB_BIN="agent-browser-linux-x64" ;;
  aarch64-unknown-linux-gnu) AB_BIN="agent-browser-linux-arm64" ;;
  x86_64-apple-darwin)       AB_BIN="agent-browser-darwin-x64" ;;
  aarch64-apple-darwin)      AB_BIN="agent-browser-darwin-arm64" ;;
  x86_64-pc-windows-msvc)    AB_BIN="agent-browser-win32-x64.exe" ;;
  *)
    echo "[copy-agent-browser] Unknown target: $TARGET — skipping"
    exit 0
    ;;
esac

SRC="node_modules/agent-browser/bin/$AB_BIN"
DST="$BINDIR/agent-browser-$TARGET"

# Windows binaries need .exe extension
case "$TARGET" in
  *windows*) DST="${DST}.exe" ;;
esac

if [ -f "$SRC" ]; then
  cp "$SRC" "$DST"
  chmod +x "$DST"
  echo "[copy-agent-browser] Copied $SRC → $DST"
else
  echo "[copy-agent-browser] WARNING: $SRC not found — agent-browser won't be bundled"
  echo "[copy-agent-browser] Run 'npm install' first to download the binary"
fi
