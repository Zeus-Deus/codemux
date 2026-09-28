#!/usr/bin/env bash
set -euo pipefail
# Mount and fill only our small disposable runner fixture, never the host disk.
[[ "${GITHUB_ACTIONS:-}" == true && "${RUNNER_ENVIRONMENT:-}" == github-hosted ]]
addon_fullfs=$(mktemp -d "$RUNNER_TEMP/codemux-addon-fullfs.XXXXXX")
sudo mount -t tmpfs -o "size=16m,mode=0700,uid=$(id -u),gid=$(id -g)" tmpfs "$addon_fullfs"
trap 'sudo umount "$addon_fullfs"; rmdir "$addon_fullfs"' EXIT
CODEMUX_TEST_FULL_FS="$addon_fullfs" cargo test -j 2 --locked --manifest-path src-tauri/Cargo.toml --lib full_filesystem_preserves_release_data_and_grant_tuple -- --ignored --nocapture --test-threads=2
