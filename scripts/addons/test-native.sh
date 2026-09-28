#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
bash scripts/build-addon-host.sh --profile debug
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) export CODEMUX_TEST_ADDON_HOST="$PWD/src-tauri/binaries/codemux-addon-host-windows-x64.exe" ;;
  *) export CODEMUX_TEST_ADDON_HOST="$PWD/src-tauri/binaries/codemux-addon-host-linux-x64" ;;
esac
cargo test -j 2 --manifest-path src-tauri/Cargo.toml --lib addons:: -- --test-threads=2
# Requires scripts/addons/build-examples.sh first.
cargo test -j 2 --manifest-path src-tauri/Cargo.toml --lib addons::manager::tests::native_ -- --ignored --test-threads=2
cargo test -j 2 --manifest-path src-tauri/Cargo.toml --lib addons_boundary_tests -- --test-threads=2
