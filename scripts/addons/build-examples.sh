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
  # Resolve from scratch; an older local tree could keep a drifted transitive version.
  rm -rf "examples/addons/$addon/node_modules"
  npm install --no-save --package-lock=false --ignore-scripts --prefix "examples/addons/$addon" "$addon_pack_dir/codemux-plugin-sdk-1.0.0.tgz" "$addon_pack_dir/codemux-plugin-cli-1.0.0.tgz"
  # The bundled Remote DOM and Preact versions must equal the reviewed SDK lockfile.
  node scripts/addons/sdk-lock.mjs "examples/addons/$addon" packages/plugin-sdk/package-lock.json
  npm run build --prefix "examples/addons/$addon"
  npm run check --prefix "examples/addons/$addon"
  npm run pack --prefix "examples/addons/$addon"
done
# Build the hostile CI fixture through the same packed public author tools.
# It is never included in a release workflow or catalog listing.
addon_fixture=scripts/addons/fixtures/fault-isolation
rm -rf "$addon_fixture/node_modules"
npm install --no-save --package-lock=false --ignore-scripts --prefix "$addon_fixture" "$addon_pack_dir/codemux-plugin-sdk-1.0.0.tgz" "$addon_pack_dir/codemux-plugin-cli-1.0.0.tgz"
node scripts/addons/sdk-lock.mjs "$addon_fixture" packages/plugin-sdk/package-lock.json
node "$addon_fixture/node_modules/@codemux/plugin-cli/src/cli.mjs" build "$addon_fixture"
node "$addon_fixture/node_modules/@codemux/plugin-cli/src/cli.mjs" pack "$addon_fixture"
# Author tools: manifest parity with the desktop validator, and the starter
# built from the same tarballs in a directory outside this checkout.
addon_packs=$addon_pack_dir
if command -v cygpath >/dev/null; then addon_packs=$(cygpath -m "$addon_pack_dir"); fi
CODEMUX_AUTHOR_TARBALLS="$addon_packs" \
  node --test packages/plugin-cli/tests/validate.test.mjs packages/plugin-cli/tests/author.test.mjs
