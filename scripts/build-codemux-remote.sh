#!/bin/bash
# Build the codemux-remote binary for the current target and stage
# it under src-tauri/binaries/codemux-remote-<target> so Tauri's
# `bundle.resources = ["binaries/codemux-remote-*"]` glob has
# something to match. Called by beforeDevCommand and beforeBuildCommand
# so `cargo run --bin codemux` / `tauri dev` / `tauri build` all work.
#
# In CI release.yml the equivalent build step is inline (see the
# "Build codemux-remote binary" step) so this script isn't strictly
# required there — but having it in beforeBuildCommand keeps release
# bundles self-contained even without the explicit CI step.
#
# Pattern mirrors copy-agent-browser.sh and build-claude-sidecar.sh.

set -e

BINDIR="src-tauri/binaries"
mkdir -p "$BINDIR"

# Detect target triple. Honors CARGO_BUILD_TARGET when cross-compiling.
TARGET="${CARGO_BUILD_TARGET:-$(rustc -vV | grep host | cut -d' ' -f2)}"

# Pick the right cargo output extension per platform.
case "$TARGET" in
  *windows*) SRC="src-tauri/target/${TARGET}/release/codemux-remote.exe" ; SRC_DEBUG="src-tauri/target/${TARGET}/debug/codemux-remote.exe" ; DST="$BINDIR/codemux-remote-$TARGET.exe" ;;
  *)         SRC="src-tauri/target/${TARGET}/release/codemux-remote" ;     SRC_DEBUG="src-tauri/target/${TARGET}/debug/codemux-remote" ;     DST="$BINDIR/codemux-remote-$TARGET" ;;
esac

# Fallback paths when no --target was passed: cargo uses target/debug
# or target/release without the triple subdir.
SRC_RELEASE_NO_TRIPLE="src-tauri/target/release/codemux-remote"
SRC_DEBUG_NO_TRIPLE="src-tauri/target/debug/codemux-remote"
case "$TARGET" in
  *windows*)
    SRC_RELEASE_NO_TRIPLE="${SRC_RELEASE_NO_TRIPLE}.exe"
    SRC_DEBUG_NO_TRIPLE="${SRC_DEBUG_NO_TRIPLE}.exe"
    ;;
esac

# Build the binary if it's missing. Use debug build for dev (fast),
# release would be slow + unnecessary for `tauri dev`.
if [ ! -f "$SRC" ] && [ ! -f "$SRC_DEBUG" ] \
   && [ ! -f "$SRC_RELEASE_NO_TRIPLE" ] && [ ! -f "$SRC_DEBUG_NO_TRIPLE" ]; then
  echo "[build-codemux-remote] no existing binary — building debug"
  # Chicken-and-egg: `codemux-remote` is a [[bin]] inside the same Cargo
  # package as the `codemux` Tauri app, so `cargo build --bin
  # codemux-remote` triggers that package's build.rs (tauri_build::build()).
  # That build script scans tauri.conf.json's
  # `bundle.resources = ["binaries/codemux-remote-*"]` glob and panics if
  # nothing matches — but on a fresh checkout, nothing matches *until*
  # we build the binary. Plant an empty stub at the glob target first so
  # the resource scan passes; we overwrite it with the real binary below.
  touch "$DST"
  cargo build --bin codemux-remote --manifest-path src-tauri/Cargo.toml
fi

# Find whatever exists and copy it. Prefer release, then debug, then
# the no-triple paths.
for candidate in "$SRC" "$SRC_DEBUG" "$SRC_RELEASE_NO_TRIPLE" "$SRC_DEBUG_NO_TRIPLE"; do
  if [ -f "$candidate" ]; then
    cp "$candidate" "$DST"
    chmod +x "$DST"
    echo "[build-codemux-remote] staged $candidate → $DST"
    exit 0
  fi
done

echo "[build-codemux-remote] WARNING: no codemux-remote binary found after build"
echo "[build-codemux-remote] checked: $SRC, $SRC_DEBUG, $SRC_RELEASE_NO_TRIPLE, $SRC_DEBUG_NO_TRIPLE"
echo "[build-codemux-remote] push-to-host feature will be unavailable in this build"
# Don't fail — the rest of the app should still build. The Tauri glob
# will fail on its own if no codemux-remote-* file exists in binaries/.
exit 0
