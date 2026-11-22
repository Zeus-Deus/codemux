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
