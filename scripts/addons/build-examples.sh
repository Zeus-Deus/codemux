#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
addon_pack_dir=$(mktemp -d)
trap 'rm -rf "$addon_pack_dir"' EXIT
npm ci --ignore-scripts --prefix packages/plugin-sdk
npm ci --ignore-scripts --prefix packages/plugin-cli
npm run build --prefix packages/plugin-sdk
npm pack ./packages/plugin-sdk --pack-destination "$addon_pack_dir"
npm pack ./packages/plugin-cli --pack-destination "$addon_pack_dir"
for addon in project-brief issue-companion; do
  npm install --no-save --package-lock=false --ignore-scripts --prefix "examples/addons/$addon" "$addon_pack_dir/codemux-plugin-sdk-1.0.0.tgz" "$addon_pack_dir/codemux-plugin-cli-1.0.0.tgz"
  npm run build --prefix "examples/addons/$addon"
  npm run check --prefix "examples/addons/$addon"
  npm run pack --prefix "examples/addons/$addon"
done
