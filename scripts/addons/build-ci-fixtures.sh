#!/usr/bin/env bash
# Build the CI-only race fixtures through the same packed public author tools
# as third-party packages. They are never included in a release or catalog.
set -euo pipefail
cd "$(dirname "$0")/../.."
fixture_pack_dir=$(mktemp -d)
trap 'rm -rf "$fixture_pack_dir"' EXIT
npm ci --ignore-scripts --prefix packages/plugin-sdk
npm ci --ignore-scripts --prefix packages/plugin-cli
npm run build --prefix packages/plugin-sdk
npm pack ./packages/plugin-sdk --pack-destination "$fixture_pack_dir"
npm pack ./packages/plugin-cli --pack-destination "$fixture_pack_dir"
for fixture in context-races activation-race; do
  fixture_dir=scripts/addons/fixtures/$fixture
  npm install --no-save --package-lock=false --ignore-scripts --prefix "$fixture_dir" "$fixture_pack_dir/codemux-plugin-sdk-1.0.0.tgz" "$fixture_pack_dir/codemux-plugin-cli-1.0.0.tgz"
  node "$fixture_dir/node_modules/@codemux/plugin-cli/src/cli.mjs" build "$fixture_dir"
  node "$fixture_dir/node_modules/@codemux/plugin-cli/src/cli.mjs" pack "$fixture_dir"
done
