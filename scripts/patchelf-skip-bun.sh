#!/usr/bin/env bash
# patchelf stand-in for linuxdeploy's AppImage step (release.yml sets $PATCHELF).
#
# linuxdeploy rewrites the rpath of every ELF file in the AppDir, including
# the Bun-compiled sidecars under usr/lib/codemux/binaries. Bun keeps a
# compiled program's payload in a `.bun` ELF section, and older patchelf
# releases (including the 0.8 bundled with Tauri's linuxdeploy) corrupt it:
# the rewritten binary dumps core, and the gtk plugin's second linuxdeploy
# pass aborts with "Failed to run ldd: exited with code 1".
#
# Bun executables only link against glibc, so they need no rpath. Leave them
# untouched and pass every other call to the real patchelf.
set -u

for arg in "$@"; do
  if [ -f "$arg" ] && readelf -SW "$arg" 2>/dev/null | grep -q ' \.bun '; then
    echo "patchelf-skip-bun: leaving Bun executable untouched: $arg" >&2
    exit 0
  fi
done

exec "${REAL_PATCHELF:-/usr/bin/patchelf}" "$@"
