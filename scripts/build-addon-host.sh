#!/usr/bin/env bash
# Build only the independent QuickJS executable. The app remains MSVC on Windows.
set -euo pipefail
cd "$(dirname "$0")/.."
addon_profile=release
if [[ $# -gt 0 ]]; then
  [[ $# == 2 && $1 == --profile && ( $2 == debug || $2 == release ) ]] || { echo 'Usage: build-addon-host.sh [--profile debug|release]' >&2; exit 2; }
  addon_profile=$2
fi
addon_rust_host=$(rustc -vV | sed -n 's/^host: //p')
case "$addon_rust_host" in
  x86_64-unknown-linux-gnu) addon_target=x86_64-unknown-linux-gnu; addon_name=codemux-addon-host-linux-x64; addon_suffix= ;;
  x86_64-pc-windows-*) addon_target=x86_64-pc-windows-gnu; addon_name=codemux-addon-host-windows-x64.exe; addon_suffix=.exe ;;
  *) echo "Unsupported add-on host target: $addon_rust_host" >&2; exit 2 ;;
esac
addon_flags=()
if [[ $addon_profile == release ]]; then addon_flags+=(--release); fi
addon_target_dir="$PWD/src-tauri/addon-host/target"
cargo build -j 2 --locked --manifest-path src-tauri/addon-host/Cargo.toml --target "$addon_target" --target-dir "$addon_target_dir" "${addon_flags[@]}"
addon_binary="$addon_target_dir/$addon_target/$addon_profile/codemux-addon-host$addon_suffix"
test -s "$addon_binary"
mkdir -p src-tauri/binaries
cp "$addon_binary" "src-tauri/binaries/$addon_name"
chmod 755 "src-tauri/binaries/$addon_name"
printf 'profile=%s\ntarget=%s\n' "$addon_profile" "$addon_target" > "src-tauri/binaries/.$addon_name.profile"
if [[ -n $addon_suffix ]]; then
  # No development-only MinGW runtime DLL may be assumed on an end-user machine.
  if objdump -p "$addon_binary" | sed -n 's/.*DLL Name: //p' | grep -Eiq 'lib(gcc|stdc|winpthread)'; then
    echo 'The add-on host depends on a development-only MinGW DLL' >&2; exit 1
  fi
fi
