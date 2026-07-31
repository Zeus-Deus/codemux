#!/bin/bash
# Build the claude-agent sidecar for the current target and stage the
# compiled binary at src-tauri/binaries/codemux-claude-sidecar-<triple>
# so Tauri's `resources` glob can bundle it into the installer / AppImage.
# (The sidecar used to ship as an externalBin; it moved to resources
# because linuxdeploy's patchelf corrupts the bun-compiled binary —
# see commit 025fa19.)
#
# Called from tauri.conf.json's beforeDevCommand / beforeBuildCommand, by
# the release workflow, and optionally during `cargo test` for local dev.
#
# Unlike codemux-remote there is no debug/release split here: `bun build
# --compile` produces one flavour, and this script always recompiles from
# source rather than reusing whatever is already staged, so the bundled
# binary is always current. The one staleness hazard is an *unsupported*
# target triple, where the script used to `exit 0` and silently leave a
# leftover file in place for the bundler to pick up. Pass `--strict`
# (beforeBuildCommand does) to turn that into a hard failure.
#
# Provenance stamp: writes src-tauri/binaries/.codemux-claude-sidecar-
# <triple>.stamp after staging. The leading dot keeps it out of the
# `codemux-claude-sidecar-*` bundle glob.

set -eu

STRICT=0
while [ $# -gt 0 ]; do
  case "$1" in
    --strict) STRICT=1 ; shift ;;
    *)
      echo "[build-claude-sidecar] ERROR: unknown argument: $1" >&2
      echo "[build-claude-sidecar] usage: $0 [--strict]" >&2
      exit 2
      ;;
  esac
done

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
    if [ "$STRICT" -eq 1 ]; then
      echo "[build-claude-sidecar] ERROR: unsupported target: $TARGET" >&2
      echo "[build-claude-sidecar] Refusing to leave a possibly stale sidecar staged for a bundle build." >&2
      exit 1
    fi
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
STAMP="$BINDIR/.codemux-claude-sidecar-$TARGET.stamp"

# Provenance: stage only the artifact `bun build --compile` just emitted.
if [ ! -s "$SRC" ]; then
  rm -f "$STAMP"
  echo "[build-claude-sidecar] ERROR: bun reported success but produced no usable binary at:" >&2
  echo "[build-claude-sidecar]   $SRC" >&2
  exit 1
fi

# Only rewrite when the bytes differ. src-tauri/binaries/ is a tracked input
# of tauri-build's resource scan, so an unconditional `cp` bumps the mtime
# and forces a full rebuild of codemux_lib on the next cargo invocation —
# i.e. right after beforeBuildCommand has staged it.
if cmp -s "$SRC" "$DST" 2>/dev/null; then
  echo "[build-claude-sidecar] staged file already identical — leaving mtime alone"
else
  cp "$SRC" "$DST"
fi
chmod +x "$DST"

# Cheap post-copy integrity check (truncated copy / full disk).
if ! cmp -s "$SRC" "$DST"; then
  echo "[build-claude-sidecar] ERROR: staged file differs from the build artifact" >&2
  echo "[build-claude-sidecar]   src: $SRC" >&2
  echo "[build-claude-sidecar]   dst: $DST" >&2
  exit 1
fi

SIZE="$(wc -c < "$DST" | tr -d ' ')"
STAMP_BODY="target=$TARGET
bun_target=$BUN_TARGET
source=$SRC
staged=$DST
bytes=$SIZE"
if [ "$(cat "$STAMP" 2>/dev/null || true)" != "$STAMP_BODY" ]; then
  printf '%s\n' "$STAMP_BODY" > "$STAMP"
fi

echo "[build-claude-sidecar] staged $DST (${SIZE} bytes)"
