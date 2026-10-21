#!/bin/bash
# Build the `codemux-remote` binary for the current target and stage it at
# src-tauri/binaries/codemux-remote-<target-triple> so Tauri's
# `bundle.resources = ["binaries/codemux-remote-*"]` glob has something to
# match and the bundler copies it into the deb/rpm/AppImage/NSIS output.
#
# THREE MODES, one per caller:
#
#   ./scripts/build-codemux-remote.sh                      # release (default)
#   ./scripts/build-codemux-remote.sh --profile release    # standalone build
#   ./scripts/build-codemux-remote.sh --profile debug      # `tauri dev` only
#   ./scripts/build-codemux-remote.sh --stub               # beforeBuildCommand
#   ./scripts/build-codemux-remote.sh --stage-tauri-build  # beforeBundleCommand
#
# `--profile` runs cargo itself for the requested profile and stages exactly
# the artifact that build produced. There is deliberately no "use whatever
# binary happens to exist" fallback chain: that is what previously let
# `tauri build` ship an ~800 MB unstripped debug binary, because
# `beforeBuildCommand` runs BEFORE tauri's own `cargo build --release`, so at
# staging time the only codemux-remote on disk was a leftover `target/debug/`
# one from a prior `tauri dev`.
#
# `tauri build` does not need its own cargo pass: it runs
# `cargo build --bins`, which already builds codemux-remote with the same
# profile as the app. Running cargo here as well recompiled the whole
# `codemux` crate a second time (~6 min Linux, ~13 min Windows per release).
# So `tauri build` uses two hooks instead:
#   beforeBuildCommand -> --stub              plant a zero-byte file so
#                                             tauri-build's resource glob
#                                             resolves at compile time
#   beforeBundleCommand -> --stage-tauri-build stage the binary tauri's cargo
#                                             pass just produced, before any
#                                             bundle is written or uploaded
# The profile for --stage-tauri-build comes from tauri itself
# (TAURI_ENV_DEBUG=true for `tauri build --debug`), never from what happens to
# be on disk.
#
# Wired up in tauri.conf.json:
#   beforeDevCommand    -> --profile debug   (fast inner loop)
#   beforeBuildCommand  -> --stub
#   beforeBundleCommand -> --stage-tauri-build
#
# `--profile debug` keeps a cheap escape hatch — if something is already
# staged it is reused as-is, because `tauri dev` only needs the glob to
# resolve. That shortcut is confined to the debug path; the other modes
# never read the staged file, they only ever write the artifact cargo built.
#
# Provenance stamp: after staging, the script writes
# src-tauri/binaries/.codemux-remote-<triple>.profile describing what was
# staged. The leading dot keeps it out of the `codemux-remote-*` bundle
# glob. --stage-tauri-build under a non-debug `tauri build` fails
# unless that stamp says `profile=release` and matches the staged bytes, and
# a failed beforeBundleCommand aborts `tauri build` before bundling — so in
# release.yml nothing is uploaded with a wrong-profile or truncated sidecar.
#
# Pattern mirrors copy-agent-browser.sh and build-claude-sidecar.sh.

set -euo pipefail

PROFILE="release"
MODE="build"

while [ $# -gt 0 ]; do
  case "$1" in
    --profile)           PROFILE="${2:-}" ; shift 2 ;;
    --profile=*)         PROFILE="${1#*=}" ; shift ;;
    --stub)              MODE="stub" ; shift ;;
    --stage-tauri-build) MODE="stage" ; shift ;;
    *)
      echo "[build-codemux-remote] ERROR: unknown argument: $1" >&2
      echo "[build-codemux-remote] usage: $0 [--profile release|debug | --stub | --stage-tauri-build]" >&2
      exit 2
      ;;
  esac
done

# tauri sets TAURI_ENV_DEBUG=true for its hooks only under `tauri build
# --debug`; its cargo pass then wrote target/debug, otherwise target/release.
if [ "$MODE" = "stage" ]; then
  if [ "${TAURI_ENV_DEBUG:-}" = "true" ]; then
    PROFILE="debug"
  else
    PROFILE="release"
  fi
fi

case "$PROFILE" in
  release|debug) ;;
  *)
    echo "[build-codemux-remote] ERROR: --profile must be 'release' or 'debug' (got '$PROFILE')" >&2
    exit 2
    ;;
esac

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
MANIFEST="$REPO_ROOT/src-tauri/Cargo.toml"
BINDIR="$REPO_ROOT/src-tauri/binaries"

mkdir -p "$BINDIR"

# Detect target triple. Honors CARGO_BUILD_TARGET when cross-compiling.
TARGET="${CARGO_BUILD_TARGET:-$(rustc -vV | grep host | cut -d' ' -f2)}"

# Where cargo will put the artifact for this profile. Mirrors cargo's own
# layout rules: CARGO_TARGET_DIR (if set) or <manifest dir>/target, plus a
# <triple>/ level only when CARGO_BUILD_TARGET forces a cross build.
TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/src-tauri/target}"
if [ -n "${CARGO_BUILD_TARGET:-}" ]; then
  OUT_DIR="$TARGET_DIR/$CARGO_BUILD_TARGET/$PROFILE"
else
  OUT_DIR="$TARGET_DIR/$PROFILE"
fi

case "$TARGET" in
  *windows*) EXE=".exe" ;;
  *)         EXE="" ;;
esac

SRC="$OUT_DIR/codemux-remote$EXE"
DST="$BINDIR/codemux-remote-$TARGET$EXE"
STAMP="$BINDIR/.codemux-remote-$TARGET.profile"

# Write the provenance stamp, but only when its content actually changes:
# src-tauri/binaries/ is a tracked input of tauri-build's resource scan, and
# needless mtime churn in there reruns build.rs on the next cargo invocation.
write_stamp() {
  body="profile=$1
target=$TARGET
source=$2
staged=$DST
bytes=$3"
  if [ "$(cat "$STAMP" 2>/dev/null || true)" != "$body" ]; then
    printf '%s\n' "$body" > "$STAMP"
  fi
}

# beforeBuildCommand: tauri's cargo pass only needs the resource glob to
# resolve. Never touch an existing file — rewriting it would rerun build.rs.
if [ "$MODE" = "stub" ]; then
  if [ ! -e "$DST" ]; then
    # A stamp without its file is stale; it must not vouch for the stub.
    rm -f "$STAMP"
    : > "$DST"
    echo "[build-codemux-remote] planted zero-byte stub at $DST; beforeBundleCommand stages the real binary"
  fi
  exit 0
fi

# Dev fast path. `tauri dev` only needs *a* binary at the glob target so the
# resource scan resolves and the push-to-host code path has something to
# read; it does not care which profile produced it, and rebuilding it on
# every dev restart is pure latency. Reuse whatever is already staged.
#
# This can never contaminate a release: only `--profile debug` takes this
# path. The release and --stage-tauri-build paths never look at $DST, they
# stage the artifact cargo just built.
if [ "$MODE" = "build" ] && [ "$PROFILE" = "debug" ] && [ -s "$DST" ]; then
  SIZE="$(wc -c < "$DST" | tr -d ' ')"
  # Leave the existing stamp alone when it still describes this exact file —
  # it stays truthful because this path does not modify $DST, and not
  # rewriting it avoids an mtime bump that would rerun build.rs. If the file
  # changed behind our back, drop the stamp rather than vouch for it.
  if ! grep -qx "bytes=$SIZE" "$STAMP" 2>/dev/null; then
    rm -f "$STAMP"
  fi
  echo "[build-codemux-remote] dev fast path: reusing already-staged $DST (${SIZE} bytes)"
  echo "[build-codemux-remote] run with --profile release to stage a bundle-quality binary"
  exit 0
fi

# Chicken-and-egg: `codemux-remote` is a [[bin]] inside the same Cargo
# package as the `codemux` Tauri app, so `cargo build --bin codemux-remote`
# triggers that package's build.rs (tauri_build::build()). That build script
# scans tauri.conf.json's `bundle.resources = ["binaries/codemux-remote-*"]`
# glob and fails if nothing matches — but on a fresh checkout nothing matches
# *until* we build the binary. Plant an empty stub at the glob target so the
# resource scan passes; the real binary overwrites it below.
PLANTED_STUB=0
if [ "$MODE" = "build" ] && [ ! -e "$DST" ]; then
  echo "[build-codemux-remote] planting stub at $DST so tauri-build's resource glob resolves"
  : > "$DST"
  PLANTED_STUB=1
fi

# On failure: drop the provenance stamp so a stale "profile=release" claim
# can never outlive the file it described, and remove the zero-byte stub if
# we planted one (a later `tauri build` would otherwise bundle a 0-byte
# sidecar quite happily).
cleanup_failure() {
  status=$?
  if [ "$status" -ne 0 ]; then
    rm -f "$STAMP"
    if [ "$PLANTED_STUB" -eq 1 ]; then
      rm -f "$DST"
      echo "[build-codemux-remote] build failed — removed the zero-byte stub at $DST" >&2
    fi
  fi
  exit "$status"
}
trap cleanup_failure EXIT

if [ "$MODE" = "stage" ]; then
  echo "[build-codemux-remote] staging the codemux-remote that tauri build just compiled (profile: $PROFILE, target: $TARGET)"
else
  echo "[build-codemux-remote] building codemux-remote (profile: $PROFILE, target: $TARGET)"
  if [ "$PROFILE" = "release" ]; then
    cargo build --release --bin codemux-remote --manifest-path "$MANIFEST"
  else
    cargo build --bin codemux-remote --manifest-path "$MANIFEST"
  fi
fi

# Provenance: the ONLY file we are willing to stage is the one cargo just
# produced for the requested profile. No fallbacks, no "prefer release then
# debug" scan — if it isn't here, something is wrong and we stop.
if [ ! -f "$SRC" ]; then
  echo "[build-codemux-remote] ERROR: no cargo artifact for this profile at:" >&2
  echo "[build-codemux-remote]   $SRC" >&2
  echo "[build-codemux-remote] profile=$PROFILE target=$TARGET target-dir=$TARGET_DIR" >&2
  exit 1
fi
if [ ! -s "$SRC" ]; then
  echo "[build-codemux-remote] ERROR: cargo artifact is empty: $SRC" >&2
  exit 1
fi

# Only rewrite the staged file when it differs. src-tauri/binaries/ is a
# tracked input of tauri-build's resource scan, so an unconditional `cp`
# bumps the mtime, reruns build.rs and forces a full relink of codemux_lib
# on the *next* cargo invocation (the next `tauri dev` or `tauri build`).
if cmp -s "$SRC" "$DST"; then
  echo "[build-codemux-remote] staged file already identical — leaving mtime alone"
else
  cp "$SRC" "$DST"
fi
chmod +x "$DST"

# Cheap post-copy integrity check: the staged file must be byte-identical to
# what we just built (catches a truncated copy or a full disk).
if ! cmp -s "$SRC" "$DST"; then
  echo "[build-codemux-remote] ERROR: staged file differs from the build artifact" >&2
  echo "[build-codemux-remote]   src: $SRC" >&2
  echo "[build-codemux-remote]   dst: $DST" >&2
  exit 1
fi

SIZE="$(wc -c < "$DST" | tr -d ' ')"
write_stamp "$PROFILE" "$SRC" "$SIZE"

# Release gate for `tauri build`: this hook is the last step before bundling
# and upload, so assert the stamp's claims against the staged file itself.
if [ "$MODE" = "stage" ] && [ "$PROFILE" = "release" ]; then
  if ! grep -qx 'profile=release' "$STAMP" ||
     ! grep -qx "bytes=$(wc -c < "$DST" | tr -d ' ')" "$STAMP"; then
    echo "[build-codemux-remote] ERROR: provenance stamp does not describe a release build of $DST — refusing to bundle" >&2
    cat "$STAMP" >&2 || true
    exit 1
  fi
  cat "$STAMP"
fi

trap - EXIT
echo "[build-codemux-remote] staged $SRC -> $DST (profile=$PROFILE, ${SIZE} bytes)"
