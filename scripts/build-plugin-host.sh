#!/usr/bin/env bash
# Bundled JavaScript runtime; plugin installation never downloads or builds a runtime.
set -euo pipefail
strict=0
while [ "$#" -gt 0 ]; do
  case "$1" in --strict) strict=1; shift ;; *) echo "Unknown argument: $1" >&2; exit 2 ;; esac
done
command -v bun >/dev/null || { echo "Bun is required to build the plugin host" >&2; exit 1; }
if [ "$(bun --version)" != "1.3.12" ]; then echo "Plugin host builds require reviewed Bun 1.3.12" >&2; exit 1; fi
script_dir="$(cd "$(dirname "$0")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
target="${CARGO_BUILD_TARGET:-$(rustc -vV | awk '/^host:/ {print $2}')}"
case "$target" in
  x86_64-unknown-linux-gnu) bun_target=bun-linux-x64; suffix= ;;
  aarch64-unknown-linux-gnu) bun_target=bun-linux-arm64; suffix= ;;
  x86_64-apple-darwin) bun_target=bun-darwin-x64; suffix= ;;
  aarch64-apple-darwin) bun_target=bun-darwin-arm64; suffix= ;;
  x86_64-pc-windows-msvc|x86_64-pc-windows-gnu) bun_target=bun-windows-x64; suffix=.exe ;;
  *) if [ "$strict" -eq 1 ]; then echo "Unsupported plugin host target: $target" >&2; exit 1; fi
     echo "Skipping unsupported plugin host target: $target" >&2; exit 0 ;;
esac
mkdir -p "$repo_root/src-tauri/binaries" "$repo_root/sidecar/plugin-host/dist"
source_binary="$repo_root/sidecar/plugin-host/dist/codemux-plugin-host-$target$suffix"
dest_binary="$repo_root/src-tauri/binaries/codemux-plugin-host-$target$suffix"
bun build --compile --target="$bun_target" --outfile="$source_binary" "$repo_root/sidecar/plugin-host/main.mjs"
test -s "$source_binary"
if ! cmp -s "$source_binary" "$dest_binary"; then cp "$source_binary" "$dest_binary"; fi
chmod +x "$dest_binary"
cmp -s "$source_binary" "$dest_binary"
echo "Staged plugin host for $target"
