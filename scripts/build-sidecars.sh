#!/usr/bin/env bash
# Build Codemux's bundled Bun programs (the Claude sidecar, the managed
# workflow sidecar and the plugin host) as ONE compiled executable and stage it
# at src-tauri/binaries/codemux-sidecar-<triple>[.exe].
#
# The three programs used to ship as three `bun build --compile` binaries,
# each carrying its own ~93 MB Bun runtime. They now share one binary; the
# entry point (sidecar/codemux-sidecar.mjs) picks the program from a leading
# `--codemux-role=<role>` argument, and each role still runs in its own
# process. See src-tauri/src/bundled_sidecar.rs for how Codemux starts them.
#
# The binary is a Tauri `resource`, not an `externalBin`, because
# linuxdeploy's patchelf corrupts Bun executables (commit 025fa19).
#
# Called from tauri.conf.json's beforeDevCommand / beforeBuildCommand and by
# CI and the release workflow. Always recompiles from source. Pass `--strict`
# (beforeBuildCommand and CI do) to fail instead of skipping an unsupported
# target triple, so a stale binary is never left staged for a bundle build.
#
# Also stages the Cursor SDK's native package beside the binary: the SDK
# resolves ripgrep and tree-sitter from node_modules/@cursor/sdk-<os>-<arch>
# next to the executable rather than from the Bun bundle.
#
# Provenance stamp: src-tauri/binaries/.codemux-sidecar-<triple>.stamp. The
# leading dot keeps it out of the `codemux-sidecar-*` resources glob.
set -euo pipefail

log() { echo "[build-sidecars] $*" >&2; }

strict=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --strict) strict=1; shift ;;
    *) log "ERROR: unknown argument: $1"; log "usage: $0 [--strict]"; exit 2 ;;
  esac
done

# Bun is not a user-machine dependency: it is compiled into the binary. Every
# role is built with the one reviewed version (CI pins it with setup-bun).
required_bun=1.3.12
command -v bun >/dev/null 2>&1 || { log "ERROR: Bun $required_bun is required; install it from https://bun.sh and re-run."; exit 1; }
bun_version="$(bun --version)"
if [ "$bun_version" != "$required_bun" ]; then
  log "ERROR: Codemux sidecar builds require reviewed Bun $required_bun; found $bun_version. Select Bun $required_bun before building Codemux."
  exit 1
fi

script_dir="$(cd "$(dirname "$0")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
sidecar_dir="$repo_root/sidecar"
binary_dir="$repo_root/src-tauri/binaries"
target="${CARGO_BUILD_TARGET:-$(rustc -vV | awk '/^host:/ {print $2}')}"

case "$target" in
  x86_64-unknown-linux-gnu) bun_target=bun-linux-x64; sdk_os=linux; sdk_arch=x64; suffix= ;;
  aarch64-unknown-linux-gnu) bun_target=bun-linux-arm64; sdk_os=linux; sdk_arch=arm64; suffix= ;;
  x86_64-apple-darwin) bun_target=bun-darwin-x64; sdk_os=darwin; sdk_arch=x64; suffix= ;;
  aarch64-apple-darwin) bun_target=bun-darwin-arm64; sdk_os=darwin; sdk_arch=arm64; suffix= ;;
  x86_64-pc-windows-msvc|x86_64-pc-windows-gnu) bun_target=bun-windows-x64; sdk_os=win32; sdk_arch=x64; suffix=.exe ;;
  *)
    if [ "$strict" -eq 1 ]; then
      log "ERROR: unsupported target: $target"
      log "Refusing to leave a possibly stale sidecar staged for a bundle build."
      exit 1
    fi
    log "Unknown target: $target — skipping"
    exit 0
    ;;
esac

name="codemux-sidecar-$target$suffix"
source_binary="$sidecar_dir/dist/$name"
dest_binary="$binary_dir/$name"
stamp="$binary_dir/.codemux-sidecar-$target.stamp"
mkdir -p "$binary_dir" "$sidecar_dir/dist"

# Each program keeps its own package and lockfile; the bundler resolves every
# import from the importing file's own node_modules.
(cd "$sidecar_dir/claude-agent" && { bun install --frozen-lockfile 2>/dev/null || bun install; })
# The Cursor SDK's native package is per-platform, so install the target's.
(cd "$sidecar_dir/managed-workflow" && bun install --frozen-lockfile --os "$sdk_os" --cpu "$sdk_arch")

(cd "$sidecar_dir" && bun build --compile --target="$bun_target" --outfile="$source_binary" codemux-sidecar.mjs)
if [ ! -s "$source_binary" ]; then
  rm -f "$stamp"
  log "ERROR: bun reported success but produced no usable binary at: $source_binary"
  exit 1
fi

# Only rewrite when the bytes differ. src-tauri/binaries/ is a tracked input
# of tauri-build's resource scan, so an unconditional copy bumps the mtime and
# forces a full rebuild of codemux_lib on the next cargo invocation.
if cmp -s "$source_binary" "$dest_binary" 2>/dev/null; then
  log "staged file already identical — leaving mtime alone"
else
  cp "$source_binary" "$dest_binary"
fi
chmod +x "$dest_binary"
if ! cmp -s "$source_binary" "$dest_binary"; then
  log "ERROR: staged file differs from the build artifact (src: $source_binary, dst: $dest_binary)"
  exit 1
fi

# The SDK deliberately resolves its native binaries outside the flat Bun
# bundle. Keep the official package layout beside the executable.
native_name="sdk-$sdk_os-$sdk_arch"
native_source="$sidecar_dir/managed-workflow/node_modules/@cursor/$native_name"
native_dest="$binary_dir/node_modules/@cursor/$native_name"
test -s "$native_source/package.json"
test -d "$native_source/bin"
test -d "$native_source/vendor/tree-sitter"
mkdir -p "$native_dest"
if ! diff -qr "$native_source" "$native_dest" >/dev/null; then
  cp -R "$native_source/." "$native_dest/"
fi
if ! diff -qr "$native_source" "$native_dest" >/dev/null; then
  log "ERROR: Cursor native asset staging failed"
  exit 1
fi

size="$(wc -c < "$dest_binary" | tr -d ' ')"
stamp_body="target=$target
bun=$bun_version
bun_target=$bun_target
source=$source_binary
staged=$dest_binary
bytes=$size"
if [ "$(cat "$stamp" 2>/dev/null || true)" != "$stamp_body" ]; then
  printf '%s\n' "$stamp_body" > "$stamp"
fi

log "staged $dest_binary (${size} bytes) and @cursor/$native_name"
