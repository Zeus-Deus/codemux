#!/bin/bash
#
# Dev-only: validate a Codemux release's artifacts and latest.json structure.
#
# This script does NOT build anything. It fetches artifacts from a completed
# GitHub release and asserts the shape — used after a test tag build finishes
# to verify the release.yml workflow's multi-platform latest.json merge
# behavior actually worked.
#
# Intended flow for testing Windows support before merging to main:
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
echo "[1/8] Checking release exists..."
gh release view "$TAG" > /dev/null || { echo "FAIL: Release $TAG not found"; exit 1; }
echo "  OK"

# 2. Check Linux artifacts
echo "[2/8] Checking Linux artifacts..."
ASSETS=$(gh release view "$TAG" --json assets --jq '.assets[].name')
echo "$ASSETS" | grep -q "\.AppImage$" || { echo "FAIL: AppImage missing"; exit 1; }
echo "$ASSETS" | grep -q "\.deb$" || { echo "FAIL: deb missing"; exit 1; }
echo "  OK: AppImage and deb present"

# 3. Check Windows artifact
echo "[3/8] Checking Windows artifact..."
if echo "$ASSETS" | grep -q "x64-setup\.exe$"; then
    echo "  OK: NSIS .exe present"
    WINDOWS_EXE_PRESENT=true
else
    echo "  WARN: NSIS .exe missing — Windows leg of the matrix may have failed"
    echo "  Continuing checks on Linux artifacts only"
    WINDOWS_EXE_PRESENT=false
fi

# 4. Check latest.json exists
echo "[4/8] Checking latest.json exists..."
echo "$ASSETS" | grep -q "latest.json" || { echo "FAIL: latest.json missing"; exit 1; }
echo "  OK"

# 5. Download and validate latest.json structure
echo "[5/8] Validating latest.json structure..."
gh release download "$TAG" --pattern "latest.json" --dir "$TMPDIR" --clobber

# Check it's valid JSON
jq empty "$TMPDIR/latest.json" || { echo "FAIL: latest.json is not valid JSON"; exit 1; }

# Check version matches
VERSION=$(jq -r '.version' "$TMPDIR/latest.json")
echo "  Version in latest.json: $VERSION"
[ "$VERSION" = "$TAG" ] || echo "  WARN: version ($VERSION) doesn't match tag ($TAG) — check format"

echo "  OK: valid JSON"

# 6. CRITICAL: Check both platforms are present
echo "[6/8] CRITICAL: Checking both platforms in latest.json..."
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
echo "[7/8] Checking update signatures..."
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
echo "[8/8] Checking update download URLs..."
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
echo ""

if $HAS_LINUX && $HAS_WINDOWS; then
    echo "ALL CHECKS PASSED — safe to merge Windows support branch"
    exit 0
else
    echo "ISSUES FOUND — review above before merging"
    exit 2
fi
