#!/usr/bin/env bash
#
# Unit tests for the pure logic in scripts/install.sh — artifact naming,
# version normalisation, distro-family detection (against mocked os-release
# files) and install-method selection, plus a network-free --dry-run smoke
# test of the whole script.
#
# No network, no package manager, nothing installed. Run with:
#   bash scripts/install-sh.test.sh

set -uo pipefail

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
INSTALL_SH="${SCRIPT_DIR}/install.sh"

# Source the installer for its helpers only.
CODEMUX_INSTALL_SH_NO_MAIN=1
export CODEMUX_INSTALL_SH_NO_MAIN
NO_COLOR=1
export NO_COLOR
# shellcheck source=/dev/null
. "$INSTALL_SH"
set +e

PASS=0
FAIL=0
TMPROOT=$(mktemp -d "${TMPDIR:-/tmp}/codemux-install-test.XXXXXX") || exit 1
trap 'rm -rf "$TMPROOT"' EXIT

is() { # <label> <actual> <expected>
    if [ "$2" = "$3" ]; then
        printf '  ok   %s\n' "$1"
        PASS=$((PASS + 1))
    else
        printf '  FAIL %s\n       expected: %s\n       actual:   %s\n' "$1" "$3" "$2"
        FAIL=$((FAIL + 1))
    fi
}

contains() { # <label> <haystack> <needle>
    case "$2" in
        *"$3"*)
            printf '  ok   %s\n' "$1"
            PASS=$((PASS + 1))
            ;;
        *)
            printf '  FAIL %s\n       expected to contain: %s\n       actual:              %s\n' "$1" "$3" "$2"
            FAIL=$((FAIL + 1))
            ;;
    esac
}

not_contains() { # <label> <haystack> <needle>
    case "$2" in
        *"$3"*)
            printf '  FAIL %s\n       expected NOT to contain: %s\n' "$1" "$3"
            FAIL=$((FAIL + 1))
            ;;
        *)
            printf '  ok   %s\n' "$1"
            PASS=$((PASS + 1))
            ;;
    esac
}

# Write a fake /etc/os-release and echo its path.
mock_os_release() { # <name> <body...>
    local name="$1"; shift
    local path="${TMPROOT}/os-release.${name}"
    printf '%s\n' "$@" > "$path"
    printf '%s' "$path"
}

# ── version normalisation ────────────────────────────────────────────────────

echo "version normalisation"
is "strips a leading v"            "$(codemux_normalize_version v0.15.5)"   "0.15.5"
is "leaves a bare version alone"   "$(codemux_normalize_version 0.15.5)"    "0.15.5"
is "tag adds v"                    "$(codemux_tag_for_version 0.15.5)"      "v0.15.5"
is "tag is idempotent"             "$(codemux_tag_for_version v0.15.5)"     "v0.15.5"
is "prerelease survives"           "$(codemux_normalize_version v1.0.0-rc1)" "1.0.0-rc1"

# ── version pinning ──────────────────────────────────────────────────────────
#
# A running Codemux exports CODEMUX=1 and CODEMUX_VERSION=<its own version>
# into every terminal pane, so CODEMUX_VERSION must not be read as a pin there.

echo "version pinning"
is "nothing set -> latest" \
   "$(env -u CODEMUX_VERSION -u CODEMUX_INSTALL_VERSION -u CODEMUX bash -c ". '$INSTALL_SH'; codemux_pinned_version")" \
   ""
is "CODEMUX_VERSION pins outside a pane" \
   "$(env -u CODEMUX -u CODEMUX_INSTALL_VERSION CODEMUX_VERSION=v1.2.3 bash -c ". '$INSTALL_SH'; codemux_pinned_version")" \
   "1.2.3"
is "CODEMUX_VERSION is ignored inside a pane" \
   "$(env -u CODEMUX_INSTALL_VERSION CODEMUX=1 CODEMUX_VERSION=0.15.5 bash -c ". '$INSTALL_SH'; codemux_pinned_version")" \
   ""
is "CODEMUX_INSTALL_VERSION wins inside a pane" \
   "$(env CODEMUX=1 CODEMUX_VERSION=0.15.5 CODEMUX_INSTALL_VERSION=0.16.0 bash -c ". '$INSTALL_SH'; codemux_pinned_version")" \
   "0.16.0"

# ── artifact names ───────────────────────────────────────────────────────────
#
# Ground truth is the tauri-action bundle output of
# .github/workflows/release.yml on ubuntu-22.04 / x86_64, as published on the
# v0.15.5 release:
#   codemux_0.15.5_amd64.deb
#   codemux-0.15.5-1.x86_64.rpm
#   codemux_0.15.5_amd64.AppImage

echo "artifact names"
is "deb name"      "$(codemux_artifact_name deb 0.15.5)"       "codemux_0.15.5_amd64.deb"
is "rpm name"      "$(codemux_artifact_name rpm 0.15.5)"       "codemux-0.15.5-1.x86_64.rpm"
is "AppImage name" "$(codemux_artifact_name appimage 0.15.5)"  "codemux_0.15.5_amd64.AppImage"
is "tag input is normalised" "$(codemux_artifact_name deb v0.15.5)" "codemux_0.15.5_amd64.deb"

codemux_artifact_name exe 0.15.5 >/dev/null 2>&1
is "unknown kind fails" "$?" "1"

echo "artifact urls"
is "deb url" "$(codemux_artifact_url deb 0.15.5)" \
   "https://github.com/Zeus-Deus/codemux/releases/download/v0.15.5/codemux_0.15.5_amd64.deb"
is "rpm url" "$(codemux_artifact_url rpm v0.15.5)" \
   "https://github.com/Zeus-Deus/codemux/releases/download/v0.15.5/codemux-0.15.5-1.x86_64.rpm"
is "AppImage url" "$(codemux_artifact_url appimage 0.15.5)" \
   "https://github.com/Zeus-Deus/codemux/releases/download/v0.15.5/codemux_0.15.5_amd64.AppImage"

echo "artifact kind from filename"
is "deb file"        "$(codemux_kind_from_filename /tmp/codemux_0.15.5_amd64.deb)"      "deb"
is "rpm file"        "$(codemux_kind_from_filename codemux-0.15.5-1.x86_64.rpm)"        "rpm"
is "AppImage file"   "$(codemux_kind_from_filename codemux_0.15.5_amd64.AppImage)"      "appimage"
is "lowercase ext"   "$(codemux_kind_from_filename ./build/codemux.appimage)"           "appimage"
is "url works too"   "$(codemux_kind_from_filename https://example.test/a/b.deb)"       "deb"
is "unknown ext"     "$(codemux_kind_from_filename codemux_0.15.5_x64-setup.exe)"       "unknown"

# ── os-release parsing ───────────────────────────────────────────────────────

echo "os-release parsing"
UBUNTU=$(mock_os_release ubuntu \
    'NAME="Ubuntu"' \
    'ID=ubuntu' \
    'ID_LIKE=debian' \
    'PRETTY_NAME="Ubuntu 22.04.4 LTS"' \
    'VERSION_ID="22.04"')
is "reads ID"           "$(codemux_os_release_field "$UBUNTU" ID)"          "ubuntu"
is "strips quotes"      "$(codemux_os_release_field "$UBUNTU" PRETTY_NAME)" "ubuntu 22.04.4 lts"
is "ID= does not match ID_LIKE=" "$(codemux_os_release_field "$UBUNTU" ID)" "ubuntu"
is "missing key is empty" "$(codemux_os_release_field "$UBUNTU" NOPE)"      ""
is "missing file is empty" "$(codemux_os_release_field "${TMPROOT}/nope" ID)" ""

# ── distro family detection ──────────────────────────────────────────────────

echo "distro family detection"
is "ubuntu -> debian" "$(codemux_detect_family "$UBUNTU")" "debian"

is "debian -> debian" \
   "$(codemux_detect_family "$(mock_os_release debian 'ID=debian' 'PRETTY_NAME="Debian GNU/Linux 12 (bookworm)"')")" \
   "debian"

is "pop!_os -> debian (via ID)" \
   "$(codemux_detect_family "$(mock_os_release pop 'ID=pop' 'ID_LIKE="ubuntu debian"')")" \
   "debian"

is "unknown derivative -> debian (via ID_LIKE)" \
   "$(codemux_detect_family "$(mock_os_release derivative 'ID=someremix' 'ID_LIKE="ubuntu debian"')")" \
   "debian"

is "raspbian -> debian" \
   "$(codemux_detect_family "$(mock_os_release raspbian 'ID=raspbian' 'ID_LIKE=debian')")" \
   "debian"

is "fedora -> fedora" \
   "$(codemux_detect_family "$(mock_os_release fedora 'ID=fedora' 'VERSION_ID=40')")" \
   "fedora"

is "rocky -> fedora (via ID_LIKE)" \
   "$(codemux_detect_family "$(mock_os_release rocky 'ID="rocky"' 'ID_LIKE="rhel centos fedora"')")" \
   "fedora"

is "amazon linux -> fedora" \
   "$(codemux_detect_family "$(mock_os_release amzn 'ID="amzn"' 'ID_LIKE="fedora"')")" \
   "fedora"

is "arch -> arch" \
   "$(codemux_detect_family "$(mock_os_release arch 'ID=arch' 'PRETTY_NAME="Arch Linux"')")" \
   "arch"

is "manjaro -> arch" \
   "$(codemux_detect_family "$(mock_os_release manjaro 'ID=manjaro' 'ID_LIKE=arch')")" \
   "arch"

is "cachyos -> arch (via ID_LIKE)" \
   "$(codemux_detect_family "$(mock_os_release cachy 'ID=cachyos' 'ID_LIKE=arch')")" \
   "arch"

is "opensuse -> suse" \
   "$(codemux_detect_family "$(mock_os_release suse 'ID="opensuse-tumbleweed"' 'ID_LIKE="opensuse suse"')")" \
   "suse"

is "alpine -> unknown" \
   "$(codemux_detect_family "$(mock_os_release alpine 'ID=alpine' 'PRETTY_NAME="Alpine Linux v3.20"')")" \
   "unknown"

is "void -> unknown" \
   "$(codemux_detect_family "$(mock_os_release void 'ID=void')")" \
   "unknown"

is "nixos -> unknown" \
   "$(codemux_detect_family "$(mock_os_release nixos 'ID=nixos')")" \
   "unknown"

is "no os-release -> unknown" \
   "$(codemux_detect_family "${TMPROOT}/definitely-missing")" \
   "unknown"

is "uppercase ID is normalised" \
   "$(codemux_detect_family "$(mock_os_release shouty 'ID=Fedora')")" \
   "fedora"

# ── install method selection ─────────────────────────────────────────────────

echo "install method selection"
is "debian -> deb"       "$(codemux_method_for_family debian)"  "deb"
is "fedora -> rpm"       "$(codemux_method_for_family fedora)"  "rpm"
is "suse -> rpm"         "$(codemux_method_for_family suse)"    "rpm"
is "arch -> arch"        "$(codemux_method_for_family arch)"    "arch"
is "unknown -> appimage" "$(codemux_method_for_family unknown)" "appimage"

# ── dependency lists ─────────────────────────────────────────────────────────

echo "dependency lists"
contains "debian deps name webkit2gtk 4.1" "$(codemux_dep_packages debian)" "libwebkit2gtk-4.1-0"
contains "debian deps name gtk3"           "$(codemux_dep_packages debian)" "libgtk-3-0"
contains "fedora deps name webkit2gtk 4.1" "$(codemux_dep_packages fedora)" "webkit2gtk4.1"
contains "arch deps name webkit2gtk-4.1"   "$(codemux_dep_packages arch)"   "webkit2gtk-4.1"
contains "suse deps name webkit2gtk 4.1"   "$(codemux_dep_packages suse)"   "libwebkit2gtk-4_1-0"
contains "unknown family still lists deps" "$(codemux_dep_packages unknown)" "gtk3"

# ── end-to-end --dry-run (no network, no writes) ─────────────────────────────
#
# CODEMUX_ARTIFACT short-circuits version resolution, so these runs never touch
# the network. The artifact only has to exist and be named plausibly.

echo "--dry-run plans"
FAKE_DEB="${TMPROOT}/codemux_9.9.9_amd64.deb"
: > "$FAKE_DEB"
FAKE_APPIMAGE="${TMPROOT}/codemux_9.9.9_amd64.AppImage"
: > "$FAKE_APPIMAGE"

# Package-manager stubs, so the printed plan depends on the mocked os-release
# rather than on whatever the host running the tests happens to have. Nothing
# is ever executed under --dry-run; these only need to exist for `command -v`.
STUB_BIN="${TMPROOT}/stub-bin"
mkdir -p "$STUB_BIN"
for stub in apt-get dpkg dpkg-query dnf yum zypper rpm pacman sudo; do
    printf '#!/bin/sh\nexit 1\n' > "${STUB_BIN}/${stub}"
    chmod +x "${STUB_BIN}/${stub}"
done

run_dry() { # <os-release path> <artifact>
    env -u CODEMUX_INSTALL_SH_NO_MAIN \
        NO_COLOR=1 \
        PATH="${STUB_BIN}:${PATH}" \
        CODEMUX_OS_RELEASE_PATH="$1" \
        CODEMUX_ARTIFACT="$2" \
        CODEMUX_INSTALL_DIR="${TMPROOT}/opt/bin" \
        bash "$INSTALL_SH" --dry-run 2>&1
}

if [ "$(uname -s)" != "Linux" ] || { [ "$(uname -m)" != "x86_64" ] && [ "$(uname -m)" != "amd64" ]; }; then
    echo "  skip (dry-run plans need Linux x86_64)"
else
    OUT=$(run_dry "$UBUNTU" "$FAKE_DEB")
    contains "ubuntu plan reports the debian family"   "$OUT" "debian family"
    contains "ubuntu plan installs the deb"            "$OUT" "apt-get install -y"
    contains "ubuntu plan is a dry run"                "$OUT" "dry run"
    not_contains "ubuntu plan does not extract"        "$OUT" "appimage-extract"

    FEDORA=$(mock_os_release fedora2 'ID=fedora' 'VERSION_ID=40' 'PRETTY_NAME="Fedora Linux 40"')
    FAKE_RPM="${TMPROOT}/codemux-9.9.9-1.x86_64.rpm"
    : > "$FAKE_RPM"
    OUT=$(run_dry "$FEDORA" "$FAKE_RPM")
    contains "fedora plan reports the fedora family"   "$OUT" "fedora family"
    contains "fedora plan installs the rpm with dnf"   "$OUT" "dnf install -y"
    not_contains "fedora plan does not extract"        "$OUT" "would extract"

    ARCHOS=$(mock_os_release arch2 'ID=arch')
    OUT=$(run_dry "$ARCHOS" "$FAKE_APPIMAGE")
    contains "arch plan points at the AUR package"     "$OUT" "codemux-bin"
    contains "arch plan falls back to the AppImage"    "$OUT" "would extract"

    ALPINE=$(mock_os_release alpine2 'ID=alpine')
    OUT=$(run_dry "$ALPINE" "$FAKE_APPIMAGE")
    contains "unknown family extracts the AppImage"    "$OUT" "would extract"
    contains "unknown family installs into the override dir" "$OUT" "${TMPROOT}/opt/bin/codemux"

    OUT=$(env -u CODEMUX_INSTALL_SH_NO_MAIN NO_COLOR=1 PATH="${STUB_BIN}:${PATH}" \
          CODEMUX_OS_RELEASE_PATH="$ALPINE" \
          CODEMUX_ARTIFACT="$FAKE_APPIMAGE" \
          CODEMUX_NO_DEPS=1 \
          CODEMUX_INSTALL_DIR="${TMPROOT}/opt/bin" \
          bash "$INSTALL_SH" --dry-run 2>&1)
    contains "CODEMUX_NO_DEPS=1 skips dependency install" "$OUT" "skipping dependency installation"

    OUT=$(env -u CODEMUX_INSTALL_SH_NO_MAIN NO_COLOR=1 PATH="${STUB_BIN}:${PATH}" \
          CODEMUX_OS_RELEASE_PATH="$UBUNTU" \
          CODEMUX_ARTIFACT="$FAKE_DEB" \
          CODEMUX_METHOD=appimage \
          CODEMUX_INSTALL_DIR="${TMPROOT}/opt/bin" \
          bash "$INSTALL_SH" --dry-run 2>&1)
    contains "CODEMUX_METHOD overrides the family choice" "$OUT" "forced"
    contains "forced appimage extracts"                   "$OUT" "would extract"

    OUT=$(env -u CODEMUX_INSTALL_SH_NO_MAIN NO_COLOR=1 PATH="${STUB_BIN}:${PATH}" \
          CODEMUX_OS_RELEASE_PATH="$UBUNTU" \
          CODEMUX_ARTIFACT="${TMPROOT}/nope.deb" \
          bash "$INSTALL_SH" --dry-run 2>&1)
    contains "missing CODEMUX_ARTIFACT errors out" "$OUT" "not found"

    OUT=$(env -u CODEMUX_INSTALL_SH_NO_MAIN NO_COLOR=1 \
          bash "$INSTALL_SH" --help 2>&1)
    contains "--help prints usage" "$OUT" "curl -fsSL https://get.codemux.org/install.sh"

    OUT=$(env -u CODEMUX_INSTALL_SH_NO_MAIN NO_COLOR=1 \
          bash "$INSTALL_SH" --bogus 2>&1)
    contains "unknown flag is rejected" "$OUT" "unknown option"

    # Nothing above may have written outside the temp dir.
    if [ -e "${TMPROOT}/opt/bin/codemux" ]; then
        printf '  FAIL dry run wrote to the install dir\n'
        FAIL=$((FAIL + 1))
    else
        printf '  ok   dry run wrote nothing\n'
        PASS=$((PASS + 1))
    fi
fi

# ── platform refusals (mocked uname) ─────────────────────────────────────────

echo "platform refusals"
mock_uname() { # <name> <kernel> <machine> -> stub bin dir
    local dir="${TMPROOT}/uname-${1}"
    mkdir -p "$dir"
    cat > "${dir}/uname" <<UNAME
#!/bin/sh
case "\$1" in
    -s) echo "$2" ;;
    -m) echo "$3" ;;
    *)  echo "$2" ;;
esac
UNAME
    chmod +x "${dir}/uname"
    printf '%s' "$dir"
}

run_with_uname() { # <stub dir>
    env -u CODEMUX_INSTALL_SH_NO_MAIN NO_COLOR=1 \
        PATH="${1}:${STUB_BIN}:${PATH}" \
        CODEMUX_OS_RELEASE_PATH="$UBUNTU" \
        CODEMUX_ARTIFACT="$FAKE_DEB" \
        CODEMUX_INSTALL_DIR="${TMPROOT}/opt/bin" \
        bash "$INSTALL_SH" --dry-run 2>&1
    printf 'exit=%s\n' "$?"
}

OUT=$(run_with_uname "$(mock_uname darwin Darwin arm64)")
contains "macOS is refused"                "$OUT" "macOS is not supported"
contains "macOS points at releases"        "$OUT" "/releases"
contains "macOS exits non-zero"            "$OUT" "exit=1"

OUT=$(run_with_uname "$(mock_uname arm Linux aarch64)")
contains "arm64 Linux is refused"          "$OUT" "arm64"
contains "arm64 explains x86_64-only CI"   "$OUT" "x86_64 only"
contains "arm64 exits non-zero"            "$OUT" "exit=1"

OUT=$(run_with_uname "$(mock_uname riscv Linux riscv64)")
contains "other arches are refused"        "$OUT" "unsupported architecture"

OUT=$(run_with_uname "$(mock_uname mingw MINGW64_NT-10.0 x86_64)")
contains "Windows is refused"              "$OUT" "Windows is not supported"
contains "Windows names the NSIS artifact" "$OUT" "x64-setup.exe"

OUT=$(run_with_uname "$(mock_uname linux Linux x86_64)")
contains "Linux x86_64 proceeds"           "$OUT" "Linux x86_64"
contains "Linux x86_64 exits clean"        "$OUT" "exit=0"

# Exercise the handover in a temporary prefix. Package-manager calls never
# leave the fixture, while the installer performs its real cleanup on disk.
echo "handing over from an AppImage to a native package"
BIN_DIR="$TMPROOT/handover/bin"
LIB_DIR="$TMPROOT/handover/lib/codemux"
DATA_DIR="$TMPROOT/handover/share"
PACKAGE_BIN="$TMPROOT/package/bin/codemux"
mkdir -p "$(dirname "$PACKAGE_BIN")"
launcher="$BIN_DIR/codemux"
desktop="$DATA_DIR/applications/codemux.desktop"

make_fallback() {
    mkdir -p "$BIN_DIR" "$LIB_DIR/usr/share/applications" "$DATA_DIR/applications"
    printf '%s\n' '#!/bin/sh' '# Generated by the Codemux installer — do not edit.' 'echo fallback' > "$launcher"
    chmod +x "$launcher"
    printf 'Exec=codemux %%U\n' > "$LIB_DIR/usr/share/applications/codemux.desktop"
    printf 'Exec=%s/codemux\n' "$BIN_DIR" > "$desktop"
}
make_fallback

# Set errexit inside the child, as it is at the real install.sh entry point.
run_package_handover() (
    set -e
    DRY_RUN=0
    USE_ROOT_WRITES=0
    have() { case "$1" in apt-get|dpkg-query|dnf) return 0 ;; *) return 1 ;; esac; }
    as_root() {
        # A manager can unpack and replace the executable before failing.
        touch "$TMPROOT/manager-ran"
        cat > "$PACKAGE_BIN" <<STUB
#!/bin/sh
case "\$1" in
    capabilities) echo '{"version":"9.9.9"}' ;;
    --help) echo 'native package' ;;
esac
exit $native_rc
STUB
        chmod +x "$PACKAGE_BIN"
        return "$manager_rc"
    }
    deb_is_configured() { return "$configured_rc"; }
    install_native_package "$method" "$TMPROOT/codemux.$method" > "$TMPROOT/handover.log" 2>&1
)
native_rc=0
for method in deb rpm; do
    manager_rc=1
    configured_rc=1
    run_package_handover
    is "$method failure is reported" "$?" "1"
    is "$method failure keeps the AppImage launcher" "$("$launcher" 2>/dev/null)" "fallback"
    is "$method failure keeps the payload and desktop entry" "$(test -d "$LIB_DIR" && test -f "$desktop" && echo yes)" "yes"
done
method=deb
manager_rc=0
run_package_handover
is "unconfigured deb is a failure" "$?" "1"
is "unconfigured deb keeps the launcher" "$("$launcher" 2>/dev/null)" "fallback"
configured_rc=0
for method in deb rpm; do
    native_rc=127
    run_package_handover
    is "$method reports a package that installed but will not run" "$?" "1"
    is "$method leaves the working fallback in place" "$("$launcher" 2>/dev/null)" "fallback"
    is "$method keeps the payload and desktop entry" "$(test -d "$LIB_DIR" && test -f "$desktop" && echo yes)" "yes"
    contains "$method checks the native executable" "$(cat "$TMPROOT/handover.log")" "$PACKAGE_BIN"
done

method=deb
native_rc=0
run_package_handover
is "runnable configured deb completes handover" "$?" "0"
contains "verified the packaged version" "$(cat "$TMPROOT/handover.log")" "codemux 9.9.9 at $PACKAGE_BIN"
is "successful handover retires the fallback files" "$(test ! -e "$launcher" && test ! -e "$LIB_DIR" && test ! -e "$desktop" && echo yes)" "yes"

make_fallback
new_bin="$TMPROOT/new-install-dir"
PATH="$BIN_DIR:${PACKAGE_BIN%/*}:$PATH" BIN_DIR="$new_bin" run_package_handover
is "handover with an AppImage in another prefix succeeds" "$?" "0"
contains "warns which AppImage still runs" "$(cat "$TMPROOT/handover.log")" "PATH still selects the AppImage launcher at $launcher"
contains "names the native command to use" "$(cat "$TMPROOT/handover.log")" "Use $PACKAGE_BIN"
is "keeps the other prefix's launcher" "$("$launcher" 2>/dev/null)" "fallback"
PATH="${PACKAGE_BIN%/*}:$BIN_DIR:$PATH" BIN_DIR="$new_bin" run_package_handover
not_contains "no shadowing warning when the package is first" "$(cat "$TMPROOT/handover.log")" "PATH still selects"

# A former CODEMUX_INSTALL_DIR may name the package's own prefix, even if
# this invocation chooses a different one. Block the manager before unpack.
PACKAGE_BIN="$launcher"
make_fallback
BIN_DIR="$new_bin"
for method in deb rpm; do
    manager_rc=1
    rm -f "$TMPROOT/manager-ran"
    run_package_handover
    is "$method refuses an overlapping AppImage" "$?" "1"
    is "$method refuses before the manager unpacks" "$(test ! -e "$TMPROOT/manager-ran" && echo yes)" "yes"
    is "$method keeps the overlapping launcher runnable" "$("$launcher" 2>/dev/null)" "fallback"
done

# An ordinary package upgrade at that same destination is still allowed.
BIN_DIR=$(dirname "$launcher")
printf 'from previous package\n' > "$launcher"
manager_rc=0
run_package_handover
is "overlapping package upgrade succeeds" "$?" "0"
is "overlapping install keeps the packaged binary" "$("$launcher" --help 2>/dev/null)" "native package"
is "overlapping install keeps package resources" "$(test -d "$LIB_DIR" && echo yes)" "yes"

# ── release signatures ───────────────────────────────────────────────────────
#
# Signatures are made here with a throwaway key in the release format (Tauri
# updater = base64 of a minisign signature file), so these run offline.

echo "release signatures"
# Selects $OPENSSL when a usable one exists; base64 works in any of them.
codemux_can_verify_signatures || true
REAL_KEY=$(sed -n 's/.*"pubkey"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "${SCRIPT_DIR}/../src-tauri/tauri.conf.json" \
    | "$OPENSSL" base64 -d -A 2>/dev/null | sed -n 2p)
is "embedded key is the updater key" "$CODEMUX_SIGNING_KEY" "$REAL_KEY"

if ! codemux_can_verify_signatures; then
    echo "  skip (needs OpenSSL 3)"
else
    SIGROOT="${TMPROOT}/sig"
    mkdir -p "$SIGROOT"
    "$OPENSSL" genpkey -algorithm ed25519 -out "${SIGROOT}/test.pem" 2>/dev/null
    TEST_KEY=$({ printf 'Ed\001\002\003\004\005\006\007\010'
                 "$OPENSSL" pkey -in "${SIGROOT}/test.pem" -pubout -outform DER | tail -c 32; } | "$OPENSSL" base64 -A)
    OTHER_KEY=$({ printf 'Ed\001\002\003\004\005\006\007\010'
                  "$OPENSSL" genpkey -algorithm ed25519 2>/dev/null | "$OPENSSL" pkey -pubout -outform DER | tail -c 32; } | "$OPENSSL" base64 -A)

    # Writes <file>.sig signed by the test key, naming <name> in the trusted comment.
    sign_fixture() { # <file> <name>
        local s="${SIGROOT}/work"
        mkdir -p "$s"
        "$OPENSSL" dgst -blake2b512 -binary "$1" > "${s}/hash"
        "$OPENSSL" pkeyutl -sign -inkey "${SIGROOT}/test.pem" -rawin -in "${s}/hash" -out "${s}/sig"
        local trusted
        trusted=$(printf 'timestamp:1700000000\tfile:%s' "$2")
        { cat "${s}/sig"; printf '%s' "$trusted"; } > "${s}/global.msg"
        "$OPENSSL" pkeyutl -sign -inkey "${SIGROOT}/test.pem" -rawin -in "${s}/global.msg" -out "${s}/global"
        {
            printf 'untrusted comment: signature from tauri secret key\n'
            { printf 'ED\001\002\003\004\005\006\007\010'; cat "${s}/sig"; } | "$OPENSSL" base64 -A
            printf '\ntrusted comment: %s\n' "$trusted"
            "$OPENSSL" base64 -A < "${s}/global"
            printf '\n'
        } | "$OPENSSL" base64 -A > "$1.sig"
    }

    verify_with() { # <key> <file> <sig> <name>
        local d
        d=$(mktemp -d "${SIGROOT}/v.XXXXXX")
        ( CODEMUX_SIGNING_KEY="$1"; codemux_verify_signature "$2" "$3" "$4" "$d" )
        printf '%s' "$?"
    }

    PKG="${SIGROOT}/codemux_9.9.9_amd64.deb"
    printf 'genuine package\n' > "$PKG"
    sign_fixture "$PKG" codemux_9.9.9_amd64.deb
    is "genuine package verifies"          "$(verify_with "$TEST_KEY" "$PKG" "$PKG.sig" codemux_9.9.9_amd64.deb)" "0"
    is "another key is refused"            "$(verify_with "$OTHER_KEY" "$PKG" "$PKG.sig" codemux_9.9.9_amd64.deb)" "1"
    is "the real release key refuses it"   "$(verify_with "$CODEMUX_SIGNING_KEY" "$PKG" "$PKG.sig" codemux_9.9.9_amd64.deb)" "1"
    is "a signature naming another asset is refused" \
       "$(verify_with "$TEST_KEY" "$PKG" "$PKG.sig" codemux_9.9.8_amd64.deb)" "1"
    TAMPERED="${SIGROOT}/tampered.deb"
    printf 'genuine packagf\n' > "$TAMPERED"
    is "tampered package is refused"       "$(verify_with "$TEST_KEY" "$TAMPERED" "$PKG.sig" codemux_9.9.9_amd64.deb)" "1"
    printf 'not a signature' > "${SIGROOT}/junk.sig"
    is "garbage signature is refused"      "$(verify_with "$TEST_KEY" "$PKG" "${SIGROOT}/junk.sig" codemux_9.9.9_amd64.deb)" "1"
    # Re-signing the trusted comment needs the key, so editing it breaks the check.
    "$OPENSSL" base64 -d -A < "$PKG.sig" | sed 's/file:codemux_9.9.9/file:codemux_9.9.8/' | "$OPENSSL" base64 -A > "${SIGROOT}/edited.sig"
    is "edited trusted comment is refused" \
       "$(verify_with "$TEST_KEY" "$PKG" "${SIGROOT}/edited.sig" codemux_9.9.8_amd64.deb)" "1"

    # The whole release path with downloads served from a fixture directory:
    # nothing may reach the package manager unless the signature holds.
    RELEASE_DIR="${SIGROOT}/release"
    CURL_BIN="${SIGROOT}/curl-bin"
    mkdir -p "$RELEASE_DIR" "$CURL_BIN"
    cat > "${CURL_BIN}/curl" <<'CURL'
#!/bin/sh
# Serve <url> from $FIXTURE_DIR by its last path segment.
out=""
while [ $# -gt 1 ]; do
    case "$1" in -o) out="$2"; shift ;; esac
    shift
done
name="${1##*/}"; name="${name%%[?#]*}"
src="${FIXTURE_DIR}/${name}"
[ -f "$src" ] || exit 22
if [ -n "$out" ]; then cp "$src" "$out"; else cat "$src"; fi
CURL
    chmod +x "${CURL_BIN}/curl"

    # -> exit status; marker file when an installer ran. Defaults to the deb
    # on Ubuntu; <method> forces rpm or appimage.
    run_release_install() ( # [os-release] [method]
        set -e
        rm -f "${SIGROOT}/installed"
        CODEMUX_SIGNING_KEY="$TEST_KEY"
        FIXTURE_DIR="$RELEASE_DIR"
        export FIXTURE_DIR
        PATH="${CURL_BIN}:${STUB_BIN}:${PATH}"
        CODEMUX_OS_RELEASE_PATH="${1:-$UBUNTU}"
        CODEMUX_INSTALL_VERSION=9.9.9
        CODEMUX_INSTALL_DIR="${TMPROOT}/opt/bin"
        # The installer's WORKDIR lands here, cleaned up with TMPROOT.
        TMPDIR="$SIGROOT"
        unset CODEMUX_ARTIFACT CODEMUX_METHOD
        [ -z "${2:-}" ] || CODEMUX_METHOD="$2"
        [ -z "${URL_ARTIFACT:-}" ] || CODEMUX_ARTIFACT="$URL_ARTIFACT"
        install_native_package() { cp "$2" "${SIGROOT}/installed"; }
        install_appimage() { cp "$1" "${SIGROOT}/installed"; }
        install_dependencies() { :; }
        verify_install() { :; }
        print_next_steps() { :; }
        main > "${SIGROOT}/release.log" 2>&1
    )

    cp "$PKG" "$PKG.sig" "$RELEASE_DIR/"
    run_release_install
    is "signed release installs" "$?" "0"
    is "the verified file is what gets installed" "$(cat "${SIGROOT}/installed" 2>/dev/null)" "genuine package"
    contains "reports the signature check" "$(cat "${SIGROOT}/release.log")" "signed with the Codemux release key"

    printf 'trojaned package\n' > "${RELEASE_DIR}/codemux_9.9.9_amd64.deb"
    run_release_install
    is "replaced release asset fails" "$?" "1"
    is "replaced release asset is never installed" "$(test ! -e "${SIGROOT}/installed" && echo yes)" "yes"
    contains "explains the mismatch" "$(cat "${SIGROOT}/release.log")" "does not match its release signature"

    cp "$PKG" "$RELEASE_DIR/"
    rm -f "${RELEASE_DIR}/codemux_9.9.9_amd64.deb.sig"
    run_release_install
    is "missing signature fails" "$?" "1"
    is "missing signature installs nothing" "$(test ! -e "${SIGROOT}/installed" && echo yes)" "yes"
    contains "explains the missing signature" "$(cat "${SIGROOT}/release.log")" "cannot be verified"

    # An explicit CODEMUX_ARTIFACT URL goes through the same check; only a
    # local file is installed unchecked.
    rm -f "$RELEASE_DIR"/*
    cp "$PKG" "$PKG.sig" "$RELEASE_DIR/"
    URL_ARTIFACT="https://downloads.example/codemux_9.9.9_amd64.deb" run_release_install
    is "signed URL artifact installs" "$?" "0"
    is "the verified URL artifact is what gets installed" "$(cat "${SIGROOT}/installed" 2>/dev/null)" "genuine package"
    printf 'trojaned package\n' > "${RELEASE_DIR}/codemux_9.9.9_amd64.deb"
    URL_ARTIFACT="https://downloads.example/codemux_9.9.9_amd64.deb" run_release_install
    is "replaced URL artifact fails" "$?" "1"
    is "replaced URL artifact is never installed" "$(test ! -e "${SIGROOT}/installed" && echo yes)" "yes"
    cp "$PKG" "$RELEASE_DIR/"
    rm -f "${RELEASE_DIR}/codemux_9.9.9_amd64.deb.sig"
    URL_ARTIFACT="https://downloads.example/codemux_9.9.9_amd64.deb" run_release_install
    is "URL artifact without a signature fails" "$?" "1"
    is "URL artifact without a signature installs nothing" "$(test ! -e "${SIGROOT}/installed" && echo yes)" "yes"
    contains "explains the missing URL signature" "$(cat "${SIGROOT}/release.log")" "cannot be verified"
    # The asset name and <path>.sig come from the URL path, not its query or
    # fragment, while the download keeps the URL as given.
    cp "$PKG" "$PKG.sig" "$RELEASE_DIR/"
    URL_ARTIFACT="https://downloads.example/codemux_9.9.9_amd64.deb?download=1#latest" run_release_install
    is "a URL with query and fragment verifies by its path" "$?" "0"
    is "the query URL installs the verified file" "$(cat "${SIGROOT}/installed" 2>/dev/null)" "genuine package"
    not_contains "a URL artifact is not called unchecked" "$(cat "${SIGROOT}/release.log")" "not checked"

    # The same gate on the rpm (zypper --allow-unsigned-rpm) and AppImage paths.
    SUSE_LEAP=$(mock_os_release leap 'ID="opensuse-leap"' 'ID_LIKE="suse opensuse"' 'VERSION_ID="15.6"')
    for kind in rpm appimage; do
        if [ "$kind" = "rpm" ]; then os="$SUSE_LEAP"; else os="$UBUNTU"; fi
        name=$(codemux_artifact_name "$kind" 9.9.9)
        rm -f "$RELEASE_DIR"/*
        printf 'genuine %s\n' "$kind" > "${RELEASE_DIR}/${name}"
        sign_fixture "${RELEASE_DIR}/${name}" "$name"
        run_release_install "$os" "$kind"
        is "signed ${kind} installs" "$?" "0"
        is "the verified ${kind} is what gets installed" "$(cat "${SIGROOT}/installed" 2>/dev/null)" "genuine ${kind}"

        printf 'trojaned %s\n' "$kind" > "${RELEASE_DIR}/${name}"
        run_release_install "$os" "$kind"
        is "replaced ${kind} fails" "$?" "1"
        is "replaced ${kind} is never installed" "$(test ! -e "${SIGROOT}/installed" && echo yes)" "yes"

        rm -f "${RELEASE_DIR}/${name}.sig"
        run_release_install "$os" "$kind"
        is "${kind} without a signature fails" "$?" "1"
        is "${kind} without a signature installs nothing" "$(test ! -e "${SIGROOT}/installed" && echo yes)" "yes"
    done

    # A signature for one kind does not pass for another.
    deb_name=$(codemux_artifact_name deb 9.9.9)
    rpm_name=$(codemux_artifact_name rpm 9.9.9)
    rm -f "$RELEASE_DIR"/*
    printf 'genuine deb\n' > "${RELEASE_DIR}/${rpm_name}"
    sign_fixture "${RELEASE_DIR}/${rpm_name}" "$deb_name"
    run_release_install "$SUSE_LEAP" rpm
    is "a deb signature does not pass an rpm" "$?" "1"
    is "the mislabelled rpm is never installed" "$(test ! -e "${SIGROOT}/installed" && echo yes)" "yes"

    # wget instead of curl downloads and checks the same way.
    WGET_BIN="${SIGROOT}/wget-bin"
    mkdir -p "$WGET_BIN"
    cat > "${WGET_BIN}/wget" <<'WGET'
#!/bin/sh
# Serve <url> from $FIXTURE_DIR by its last path segment; -O - or -qO- is stdout.
out="-"
while [ $# -gt 1 ]; do
    case "$1" in -O) out="$2"; shift ;; -qO-) out="-" ;; esac
    shift
done
src="${FIXTURE_DIR}/${1##*/}"
[ -f "$src" ] || exit 8
if [ "$out" = "-" ]; then cat "$src"; else cp "$src" "$out"; fi
WGET
    chmod +x "${WGET_BIN}/wget"
    # The real curl stays on PATH, so hide it for these runs.
    have() { case "$1" in curl) return 1 ;; esac; command -v "$1" >/dev/null 2>&1; }
    CURL_BIN="$WGET_BIN"
    rm -f "$RELEASE_DIR"/*
    cp "$PKG" "$PKG.sig" "$RELEASE_DIR/"
    run_release_install
    is "signed release installs via wget" "$?" "0"
    is "wget path installs the verified file" "$(cat "${SIGROOT}/installed" 2>/dev/null)" "genuine package"
    printf 'trojaned package\n' > "${RELEASE_DIR}/codemux_9.9.9_amd64.deb"
    run_release_install
    is "replaced asset via wget fails" "$?" "1"
    is "replaced asset via wget is never installed" "$(test ! -e "${SIGROOT}/installed" && echo yes)" "yes"
    have() { command -v "$1" >/dev/null 2>&1; }
fi

# ── which OpenSSL does the check ─────────────────────────────────────────────

echo "openssl selection"
OSSL="${TMPROOT}/openssl-fakes"
mkdir -p "${OSSL}/old" "${OSSL}/fips" "${OSSL}/none" "${OSSL}/leap"
# OpenSSL 1.1.1: pkeyutl has no -rawin.
printf '#!/bin/sh\ncase "$1" in version) echo "OpenSSL 1.1.1w  11 Sep 2023" ;; pkeyutl) echo "Usage: pkeyutl [options] -in file" >&2 ;; esac\nexit 0\n' > "${OSSL}/old/openssl"
# OpenSSL 3 in FIPS-only mode: -rawin exists, BLAKE2b is refused.
printf '#!/bin/sh\ncase "$1" in pkeyutl) echo " -rawin  raw input" >&2; exit 0 ;; esac\nexit 1\n' > "${OSSL}/fips/openssl"
chmod +x "${OSSL}/old/openssl" "${OSSL}/fips/openssl"
cp "${OSSL}/old/openssl" "${OSSL}/leap/openssl"
# The binary the installer's own probe picks (openssl-3 beside an old
# openssl, for example), not whatever `openssl` comes first on PATH.
REAL_OPENSSL=""
if codemux_can_verify_signatures; then REAL_OPENSSL=$(command -v "$OPENSSL" || true); fi
[ -z "$REAL_OPENSSL" ] || ln -s "$REAL_OPENSSL" "${OSSL}/leap/openssl-3"

probe() { # <dir on PATH> [CODEMUX_OPENSSL] -> "<status> <OPENSSL> <problem>"
    # Unset first so a probe that reads state it never set fails under set -u.
    ( unset OPENSSL_PROBLEM OPENSSL_PROBLEM_BIN
      PATH="$1"; CODEMUX_OPENSSL="${2:-}"; OPENSSL=openssl
      codemux_can_verify_signatures; printf '%s %s %s' "$?" "${OPENSSL##*/}" "$OPENSSL_PROBLEM" )
}
is "no openssl is missing"            "$(probe "${OSSL}/none")" "1 openssl missing"
is "openssl 1.1.1 is old"             "$(probe "${OSSL}/old")"  "1 openssl old"
is "FIPS-only openssl lacks algorithms" "$(probe "${OSSL}/fips")" "1 openssl algorithms"
is "CODEMUX_OPENSSL is the only one tried" "$(probe "${OSSL}/none" "${OSSL}/old/openssl")" "1 openssl old"
if [ -n "$REAL_OPENSSL" ] && codemux_can_verify_signatures; then
    is "Leap's openssl-3 is used beside an old openssl" "$(probe "${OSSL}/leap")" "0 openssl-3 "
    is "CODEMUX_OPENSSL picks a binary off PATH" "$(probe "${OSSL}/none" "$REAL_OPENSSL")" "0 ${REAL_OPENSSL##*/} "
fi

tools_out() { # <family> <PATH dir> [priv] [dry run] -> output of require_signature_tools
    ( DRY_RUN="${4:-1}" PRIV="${3:-sudo}" FAMILY="$1"
      PATH="${2}:${STUB_BIN}"
      fetch_text() { echo "network used" >&2; }
      require_signature_tools; echo "status=$?" ) 2>&1 || echo "status=$?"
}
OUT=$(tools_out suse "${OSSL}/old")
contains "Leap dry run plans openssl-3" "$OUT" "would run: sudo zypper --non-interactive install openssl-3"
contains "Leap dry run carries on" "$OUT" "status=0"
OUT=$(tools_out debian "${OSSL}/none")
contains "dry run shows the apt-get update" "$OUT" "would run: sudo env DEBIAN_FRONTEND=noninteractive apt-get update -qq"
contains "dry run shows the openssl install" "$OUT" "would run: sudo env DEBIAN_FRONTEND=noninteractive apt-get install -y openssl"
OUT=$(tools_out debian "${OSSL}/old")
not_contains "an old openssl on Debian is not reinstalled" "$OUT" "would run"
contains "an old openssl is named as old" "$OUT" "needs OpenSSL 3 or newer"
contains "the old version is shown" "$OUT" "OpenSSL 1.1.1w"
not_contains "an old openssl is not called missing" "$OUT" "was not found"
contains "dry run says a real run stops" "$OUT" "a real run would stop here"
OUT=$(tools_out fedora "${OSSL}/fips" sudo 0)
contains "FIPS-only openssl is explained" "$OUT" "refuses BLAKE2b-512 or Ed25519"
contains "the workaround is named" "$OUT" "CODEMUX_ARTIFACT=/path/to/package"
contains "a real run stops" "$OUT" "status=1"

OUT=$( (
    DRY_RUN=0 PRIV=none
    codemux_can_verify_signatures() { OPENSSL_PROBLEM=missing; return 1; }
    fetch_text() { echo "network used" >&2; }
    require_signature_tools
) 2>&1)
contains "no OpenSSL 3 stops before downloading" "$OUT" "needs the OpenSSL 3 command-line tool"
not_contains "no OpenSSL 3 downloads nothing" "$OUT" "network used"

OUT=$(env -u CODEMUX_INSTALL_SH_NO_MAIN NO_COLOR=1 PATH="${STUB_BIN}:${PATH}" \
      CODEMUX_OS_RELEASE_PATH="$UBUNTU" CODEMUX_INSTALL_VERSION=9.9.9 \
      CODEMUX_INSTALL_DIR="${TMPROOT}/opt/bin" \
      bash "$INSTALL_SH" --dry-run 2>&1)
contains "dry run plans the signature check" "$OUT" "would verify: codemux_9.9.9_amd64.deb against https://github.com/Zeus-Deus/codemux/releases/download/v9.9.9/codemux_9.9.9_amd64.deb.sig"
# A fresh installer process on a host with no openssl at all (minimal
# container images): every tool on PATH except openssl and sudo.
NOSSL_BIN="${TMPROOT}/no-openssl-bin"
mkdir -p "$NOSSL_BIN"
_old_ifs="$IFS"; IFS=:
for _dir in $PATH; do
    [ -d "$_dir" ] || continue
    for _tool in "$_dir"/*; do
        _name="${_tool##*/}"
        case "$_name" in openssl|openssl-3|sudo) continue ;; esac
        [ -x "$_tool" ] && [ ! -e "${NOSSL_BIN}/${_name}" ] && ln -s "$_tool" "${NOSSL_BIN}/${_name}"
    done
done
IFS="$_old_ifs"
no_openssl_dry() { # <PATH> -> output of a pinned --dry-run on Ubuntu
    env -i HOME="${TMPROOT}/home" NO_COLOR=1 PATH="$1" \
        CODEMUX_OS_RELEASE_PATH="$UBUNTU" CODEMUX_INSTALL_VERSION=9.9.9 \
        CODEMUX_INSTALL_DIR="${TMPROOT}/opt/bin" \
        bash "$INSTALL_SH" --dry-run 2>&1
}
if [ "$(id -u)" != "0" ] && ! PATH="$NOSSL_BIN" command -v openssl >/dev/null 2>&1; then
    OUT=$(no_openssl_dry "${STUB_BIN}:${NOSSL_BIN}")
    not_contains "no openssl with sudo: no unbound variable" "$OUT" "unbound variable"
    contains "no openssl with sudo plans the openssl install" "$OUT" "would run: sudo env DEBIAN_FRONTEND=noninteractive apt-get install -y openssl"
    contains "no openssl with sudo still plans the check" "$OUT" "would verify: codemux_9.9.9_amd64.deb"
    OUT=$(no_openssl_dry "$NOSSL_BIN")
    not_contains "no openssl without root: no unbound variable" "$OUT" "unbound variable"
    contains "no openssl without root explains what is missing" "$OUT" "which was not found"
    contains "no openssl without root names the workaround" "$OUT" "CODEMUX_ARTIFACT=/path/to/package"
else
    echo "  skip (no-openssl runs need a non-root user)"
fi

OUT=$(run_dry "$UBUNTU" "$FAKE_DEB")
contains "a local CODEMUX_ARTIFACT warns it is unverified" "$OUT" "signature is not checked"

# ── syntax ───────────────────────────────────────────────────────────────────

echo "syntax"
if bash -n "$INSTALL_SH" 2>/dev/null; then
    printf '  ok   install.sh parses under bash\n'
    PASS=$((PASS + 1))
else
    printf '  FAIL install.sh does not parse under bash\n'
    FAIL=$((FAIL + 1))
fi

# The advertised entry point is `curl ... | sh`, so the script has to parse
# under a POSIX shell too. Only checked when a non-bash /bin/sh is available.
for posix_sh in dash ash busybox; do
    if command -v "$posix_sh" >/dev/null 2>&1; then
        if [ "$posix_sh" = "busybox" ]; then
            check_cmd="busybox sh -n"
        else
            check_cmd="$posix_sh -n"
        fi
        if $check_cmd "$INSTALL_SH" 2>/dev/null; then
            printf '  ok   install.sh parses under %s\n' "$posix_sh"
            PASS=$((PASS + 1))
        else
            printf '  FAIL install.sh does not parse under %s\n' "$posix_sh"
            FAIL=$((FAIL + 1))
        fi
        break
    fi
done

if command -v shellcheck >/dev/null 2>&1; then
    if shellcheck "$INSTALL_SH"; then
        printf '  ok   shellcheck clean\n'
        PASS=$((PASS + 1))
    else
        printf '  FAIL shellcheck reported problems\n'
        FAIL=$((FAIL + 1))
    fi
    # `curl ... | sh` runs the script under /bin/sh, so it must also be free of
    # bashisms. POSIX-mode shellcheck catches those even where no dash exists.
    if shellcheck -s sh "$INSTALL_SH"; then
        printf '  ok   shellcheck clean in POSIX sh mode\n'
        PASS=$((PASS + 1))
    else
        printf '  FAIL install.sh uses bash-only syntax\n'
        FAIL=$((FAIL + 1))
    fi
else
    printf '  skip shellcheck not installed\n'
fi

# ── summary ──────────────────────────────────────────────────────────────────

echo
printf '%d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ] || exit 1
