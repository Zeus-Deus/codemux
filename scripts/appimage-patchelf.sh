#!/usr/bin/env bash
# linuxdeploy's bundled patchelf corrupts Bun's appended executable payload.
# Dynamic SDK assets still get RPATHs; its static helpers need no relocation.
set -euo pipefail

if [[ $# == 3 && $1 == --set-rpath ]]; then
  case "$3" in
    */codemux.AppDir/usr/lib/codemux/binaries/codemux-managed-workflow-sidecar-x86_64-unknown-linux-gnu|\
    */codemux.AppDir/usr/lib/codemux/binaries/codemux-managed-workflow-sidecar-aarch64-unknown-linux-gnu|\
    */codemux.AppDir/usr/lib/codemux/binaries/codemux-claude-sidecar-x86_64-unknown-linux-gnu|\
    */codemux.AppDir/usr/lib/codemux/binaries/codemux-claude-sidecar-aarch64-unknown-linux-gnu)
      sections="$(readelf --wide --sections "$3")"
      if [[ $sections =~ [[:space:]]\.bun[[:space:]] ]]; then
        echo "[codemux::appimage] preserving Bun sidecar: $3" >&2
        exit 0
      fi
      ;;
    */codemux.AppDir/usr/lib/codemux/binaries/node_modules/@cursor/sdk-linux-x64/bin/rg|\
    */codemux.AppDir/usr/lib/codemux/binaries/node_modules/@cursor/sdk-linux-arm64/bin/rg|\
    */codemux.AppDir/usr/lib/codemux/binaries/node_modules/@cursor/sdk-linux-x64/bin/cursorsandbox|\
    */codemux.AppDir/usr/lib/codemux/binaries/node_modules/@cursor/sdk-linux-arm64/bin/cursorsandbox)
      dynamic="$(readelf --wide --dynamic "$3")"
      segments="$(readelf --wide --program-headers "$3")"
      if [[ ! $dynamic =~ \(NEEDED\) && ! $segments =~ [[:space:]]INTERP[[:space:]] ]]; then
        echo "[codemux::appimage] preserving static SDK helper: $3" >&2
        exit 0
      fi
      ;;
  esac
fi

exec "${CODEMUX_REAL_PATCHELF:?An actual patchelf executable is required}" "$@"
