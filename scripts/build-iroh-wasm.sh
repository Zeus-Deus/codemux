#!/bin/bash
# Build the browser (WASM) iroh relay client and stage the generated web-target
# glue + wasm into public/iroh-wasm/ so `vite build` copies it to the deploy
# root (served at /iroh-wasm/…). The artifact is multi-MB and gitignored; the
# main build stays green without it (relay mode degrades to "build the wasm",
# the LAN/mesh WebSocket path is the default). Mirrors how the repo treats the
# bundled agent sidecar: a build script produces a bundled binary, not committed.
#
# The toolchain is isolated: wasm-bindgen-cli is installed into a local dir under
# iroh-wasm/.tools (pinned to the crate's wasm-bindgen version) so it can't
# collide with a globally-installed, mismatched CLI. Requires rustup (to add the
# wasm32-unknown-unknown target) plus a network connection on first run.
#
# Usage: scripts/build-iroh-wasm.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CRATE_DIR="$REPO_ROOT/iroh-wasm"
OUT_DIR="$REPO_ROOT/public/iroh-wasm"
TOOLS_DIR="$CRATE_DIR/.tools"
TARGET="wasm32-unknown-unknown"

log() { echo "[build-iroh-wasm] $*"; }
die() { echo "[build-iroh-wasm] ERROR: $*" >&2; exit 1; }

# wasm-bindgen-cli MUST match the crate's pinned `wasm-bindgen` exactly.
WASM_BINDGEN_VERSION="$(
  grep -E '^wasm-bindgen = "=' "$CRATE_DIR/Cargo.toml" | head -1 |
    sed -E 's/.*"=?([0-9.]+)".*/\1/'
)"
[ -n "$WASM_BINDGEN_VERSION" ] || die "could not read wasm-bindgen version from Cargo.toml"
log "wasm-bindgen version: $WASM_BINDGEN_VERSION"

command -v cargo >/dev/null 2>&1 || die "cargo not found — install Rust."

# ── 1. wasm target ──────────────────────────────────────────────────
if ! rustc --print target-list 2>/dev/null | grep -qx "$TARGET"; then
  die "rustc doesn't know target $TARGET"
fi
# Presence of the target's std is what actually matters: check the sysroot's
# rustlib dir (works with or without rustup).
std_installed() {
  local sysroot
  sysroot="$(rustc --print sysroot 2>/dev/null)" || return 1
  [ -d "$sysroot/lib/rustlib/$TARGET/lib" ]
}
if ! std_installed; then
  if command -v rustup >/dev/null 2>&1; then
    log "adding $TARGET via rustup"
    rustup target add "$TARGET"
  else
    die "the $TARGET std is not installed and rustup is unavailable. Install rustup (https://rustup.rs), or your distro's rust-std-$TARGET package, then re-run."
  fi
fi

# ── 2. isolated, version-matched wasm-bindgen-cli ───────────────────
WASM_BINDGEN="$TOOLS_DIR/bin/wasm-bindgen"
current_cli_version() { "$WASM_BINDGEN" --version 2>/dev/null | awk '{print $2}'; }
if [ "$(current_cli_version)" != "$WASM_BINDGEN_VERSION" ]; then
  log "installing wasm-bindgen-cli@$WASM_BINDGEN_VERSION into $TOOLS_DIR"
  cargo install wasm-bindgen-cli \
    --version "$WASM_BINDGEN_VERSION" \
    --root "$TOOLS_DIR" \
    --locked 2>/dev/null ||
    cargo install wasm-bindgen-cli --version "$WASM_BINDGEN_VERSION" --root "$TOOLS_DIR"
fi

# ── 3. compile the crate to wasm ────────────────────────────────────
# getrandom 0.3/0.4 need the browser backend selected via this cfg (paired with
# the crates' `wasm_js` features in Cargo.toml); 0.2 uses its `js` feature.
export RUSTFLAGS="${RUSTFLAGS:-} --cfg getrandom_backend=\"wasm_js\""
log "compiling (release, opt-level=z) for $TARGET"
(cd "$CRATE_DIR" && cargo build --release --target "$TARGET")

WASM_IN="$CRATE_DIR/target/$TARGET/release/iroh_wasm.wasm"
[ -f "$WASM_IN" ] || die "expected wasm at $WASM_IN not found"

# ── 4. generate the web-target JS glue + bindings ───────────────────
rm -rf "$OUT_DIR"
mkdir -p "$OUT_DIR"
log "running wasm-bindgen (--target web) → $OUT_DIR"
"$WASM_BINDGEN" \
  --target web \
  --out-dir "$OUT_DIR" \
  --out-name iroh_wasm \
  --no-typescript \
  "$WASM_IN"

# ── 5. optional size pass (binaryen wasm-opt) ───────────────────────
WASM_OUT="$OUT_DIR/iroh_wasm_bg.wasm"
if command -v wasm-opt >/dev/null 2>&1; then
  log "wasm-opt -Oz"
  wasm-opt -Oz --strip-debug "$WASM_OUT" -o "$WASM_OUT.opt" && mv "$WASM_OUT.opt" "$WASM_OUT"
else
  log "wasm-opt not found — skipping (install binaryen for a smaller artifact)"
fi

# ── 6. report sizes (raw + brotli, the CDN transfer size) ───────────
raw_bytes=$(wc -c <"$WASM_OUT")
log "artifact: $WASM_OUT"
log "  raw:    $(numfmt --to=iec --suffix=B "$raw_bytes" 2>/dev/null || echo "${raw_bytes}B")"
if command -v brotli >/dev/null 2>&1; then
  br_bytes=$(brotli -q 11 -c "$WASM_OUT" | wc -c)
  log "  brotli: $(numfmt --to=iec --suffix=B "$br_bytes" 2>/dev/null || echo "${br_bytes}B")"
fi
log "done. Files in $OUT_DIR:"
ls -la "$OUT_DIR"
