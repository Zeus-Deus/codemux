#!/bin/bash
# Copy the agent-browser native binary to src-tauri/binaries/ for Tauri sidecar bundling.
# Tauri expects: binaries/{name}-{target_triple}
# Called by beforeDevCommand/beforeBuildCommand or CI.

set -e

BINDIR="src-tauri/binaries"
mkdir -p "$BINDIR"

# Detect target triple
TARGET="${CARGO_BUILD_TARGET:-$(rustc -vV | grep host | cut -d' ' -f2)}"

# agent-browser v0.44+ publishes native binaries as optional platform packages.
case "$TARGET" in
  x86_64-unknown-linux-gnu)  AB_PACKAGE="cli-linux-x64-gnu" ;;
  aarch64-unknown-linux-gnu) AB_PACKAGE="cli-linux-arm64-gnu" ;;
  x86_64-apple-darwin)       AB_PACKAGE="cli-darwin-x64" ;;
  aarch64-apple-darwin)      AB_PACKAGE="cli-darwin-arm64" ;;
  x86_64-pc-windows-msvc)    AB_PACKAGE="cli-win32-x64-msvc" ;;
  *)
    echo "[copy-agent-browser] Unsupported target: $TARGET" >&2
    exit 1
    ;;
esac

AB_BIN="agent-browser"
case "$TARGET" in
  *windows*) AB_BIN="${AB_BIN}.exe" ;;
esac
SRC="node_modules/@agent-browser/$AB_PACKAGE/bin/$AB_BIN"
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
  echo "[copy-agent-browser] ERROR: $SRC not found — refusing to reuse a staged sidecar" >&2
  echo "[copy-agent-browser] Run 'npm install' first to download the binary" >&2
  exit 1
fi
