#!/usr/bin/env bash
# Ship the reviewed harness SDK and its platform assets; never install on a user's machine.
set -euo pipefail
strict=0
while [ "$#" -gt 0 ]; do
  case "$1" in --strict) strict=1; shift ;; *) echo "Unknown argument: $1" >&2; exit 2 ;; esac
done
command -v bun >/dev/null || { echo "Bun is required to build the managed workflow sidecar" >&2; exit 1; }
bun_version="$(bun --version)"
if [ "$bun_version" != "1.3.12" ]; then
  echo "Managed workflow sidecar builds require reviewed Bun 1.3.12; found $bun_version. Select Bun 1.3.12 before building CodeMux." >&2
  exit 1
fi
script_dir="$(cd "$(dirname "$0")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
sidecar_dir="$repo_root/sidecar/managed-workflow"
binary_dir="$repo_root/src-tauri/binaries"
target="${CARGO_BUILD_TARGET:-$(rustc -vV | awk '/^host:/ {print $2}')}"
case "$target" in
  x86_64-unknown-linux-gnu) bun_target=bun-linux-x64; sdk_os=linux; sdk_arch=x64; suffix= ;;
  aarch64-unknown-linux-gnu) bun_target=bun-linux-arm64; sdk_os=linux; sdk_arch=arm64; suffix= ;;
  x86_64-apple-darwin) bun_target=bun-darwin-x64; sdk_os=darwin; sdk_arch=x64; suffix= ;;
  aarch64-apple-darwin) bun_target=bun-darwin-arm64; sdk_os=darwin; sdk_arch=arm64; suffix= ;;
  x86_64-pc-windows-msvc|x86_64-pc-windows-gnu) bun_target=bun-windows-x64; sdk_os=win32; sdk_arch=x64; suffix=.exe ;;
  *)
    if [ "$strict" -eq 1 ]; then echo "Unsupported managed workflow sidecar target: $target" >&2; exit 1; fi
    echo "Skipping unsupported managed workflow sidecar target: $target" >&2; exit 0 ;;
esac
mkdir -p "$binary_dir"
(
  cd "$sidecar_dir"
  bun install --frozen-lockfile --os "$sdk_os" --cpu "$sdk_arch"
  bun build --compile --target="$bun_target" --outfile="dist/codemux-managed-workflow-sidecar-$target$suffix" src/main.ts
)
source_binary="$sidecar_dir/dist/codemux-managed-workflow-sidecar-$target$suffix"
dest_binary="$binary_dir/codemux-managed-workflow-sidecar-$target$suffix"
test -s "$source_binary"
if ! cmp -s "$source_binary" "$dest_binary"; then cp "$source_binary" "$dest_binary"; fi
chmod +x "$dest_binary"
cmp -s "$source_binary" "$dest_binary"

# The SDK deliberately resolves its native binaries outside the flat Bun bundle.
# Keep the official package layout beside the executable, in Tauri resources.
native_name="sdk-$sdk_os-$sdk_arch"
native_source="$sidecar_dir/node_modules/@cursor/$native_name"
test -s "$native_source/package.json"
test -d "$native_source/bin"
test -d "$native_source/vendor/tree-sitter"
mkdir -p "$binary_dir/node_modules/@cursor/$native_name"
if ! diff -qr "$native_source" "$binary_dir/node_modules/@cursor/$native_name" >/dev/null; then
  cp -R "$native_source/." "$binary_dir/node_modules/@cursor/$native_name/"
fi
if ! diff -qr "$native_source" "$binary_dir/node_modules/@cursor/$native_name" >/dev/null; then
  echo "Managed workflow native asset staging failed" >&2; exit 1
fi
echo "Staged managed workflow sidecar and @cursor/$native_name for $target"
