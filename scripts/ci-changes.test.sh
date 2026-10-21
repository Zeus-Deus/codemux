#!/usr/bin/env bash
#
# Unit tests for scripts/ci-changes.sh, the CI changed-path classifier.
# Run with:
#   bash scripts/ci-changes.test.sh

set -uo pipefail

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
PASS=0
FAIL=0

expect() { # <label> <expected outputs> <changed paths...>
  local label=$1 expected=$2 actual
  shift 2
  actual=$(printf '%s\n' "$@" | bash "$SCRIPT_DIR/ci-changes.sh" | tr '\n' ' ')
  if [ "$actual" = "$expected " ]; then
    PASS=$((PASS + 1))
  else
    FAIL=$((FAIL + 1))
    echo "FAIL: $label"
    echo "  expected: $expected"
    echo "  actual:   $actual"
  fi
}

none='code=false rust=false iroh_wasm=false'
web='code=true rust=false iroh_wasm=false'
desktop='code=true rust=true iroh_wasm=false'
iroh='code=true rust=false iroh_wasm=true'
all='code=true rust=true iroh_wasm=true'

expect 'empty diff' "$none"
expect 'docs and screenshots only' "$none" \
  README.md AGENTS.md docs/plans/x.md docs/features/a.png design/mock.html \
  .design/top-bar.dc.html assets/logo/x.png .github/pr-screenshots/a.png \
  .github/pr-assets/b/after.jpg scripts/e2e/remote-bootstrap/README.md
expect 'frontend only' "$web" \
  src/components/chat/x.tsx src/lib/foo.test.ts public/favicon.svg index.html \
  vite.config.ts tsconfig.json
expect 'chat workflow script is a Rust input' "$desktop" src/lib/workflows/chat-workflow.script.js
expect 'shared fixtures are Rust inputs' "$desktop" \
  src/lib/agent-chat/__fixtures__/opencode-native-image-tool-result.json
expect 'rust source' "$desktop" src-tauri/src/lib.rs
expect 'hermes sidecar' "$desktop" sidecar/managed-workflow/hermes.py
expect 'plugin catalog' "$desktop" catalog/plugins/catalog-v1.json
expect 'npm lockfile' "$desktop" package-lock.json
expect 'unknown top-level path' "$desktop" .gitattributes
expect 'iroh-wasm crate' "$iroh" iroh-wasm/src/lib.rs
expect 'iroh-wasm build script' "$iroh" scripts/build-iroh-wasm.sh
expect 'toolchain file' "$all" rust-toolchain.toml
expect 'ci workflow' "$all" .github/workflows/ci.yml
expect 'docs plus frontend' "$web" README.md src/App.tsx

echo "ci-changes: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
