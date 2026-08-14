#!/bin/bash
#
# Dev-only: validate a Codemux release's desktop and hosted-web artifacts.
#
# This script does NOT build anything. It fetches artifacts from a completed
# GitHub release and asserts the shape — used after a test tag build finishes
# to verify the release.yml workflow's multi-platform latest.json merge
# behavior actually worked.
#
# Intended flow for smoke-testing release workflow changes before merging:
#   1. Create a throwaway tag: `git tag v0.0.0-test1 && git push origin v0.0.0-test1`
#   2. Wait for release.yml CI to complete (~15 min — Windows is the slow leg)
#   3. Run this script against the tag: ./scripts/test-release-pipeline.sh v0.0.0-test1
#   4. If all checks pass, merge the feature branch.
#   5. Delete the test release + tag afterward:
#        gh release delete v0.0.0-test1 --yes
#        git push origin :refs/tags/v0.0.0-test1
#
# Usage: ./scripts/test-release-pipeline.sh <tag>

set -euo pipefail

TAG="${1:?Usage: $0 <tag>}"
TMPDIR=$(mktemp -d)
trap "rm -rf $TMPDIR" EXIT

# Initialize booleans BEFORE the checks so the final summary can reference
# them unconditionally (`set -u` would otherwise unbind them on early exit).
HAS_LINUX=false
HAS_WINDOWS=false

echo "=== Release Pipeline Verification for $TAG ==="

# 1. Check release exists
echo "[1/9] Checking release exists..."
gh release view "$TAG" > /dev/null || { echo "FAIL: Release $TAG not found"; exit 1; }
echo "  OK"

# 2. Check Linux artifacts
echo "[2/9] Checking Linux artifacts..."
ASSETS=$(gh release view "$TAG" --json assets --jq '.assets[].name')
echo "$ASSETS" | grep -q "\.AppImage$" || { echo "FAIL: AppImage missing"; exit 1; }
echo "$ASSETS" | grep -q "\.deb$" || { echo "FAIL: deb missing"; exit 1; }
echo "$ASSETS" | grep -q "\.rpm$" || { echo "FAIL: rpm missing"; exit 1; }
echo "  OK: AppImage, deb, and rpm present"

# 3. Check Windows artifact
echo "[3/9] Checking Windows artifact..."
if echo "$ASSETS" | grep -q "x64-setup\.exe$"; then
    echo "  OK: NSIS .exe present"
    WINDOWS_EXE_PRESENT=true
else
    echo "  WARN: NSIS .exe missing — Windows leg of the matrix may have failed"
    echo "  Continuing checks on Linux artifacts only"
    WINDOWS_EXE_PRESENT=false
fi

# 4. Check latest.json exists
echo "[4/9] Checking latest.json exists..."
echo "$ASSETS" | grep -q "latest.json" || { echo "FAIL: latest.json missing"; exit 1; }
echo "  OK"

# 5. Download and validate latest.json structure
echo "[5/9] Validating latest.json structure..."
gh release download "$TAG" --pattern "latest.json" --dir "$TMPDIR" --clobber

# Check it's valid JSON
jq empty "$TMPDIR/latest.json" || { echo "FAIL: latest.json is not valid JSON"; exit 1; }

# Check version matches
VERSION=$(jq -r '.version' "$TMPDIR/latest.json")
echo "  Version in latest.json: $VERSION"
[ "$VERSION" = "$TAG" ] || echo "  WARN: version ($VERSION) doesn't match tag ($TAG) — check format"

echo "  OK: valid JSON"

# 6. CRITICAL: Check both platforms are present
echo "[6/9] CRITICAL: Checking both platforms in latest.json..."
PLATFORMS=$(jq -r '.platforms | keys[]' "$TMPDIR/latest.json" 2>/dev/null || true)

if [ -z "$PLATFORMS" ]; then
    echo "FAIL: No 'platforms' key in latest.json — updater format may be flat instead of multi-platform"
    echo "Dumping latest.json for inspection:"
    cat "$TMPDIR/latest.json"
    exit 1
fi

echo "  Platforms found:"
echo "$PLATFORMS" | sed 's/^/    - /'

echo "$PLATFORMS" | grep -q "linux" && HAS_LINUX=true
echo "$PLATFORMS" | grep -q "windows" && HAS_WINDOWS=true

if ! $HAS_LINUX; then
    echo "CRITICAL FAIL: linux platform missing from latest.json — Linux auto-updates would be BROKEN"
    exit 1
fi

if ! $HAS_WINDOWS; then
    echo "  WARN: windows platform missing from latest.json — Windows auto-updates won't work"
    echo "  This could mean: Windows build failed, or latest.json merge race occurred"
fi

echo "  OK: Linux=$HAS_LINUX, Windows=$HAS_WINDOWS"

# 7. Check signatures exist for each platform
echo "[7/9] Checking update signatures..."
while IFS= read -r platform; do
    [ -z "$platform" ] && continue
    SIG=$(jq -r --arg p "$platform" '.platforms[$p].signature // empty' "$TMPDIR/latest.json")
    if [ -z "$SIG" ]; then
        echo "  WARN: No signature for $platform — auto-update will fail for this platform"
    else
        echo "  OK: $platform has signature (${#SIG} chars)"
    fi
done <<< "$PLATFORMS"

# 8. Check download URLs are valid
echo "[8/9] Checking update download URLs..."
while IFS= read -r platform; do
    [ -z "$platform" ] && continue
    URL=$(jq -r --arg p "$platform" '.platforms[$p].url // empty' "$TMPDIR/latest.json")
    if [ -z "$URL" ]; then
        echo "  FAIL: No URL for $platform"
    else
        # Just verify it looks like an https URL. Don't actually download —
        # this script is read-only and should never pull multi-GB bundles.
        case "$URL" in
            https://*)
                echo "  OK: $platform -> $URL"
                ;;
            *)
                echo "  WARN: $platform URL doesn't look like https: $URL"
                ;;
        esac
    fi
done <<< "$PLATFORMS"

# 9. Check the pull-based hosted-web payload, its checksum, and its
# load-bearing iroh WASM files. The page can render without these WASM files,
# so their absence must fail verification rather than ship a dead relay path.
echo "[9/9] Checking hosted web release asset..."
WEB_VERSION="${TAG#v}"
WEB_ARCHIVE="codemux-web-${WEB_VERSION}.tar.gz"
WEB_CHECKSUM="${WEB_ARCHIVE}.sha256"

echo "$ASSETS" | grep -Fxq "$WEB_ARCHIVE" || {
    echo "FAIL: hosted web archive missing: $WEB_ARCHIVE"
    exit 1
}
echo "$ASSETS" | grep -Fxq "$WEB_CHECKSUM" || {
    echo "FAIL: hosted web checksum missing: $WEB_CHECKSUM"
    exit 1
}

gh release download "$TAG" \
    --pattern "$WEB_ARCHIVE" \
    --pattern "$WEB_CHECKSUM" \
    --dir "$TMPDIR" \
    --clobber
(cd "$TMPDIR" && sha256sum -c "$WEB_CHECKSUM")

tar -tzf "$TMPDIR/$WEB_ARCHIVE" > "$TMPDIR/web-archive-contents.txt"
grep -qE '^(\./)?index\.html$' "$TMPDIR/web-archive-contents.txt" || {
    echo "FAIL: hosted web archive has no index.html"
    exit 1
}
grep -qE '^(\./)?assets/.+\.js$' "$TMPDIR/web-archive-contents.txt" || {
    echo "FAIL: hosted web archive has no JavaScript assets"
    exit 1
}
grep -qE "^(\\./)?iroh-wasm/$TAG/iroh_wasm\\.js$" "$TMPDIR/web-archive-contents.txt" || {
    echo "FAIL: hosted web archive has no iroh WASM JavaScript glue"
    exit 1
}
grep -qE "^(\\./)?iroh-wasm/$TAG/iroh_wasm_bg\\.wasm$" "$TMPDIR/web-archive-contents.txt" || {
    echo "FAIL: hosted web archive has no iroh WASM binary"
    exit 1
}

mkdir -p "$TMPDIR/web"
tar --no-same-owner --no-same-permissions -xzf "$TMPDIR/$WEB_ARCHIVE" -C "$TMPDIR/web"
WASM_MAGIC=$(od -An -tx1 -N4 "$TMPDIR/web/iroh-wasm/$TAG/iroh_wasm_bg.wasm" | tr -d '[:space:]')
[ "$WASM_MAGIC" = "0061736d" ] || {
    echo "FAIL: hosted web archive contains an invalid WASM binary"
    exit 1
}
WASM_LOADER_ASSET=$(grep -Rl '/iroh-wasm' "$TMPDIR/web/assets" | sed -n '1p' || true)
[ -n "$WASM_LOADER_ASSET" ] && grep -Fq "$TAG" "$WASM_LOADER_ASSET" || {
    echo "FAIL: hosted frontend does not reference its versioned WASM glue"
    exit 1
}
echo "  OK: archive checksum and hosted relay payload are valid"

echo ""
echo "=== SUMMARY ==="
echo "Release: $TAG"
echo "Linux artifacts: present"
if $WINDOWS_EXE_PRESENT; then
    echo "Windows artifacts: present"
else
    echo "Windows artifacts: MISSING"
fi
echo "latest.json: valid"
echo "Linux in latest.json: $HAS_LINUX"
echo "Windows in latest.json: $HAS_WINDOWS"
echo "Hosted web artifact: present and valid"
echo ""

if $HAS_LINUX && $HAS_WINDOWS; then
    echo "ALL CHECKS PASSED"
    exit 0
else
    echo "ISSUES FOUND — review above before merging"
    exit 2
fi
