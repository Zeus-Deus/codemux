#!/bin/bash
# Build the `codemux-remote` binary for the current target and stage it at
# src-tauri/binaries/codemux-remote-<target-triple> so Tauri's
# `bundle.resources = ["binaries/codemux-remote-*"]` glob has something to
# match and the bundler copies it into the deb/rpm/AppImage/NSIS output.
#
# PROFILE IS EXPLICIT AND MANDATORY-BY-DEFAULT.
#
#   ./scripts/build-codemux-remote.sh                    # release (default)
#   ./scripts/build-codemux-remote.sh --profile release  # bundling / CI
#   ./scripts/build-codemux-remote.sh --profile debug    # `tauri dev` only
#
# The script ALWAYS runs cargo itself for the requested profile and stages
# exactly the artifact that build produced. There is deliberately no
# "use whatever binary happens to exist" fallback chain: that is what
# previously let `tauri build` ship an ~800 MB unstripped debug binary,
# because `beforeBuildCommand` runs BEFORE tauri's own `cargo build
# --release`, so at staging time the only codemux-remote on disk was a
# leftover `target/debug/` one from a prior `tauri dev`.
#
# Wired up in tauri.conf.json:
#   beforeDevCommand   -> --profile debug   (fast inner loop)
#   beforeBuildCommand -> --profile release (what actually ships)
#
# `--profile debug` keeps a cheap escape hatch — if something is already
# staged it is reused as-is, because `tauri dev` only needs the glob to
# resolve. That shortcut is confined to the debug path; `--profile release`
# never reads the staged file, it only ever writes the artifact it just
# built. The two concerns do not touch.
#
# Provenance stamp: after staging, the script writes
# src-tauri/binaries/.codemux-remote-<triple>.profile describing what was
# staged. The leading dot keeps it out of the `codemux-remote-*` bundle
# glob. release.yml asserts `profile=release` there before bundling, so a
# wrong-profile sidecar cannot silently ship again.
#
# Pattern mirrors copy-agent-browser.sh and build-claude-sidecar.sh.

set -euo pipefail

PROFILE="release"

while [ $# -gt 0 ]; do
  case "$1" in
    --profile)   PROFILE="${2:-}" ; shift 2 ;;
    --profile=*) PROFILE="${1#*=}" ; shift ;;
    *)
      echo "[build-codemux-remote] ERROR: unknown argument: $1" >&2
      echo "[build-codemux-remote] usage: $0 [--profile release|debug]" >&2
      exit 2
      ;;
  esac
done

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

# Dev fast path. `tauri dev` only needs *a* binary at the glob target so the
# resource scan resolves and the push-to-host code path has something to
# read; it does not care which profile produced it, and rebuilding it on
# every dev restart is pure latency. Reuse whatever is already staged.
#
# This can never contaminate a release: the --profile release path below
# never looks at $DST, it stages the artifact cargo just built. And the
# stamp records `profile=reused` so release.yml's `profile=release`
# assertion rejects a dev-staged file outright.
if [ "$PROFILE" = "debug" ] && [ -s "$DST" ]; then
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
if [ ! -e "$DST" ]; then
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

echo "[build-codemux-remote] building codemux-remote (profile: $PROFILE, target: $TARGET)"
if [ "$PROFILE" = "release" ]; then
  cargo build --release --bin codemux-remote --manifest-path "$MANIFEST"
else
  cargo build --bin codemux-remote --manifest-path "$MANIFEST"
fi

# Provenance: the ONLY file we are willing to stage is the one cargo just
# produced for the requested profile. No fallbacks, no "prefer release then
# debug" scan — if it isn't here, something is wrong and we stop.
if [ ! -f "$SRC" ]; then
  echo "[build-codemux-remote] ERROR: cargo reported success but no artifact at:" >&2
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
# on the *next* cargo invocation — i.e. `tauri build` would recompile the
# whole crate again right after beforeBuildCommand staged this binary.
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

trap - EXIT
echo "[build-codemux-remote] staged $SRC -> $DST (profile=$PROFILE, ${SIZE} bytes)"
