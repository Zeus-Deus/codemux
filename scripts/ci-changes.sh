#!/usr/bin/env bash
#
# Decide which CI jobs a change needs. Reads changed paths (repo-relative, one
# per line) on stdin and prints `key=true|false` lines for $GITHUB_OUTPUT:
#
#   code       anything other than docs/screenshots changed (web-checks)
#   rust       the desktop Rust crate or one of its inputs changed (rust)
#   iroh_wasm  the standalone browser relay crate or its build inputs changed
#
# Unknown paths count as `rust`, so a new top-level input is over-tested rather
# than silently skipped. Run from anywhere:
#   git diff --name-only --no-renames <base> HEAD | bash scripts/ci-changes.sh

set -euo pipefail

cd "$(dirname "$0")/.."

# Frontend files the Rust crate reads at compile/test time (include_str! of the
# chat workflow script, shared JSON fixtures, ...). Derived from the references
# in src-tauri so a new cross-language input keeps triggering Rust without
# anyone updating this script. Over-matching only runs Rust more often.
rust_frontend_inputs=()
while IFS= read -r path; do
  if [ -e "$path" ]; then rust_frontend_inputs+=("$path"); fi
done < <(
  grep -rhoE '(\.\./)+src/[A-Za-z0-9_./@-]*[A-Za-z0-9_@-]' src-tauri/src src-tauri/tests src-tauri/build.rs |
    sed -E 's#^(\.\./)+##' | sort -u
)

is_rust_frontend_input() {
  local input
  for input in "${rust_frontend_inputs[@]}"; do
    if [[ $1 == "$input" || $1 == "$input"/* ]]; then return 0; fi
  done
  return 1
}

code=false
rust=false
iroh_wasm=false

while IFS= read -r file; do
  [ -n "$file" ] || continue

  # Docs and screenshots: nothing builds or tests them.
  if [[ $file != */* && $file == *.md ]]; then continue; fi
  case $file in
    docs/* | design/* | .design/* | assets/* | .github/pr-assets/* | .github/pr-screenshots/* | */README.md)
      continue ;;
  esac
  code=true

  case $file in
    iroh-wasm/* | scripts/build-iroh-wasm.sh)
      # Standalone crate outside the Tauri Cargo graph.
      iroh_wasm=true
      continue ;;
    rust-toolchain | rust-toolchain.toml | .cargo/* | .github/workflows/ci.yml)
      iroh_wasm=true ;;
  esac

  case $file in
    src/* | public/* | index.html | vite.config.ts | vitest.setup.ts | tsconfig.json | components.json)
      if ! is_rust_frontend_input "$file"; then continue; fi ;;
  esac
  rust=true
done

printf 'code=%s\nrust=%s\niroh_wasm=%s\n' "$code" "$rust" "$iroh_wasm"
