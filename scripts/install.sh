#!/usr/bin/env bash
#
# Codemux one-line installer.
#
#   curl -fsSL https://get.codemux.org/install.sh | sh
#
# Installs the `codemux` binary on Linux (x86_64) and prints next steps.
# Uses the native package for the detected distro family (.deb / .rpm) so the
# system package manager resolves the WebKitGTK/GTK runtime dependencies, and
# falls back to extracting the AppImage payload (which ships its own copies of
# those libraries) everywhere else — including when the caller has no root.
#
# No telemetry. Nothing is executed that is not printed first under --dry-run.
#
# Environment overrides
#   CODEMUX_INSTALL_VERSION    version or tag to install (default: latest release)
#   CODEMUX_VERSION            same, but ignored inside a Codemux terminal pane,
#                              where the app already exports it (see
#                              codemux_pinned_version below)
#   CODEMUX_ARTIFACT           path or URL to a .deb / .rpm / .AppImage; skips
#                              version resolution and download-by-name entirely
#   CODEMUX_INSTALL_DIR        bin directory for the AppImage fallback
#                              (default: /usr/local/bin, or ~/.local/bin non-root)
#   CODEMUX_NO_DEPS=1          never touch the package manager for dependencies
#   CODEMUX_METHOD             force deb | rpm | appimage
#   CODEMUX_OS_RELEASE_PATH    alternate os-release file (testing)
#   CODEMUX_REPO_SLUG          alternate GitHub owner/repo (testing)
#   NO_COLOR=1                 disable ANSI colour
#
# Flags
#   --dry-run                  print the plan, change nothing
#   --version <v>              same as CODEMUX_VERSION
#   --help
#
# shellcheck shell=bash

set -eu
# `set -o pipefail` is not POSIX. Enable it where the running shell supports it
# (bash/ksh/zsh) instead of hard-failing under a `sh` that does not — the
# advertised entry point is `curl ... | sh`, and /bin/sh is dash on Debian.
# shellcheck disable=SC3040
if (set -o pipefail 2>/dev/null); then set -o pipefail; fi

# ── Constants ────────────────────────────────────────────────────────────────

CODEMUX_REPO_SLUG="${CODEMUX_REPO_SLUG:-Zeus-Deus/codemux}"
CODEMUX_API_BASE="https://api.github.com/repos/${CODEMUX_REPO_SLUG}"
CODEMUX_DOWNLOAD_BASE="https://github.com/${CODEMUX_REPO_SLUG}/releases/download"
CODEMUX_RELEASES_URL="https://github.com/${CODEMUX_REPO_SLUG}/releases"

# ── Output helpers ───────────────────────────────────────────────────────────

if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
    C_GREEN='\033[0;32m'
    C_RED='\033[0;31m'
    C_YELLOW='\033[0;33m'
    C_BLUE='\033[0;34m'
    C_BOLD='\033[1m'
    C_DIM='\033[2m'
    C_RESET='\033[0m'
else
    C_GREEN='' C_RED='' C_YELLOW='' C_BLUE='' C_BOLD='' C_DIM='' C_RESET=''
fi

say()  { printf '%b\n' "$*"; }
step() { printf '%b\n' "${C_BOLD}$*${C_RESET}"; }
ok()   { printf '%b\n' "  ${C_GREEN}✓${C_RESET} $*"; }
bad()  { printf '%b\n' "  ${C_RED}✗${C_RESET} $*" >&2; }
warn() { printf '%b\n' "  ${C_YELLOW}!${C_RESET} $*" >&2; }
note() { printf '%b\n' "  ${C_DIM}$*${C_RESET}"; }
die()  { bad "$*"; exit 1; }

# ── Pure helpers (unit-tested by scripts/install-sh.test.sh) ─────────────────

# 0.15.5 / v0.15.5 -> 0.15.5
codemux_normalize_version() {
    printf '%s' "${1#v}"
}

# 0.15.5 / v0.15.5 -> v0.15.5
codemux_tag_for_version() {
    printf 'v%s' "${1#v}"
}

# Artifact file names, straight out of .github/workflows/release.yml's
# tauri-action bundle output for ubuntu-22.04 / x86_64:
#   codemux_<version>_amd64.deb
#   codemux-<version>-1.x86_64.rpm
#   codemux_<version>_amd64.AppImage
codemux_artifact_name() {
    _kind="$1"
    _version="${2#v}"
    case "$_kind" in
        deb)      printf 'codemux_%s_amd64.deb' "$_version" ;;
        rpm)      printf 'codemux-%s-1.x86_64.rpm' "$_version" ;;
        appimage) printf 'codemux_%s_amd64.AppImage' "$_version" ;;
        *)        return 1 ;;
    esac
}

codemux_artifact_url() {
    _name=$(codemux_artifact_name "$1" "$2") || return 1
    printf '%s/%s/%s' "$CODEMUX_DOWNLOAD_BASE" "$(codemux_tag_for_version "$2")" "$_name"
}

# The version the caller pinned, or empty for "latest".
#
# CODEMUX_INSTALL_VERSION wins. Plain CODEMUX_VERSION is honoured too, but a
# running Codemux exports CODEMUX=1 plus CODEMUX_VERSION=<its own version> into
# every terminal pane it opens (src-tauri/src/terminal/mod.rs), so inside such a
# pane that variable describes the *installed* build, not a request to pin. It
# is ignored there — otherwise upgrading from a Codemux terminal would silently
# reinstall the version you already have.
codemux_pinned_version() {
    if [ -n "${CODEMUX_INSTALL_VERSION:-}" ]; then
        printf '%s' "${CODEMUX_INSTALL_VERSION#v}"
        return 0
    fi
    if [ -n "${CODEMUX_VERSION:-}" ] && [ -z "${CODEMUX:-}" ]; then
        printf '%s' "${CODEMUX_VERSION#v}"
        return 0
    fi
    printf ''
}

# deb | rpm | appimage | unknown, from a file name or URL.
codemux_kind_from_filename() {
    case "$1" in
        *.deb)                    printf 'deb' ;;
        *.rpm)                    printf 'rpm' ;;
        *.AppImage|*.appimage)    printf 'appimage' ;;
        *)                        printf 'unknown' ;;
    esac
}

# Read one KEY=value line out of an os-release file, unquoted and lowercased.
codemux_os_release_field() {
    _path="$1"
    _key="$2"
    [ -r "$_path" ] || return 0
    _value=$(sed -n "s/^${_key}=//p" "$_path" 2>/dev/null | head -n 1)
    _value=${_value%\"}; _value=${_value#\"}
    _value=${_value%\'}; _value=${_value#\'}
    printf '%s' "$_value" | tr '[:upper:]' '[:lower:]'
}

# debian | fedora | suse | arch | unknown
codemux_detect_family() {
    _os_release="${1:-/etc/os-release}"
    [ -r "$_os_release" ] || { printf 'unknown'; return 0; }
    _id=$(codemux_os_release_field "$_os_release" ID)
    _id_like=$(codemux_os_release_field "$_os_release" ID_LIKE)
    # ID first, then each whitespace-separated ID_LIKE token, so a derivative
    # that names itself (`ID=pop`) and one that only inherits
    # (`ID_LIKE="ubuntu debian"`) both resolve.
    for _token in $_id $_id_like; do
        case "$_token" in
            debian|ubuntu|pop|linuxmint|mint|elementary|zorin|raspbian|kali|neon|devuan|deepin|pureos|tuxedo)
                printf 'debian'; return 0 ;;
            fedora|rhel|centos|rocky|almalinux|ol|oracle|nobara|amzn|scientific|"centos stream")
                printf 'fedora'; return 0 ;;
            arch|archarm|manjaro|endeavouros|garuda|cachyos|artix|arcolinux)
                printf 'arch'; return 0 ;;
            suse|opensuse|opensuse-leap|opensuse-tumbleweed|sles|sled)
                printf 'suse'; return 0 ;;
        esac
    done
    printf 'unknown'
}

# deb | rpm | arch | appimage
codemux_method_for_family() {
    case "$1" in
        debian) printf 'deb' ;;
        fedora) printf 'rpm' ;;
        suse)   printf 'rpm' ;;
        arch)   printf 'arch' ;;
        *)      printf 'appimage' ;;
    esac
}

# Runtime libraries the binary links against (WebKitGTK/GTK3 are needed even
# for headless `codemux serve`, because they are linked into the executable
# itself, not dlopen'd on demand). Only used by the AppImage fallback — the
# .deb and .rpm carry these as declared dependencies.
codemux_dep_packages() {
    case "$1" in
        debian) printf 'libwebkit2gtk-4.1-0 libgtk-3-0 libglib2.0-0 libssl3' ;;
        fedora) printf 'webkit2gtk4.1 gtk3 glib2 openssl-libs' ;;
        suse)   printf 'libwebkit2gtk-4_1-0 gtk3 glib2 libopenssl3' ;;
        arch)   printf 'webkit2gtk-4.1 gtk3 glib2 openssl' ;;
        *)      printf 'webkit2gtk-4.1 gtk3 glib2 openssl' ;;
    esac
}

# Optional tools that unlock features but are never required to start.
codemux_optional_packages() {
    case "$1" in
        debian) printf 'git ripgrep fd-find gh xdg-utils' ;;
        fedora) printf 'git ripgrep fd-find gh xdg-utils' ;;
        suse)   printf 'git ripgrep fd gh xdg-utils' ;;
        arch)   printf 'git ripgrep fd github-cli xdg-utils' ;;
        *)      printf 'git ripgrep fd gh xdg-utils' ;;
    esac
}

# ── Runtime state ────────────────────────────────────────────────────────────

DRY_RUN=0
WORKDIR=""
PRIV="none"          # root | sudo | none
FAMILY="unknown"
METHOD=""
BIN_DIR=""
LIB_DIR=""
DATA_DIR=""
USE_ROOT_WRITES=0

cleanup() {
    [ -n "$WORKDIR" ] && [ -d "$WORKDIR" ] && rm -rf "$WORKDIR"
    return 0
}
trap cleanup EXIT

# Print-then-run. Under --dry-run nothing is executed.
run() {
    if [ "$DRY_RUN" = "1" ]; then
        printf '%b\n' "  ${C_DIM}would run:${C_RESET} $*"
        return 0
    fi
    "$@"
}

as_root() {
    case "$PRIV" in
        root) run "$@" ;;
        sudo) run sudo "$@" ;;
        *)    die "this step needs root: $*" ;;
    esac
}

have() { command -v "$1" >/dev/null 2>&1; }

# True when the current user could create or replace <path>: walk up to the
# nearest existing ancestor and test it for write permission.
writable_target() {
    _wt_path="$1"
    while [ -n "$_wt_path" ] && [ ! -e "$_wt_path" ]; do
        _wt_parent=$(dirname "$_wt_path")
        [ "$_wt_parent" = "$_wt_path" ] && break
        _wt_path="$_wt_parent"
    done
    [ -w "$_wt_path" ]
}

# True when writing every given path needs elevation. Having sudo is not a
# reason to use it — a home-directory install must stay owned by the user.
needs_root_for() {
    [ "$(id -u)" = "0" ] && return 1
    for _nr_target in "$@"; do
        writable_target "$_nr_target" || return 0
    done
    return 1
}

# Write through sudo only where it is actually required.
maybe_root() {
    if [ "$USE_ROOT_WRITES" = "1" ]; then
        as_root "$@"
    else
        run "$@"
    fi
}

# Kept as a heredoc rather than read back out of "$0": when the script arrives
# on stdin (`curl ... | sh`) there is no file to read.
usage() {
    cat <<USAGE
Codemux installer

  curl -fsSL https://get.codemux.org/install.sh | sh

Flags
  --dry-run              print the plan, change nothing
  --version <v>          install a specific version (default: latest release)
  -h, --help             this message

Environment
  CODEMUX_INSTALL_VERSION  version or tag to install (CODEMUX_VERSION also
                         works outside a Codemux terminal)
  CODEMUX_ARTIFACT       path or URL to a .deb / .rpm / .AppImage to install
  CODEMUX_INSTALL_DIR    bin directory for the AppImage fallback
  CODEMUX_NO_DEPS=1      never touch the package manager for dependencies
  CODEMUX_METHOD         force deb | rpm | appimage
  NO_COLOR=1             disable colour

Releases: ${CODEMUX_RELEASES_URL}
USAGE
}

# ── Preflight ────────────────────────────────────────────────────────────────

check_platform() {
    _os=$(uname -s 2>/dev/null || printf 'unknown')
    case "$_os" in
        Linux) : ;;
        Darwin)
            bad "macOS is not supported by this installer yet."
            note "Codemux publishes Linux and Windows builds today."
            note "Watch ${CODEMUX_RELEASES_URL} for a macOS build."
            exit 1
            ;;
        MINGW*|MSYS*|CYGWIN*|Windows_NT)
            bad "Windows is not supported by this installer."
            note "Download the Windows installer (codemux_<version>_x64-setup.exe) from:"
            note "  ${CODEMUX_RELEASES_URL}/latest"
            exit 1
            ;;
        *)
            die "unsupported operating system: ${_os}. See ${CODEMUX_RELEASES_URL}"
            ;;
    esac

    _arch=$(uname -m 2>/dev/null || printf 'unknown')
    case "$_arch" in
        x86_64|amd64) ok "Linux x86_64" ;;
        aarch64|arm64)
            bad "No arm64 build is published yet — release builds are x86_64 only."
            note "Follow ${CODEMUX_RELEASES_URL} for arm64 availability, or build from"
            note "source: https://github.com/${CODEMUX_REPO_SLUG}#building"
            exit 1
            ;;
        *)
            die "unsupported architecture: ${_arch} (x86_64 only)"
            ;;
    esac
}

detect_privileges() {
    if [ "$(id -u)" = "0" ]; then
        PRIV="root"
        ok "running as root"
    elif have sudo && sudo -n true 2>/dev/null; then
        PRIV="sudo"
        ok "sudo available (passwordless)"
    elif have sudo; then
        PRIV="sudo"
        ok "sudo available (may prompt for your password)"
    else
        PRIV="none"
        warn "no root and no sudo — installing into your home directory"
    fi
}

resolve_dirs() {
    if [ -n "${CODEMUX_INSTALL_DIR:-}" ]; then
        BIN_DIR="$CODEMUX_INSTALL_DIR"
        case "$BIN_DIR" in
            */bin) LIB_DIR="${BIN_DIR%/bin}/lib/codemux"; DATA_DIR="${BIN_DIR%/bin}/share" ;;
            *)     LIB_DIR="${BIN_DIR}/codemux-payload"; DATA_DIR="${BIN_DIR}/share" ;;
        esac
    elif [ "$PRIV" = "none" ]; then
        BIN_DIR="${HOME}/.local/bin"
        LIB_DIR="${HOME}/.local/lib/codemux"
        DATA_DIR="${XDG_DATA_HOME:-${HOME}/.local/share}"
    else
        BIN_DIR="/usr/local/bin"
        LIB_DIR="/usr/local/lib/codemux"
        DATA_DIR="/usr/local/share"
    fi

    if needs_root_for "$BIN_DIR" "$LIB_DIR"; then
        USE_ROOT_WRITES=1
    else
        USE_ROOT_WRITES=0
    fi
}

# ── Existing install ─────────────────────────────────────────────────────────

report_existing_install() {
    _existing_path=$(command -v codemux 2>/dev/null || true)

    # Only consult pacman on an Arch-family system — a stray pacman on another
    # distro should not hijack the decision.
    if [ "$FAMILY" = "arch" ] && have pacman && pacman -Qq codemux-bin >/dev/null 2>&1; then
        _aur_version=$(pacman -Q codemux-bin 2>/dev/null | awk '{print $2}')
        say ""
        ok "codemux ${_aur_version} is already installed from the AUR (codemux-bin)"
        note "Upgrade it with your AUR helper so the two installs never disagree:"
        note "  yay -S codemux-bin      # or: paru -S codemux-bin"
        if [ "$DRY_RUN" != "1" ]; then
            say ""
            say "Nothing to do."
            exit 0
        fi
        say ""
    fi

    if have dpkg-query && dpkg-query -W -f='${Status}' codemux 2>/dev/null | grep -q 'install ok installed'; then
        _deb_version=$(dpkg-query -W -f='${Version}' codemux 2>/dev/null || true)
        ok "found existing .deb install (codemux ${_deb_version}) — it will be upgraded in place"
        return 0
    fi

    if have rpm && rpm -q codemux >/dev/null 2>&1; then
        ok "found existing .rpm install ($(rpm -q codemux 2>/dev/null | head -n 1)) — it will be upgraded in place"
        return 0
    fi

    if [ -e "${BIN_DIR}/codemux" ]; then
        ok "found existing install at ${BIN_DIR}/codemux — it will be replaced"
        return 0
    fi

    if [ -n "$_existing_path" ]; then
        ok "found existing codemux on PATH at ${_existing_path}"
        return 0
    fi

    note "no existing install detected — this is a fresh install"
}

# A stale AppImage-extract install in /usr/local/bin shadows the package
# manager's /usr/bin/codemux (because /usr/local/bin sorts earlier on PATH).
# Remove it whenever we hand over to a native package.
remove_fallback_install() {
    if [ -e "${BIN_DIR}/codemux" ] || [ -d "$LIB_DIR" ]; then
        warn "removing the previous AppImage-extract install at ${BIN_DIR}/codemux"
        warn "so it cannot shadow the packaged binary"
        maybe_root rm -rf "${BIN_DIR}/codemux" "$LIB_DIR"
    fi
}

# ── Download ─────────────────────────────────────────────────────────────────

fetch_text() {
    if have curl; then
        curl -fsSL "$1"
    elif have wget; then
        wget -qO- "$1"
    else
        die "need curl or wget to talk to GitHub"
    fi
}

download_to() {
    _url="$1"
    _dest="$2"
    if [ "$DRY_RUN" = "1" ]; then
        printf '%b\n' "  ${C_DIM}would download:${C_RESET} ${_url}"
        return 0
    fi
    if have curl; then
        if [ -t 1 ]; then
            curl -fL --progress-bar -o "$_dest" "$_url"
        else
            curl -fsSL -o "$_dest" "$_url"
        fi
    elif have wget; then
        wget -q --show-progress -O "$_dest" "$_url"
    else
        die "need curl or wget to download the release artifact"
    fi
}

resolve_version() {
    _pinned=$(codemux_pinned_version)
    if [ -n "$_pinned" ]; then
        printf '%s' "$_pinned"
        return 0
    fi
    _json=$(fetch_text "${CODEMUX_API_BASE}/releases/latest" 2>/dev/null || true)
    _tag=$(printf '%s' "$_json" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)
    if [ -z "$_tag" ]; then
        if [ "$DRY_RUN" = "1" ]; then
            printf '%s' "<latest>"
            return 0
        fi
        die "could not resolve the latest release from the GitHub API. Set CODEMUX_VERSION=x.y.z and retry."
    fi
    codemux_normalize_version "$_tag"
}

# ── Dependencies ─────────────────────────────────────────────────────────────

install_dependencies() {
    _packages=$(codemux_dep_packages "$FAMILY")

    if [ "${CODEMUX_NO_DEPS:-0}" = "1" ]; then
        note "CODEMUX_NO_DEPS=1 — skipping dependency installation"
        return 0
    fi

    if [ "$PRIV" = "none" ]; then
        warn "no root — cannot install system libraries"
        print_manual_deps "$_packages"
        return 0
    fi

    case "$FAMILY" in
        debian)
            if have apt-get; then
                # shellcheck disable=SC2086
                as_root env DEBIAN_FRONTEND=noninteractive apt-get update -qq || true
                # shellcheck disable=SC2086
                if as_root env DEBIAN_FRONTEND=noninteractive apt-get install -y $_packages; then
                    ok "runtime libraries installed"
                    return 0
                fi
            fi
            ;;
        fedora)
            if have dnf; then
                # shellcheck disable=SC2086
                if as_root dnf install -y $_packages; then ok "runtime libraries installed"; return 0; fi
            elif have yum; then
                # shellcheck disable=SC2086
                if as_root yum install -y $_packages; then ok "runtime libraries installed"; return 0; fi
            fi
            ;;
        suse)
            if have zypper; then
                # shellcheck disable=SC2086
                if as_root zypper --non-interactive install $_packages; then ok "runtime libraries installed"; return 0; fi
            fi
            ;;
        arch)
            if have pacman; then
                # shellcheck disable=SC2086
                if as_root pacman -S --needed --noconfirm $_packages; then ok "runtime libraries installed"; return 0; fi
            fi
            ;;
    esac

    warn "could not install the runtime libraries automatically"
    print_manual_deps "$_packages"
}

print_manual_deps() {
    note "The extracted AppImage carries its own copies of these libraries, so"
    note "Codemux usually starts anyway. If it does not, install them by hand:"
    note "  ${1}"
}

report_optional_tools() {
    _missing=""
    have git   || _missing="${_missing} git"
    have rg    || _missing="${_missing} ripgrep"
    have fd    || have fdfind || _missing="${_missing} fd"
    have gh    || _missing="${_missing} gh"
    if [ -n "$_missing" ]; then
        note "optional tools not found:${_missing}"
        note "  they unlock fast search, file picking and PR features:"
        note "  $(codemux_optional_packages "$FAMILY")"
    fi
}

# ── Install methods ──────────────────────────────────────────────────────────

# True when dpkg considers the codemux package fully installed (unpacked AND
# configured). `dpkg -i` leaves a package "unpacked, not configured" when its
# Depends are missing, and `apt-get -f install` resolves that by *removing* it
# again — both of which exit in ways that are easy to mistake for success.
deb_is_configured() {
    have dpkg-query || return 1
    dpkg-query -W -f='${Status}' codemux 2>/dev/null \
        | grep -q 'install ok installed'
}

install_deb() {
    _file="$1"
    remove_fallback_install
    step "Installing the Debian package"
    if have apt-get; then
        # apt-get resolves libwebkit2gtk-4.1-0, libgtk-3-0 & friends from the
        # package's own Depends — that is why the .deb path is preferred over
        # extracting the AppImage on Debian/Ubuntu.
        if ! as_root env DEBIAN_FRONTEND=noninteractive apt-get install -y "$_file"; then
            # Most often the package index is empty or stale (a minimal image,
            # or a box that has not run `apt-get update` since release), so apt
            # cannot resolve the Depends and refuses the file. Refresh once and
            # retry before falling back to dpkg — the retry is what makes the
            # dependency resolution this path exists for actually happen.
            warn "apt-get refused the file — refreshing the package index and retrying"
            as_root env DEBIAN_FRONTEND=noninteractive apt-get update -qq || true
            if ! as_root env DEBIAN_FRONTEND=noninteractive apt-get install -y "$_file"; then
                # apt older than 1.1 cannot install a local file path at all.
                warn "apt-get could not take the file directly — falling back to dpkg"
                as_root dpkg -i "$_file" \
                    || as_root env DEBIAN_FRONTEND=noninteractive apt-get -f install -y
            fi
        fi
    elif have dpkg; then
        as_root dpkg -i "$_file" || as_root apt-get -f install -y
    else
        die "neither apt-get nor dpkg found — rerun with CODEMUX_METHOD=appimage"
    fi

    # Do not claim success on a package that dpkg left unconfigured — or that
    # `apt-get -f install` removed again to repair the dependency state. Both
    # exit 0, so without this check the installer prints "✓ installed" and the
    # user only finds out at the verify step, with no idea why.
    if [ "$DRY_RUN" != "1" ] && have dpkg-query && ! deb_is_configured; then
        bad "the codemux package did not install cleanly (unmet dependencies)."
        note "Its runtime libraries could not be resolved from your configured"
        note "APT sources. Try:"
        note "  sudo apt-get update && sudo apt-get install -y $(codemux_dep_packages debian)"
        note "then re-run this installer — or use the self-contained AppImage:"
        note "  CODEMUX_METHOD=appimage sh install.sh"
        exit 1
    fi
    ok "installed from $(basename "$_file")"
}

install_rpm() {
    _file="$1"
    remove_fallback_install
    step "Installing the RPM package"
    if have dnf; then
        as_root dnf install -y "$_file"
    elif have zypper; then
        as_root zypper --non-interactive install --allow-unsigned-rpm "$_file"
    elif have yum; then
        as_root yum install -y "$_file"
    elif have rpm; then
        as_root rpm -Uvh --replacepkgs "$_file"
    else
        die "no dnf/yum/zypper/rpm found — rerun with CODEMUX_METHOD=appimage"
    fi
    ok "installed from $(basename "$_file")"
}

# The AppImage is a squashfs AppDir. `--appimage-extract` unpacks it without
# FUSE (headless VPS images rarely have libfuse2), and the payload keeps the
# layout the binary expects:
#   usr/bin/codemux         RUNPATH=$ORIGIN/../lib  -> bundled GTK/WebKit libs
#   usr/lib/codemux/        Tauri resource dir, resolved as <exe>/../lib/codemux
#   AppRun                  sets the GTK/GIO/pixbuf env the bundled libs need
# so the whole AppDir is installed as a unit and a small wrapper in BIN_DIR
# execs AppRun. This is the same payload the AUR package unpacks.
install_appimage() {
    _file="$1"
    step "Extracting the AppImage payload"

    if [ "$DRY_RUN" = "1" ]; then
        note "would extract ${_file} and install:"
        note "  payload  -> ${LIB_DIR}"
        note "  launcher -> ${BIN_DIR}/codemux"
        note "  desktop  -> ${DATA_DIR}/applications/codemux.desktop"
        return 0
    fi

    _extract_root="${WORKDIR}/extract"
    mkdir -p "$_extract_root"
    chmod +x "$_file"
    ( cd "$_extract_root" && "$_file" --appimage-extract >/dev/null ) \
        || die "failed to extract the AppImage (is ${_file} a valid AppImage?)"

    _payload="${_extract_root}/squashfs-root"
    [ -x "${_payload}/usr/bin/codemux" ] \
        || die "extracted payload has no usr/bin/codemux — artifact looks wrong"

    # AppImage payloads are packed with build-machine permissions; make every
    # file world-readable so a root install works for all users.
    chmod -R u+rwX,go+rX "$_payload"
    ok "payload extracted"

    step "Installing to ${LIB_DIR}"
    # Replacing the whole payload directory is what makes a re-run an upgrade
    # rather than a half-merged mix of two versions.
    maybe_root rm -rf "$LIB_DIR"
    maybe_root mkdir -p "$(dirname "$LIB_DIR")" "$BIN_DIR"
    maybe_root cp -a "$_payload" "$LIB_DIR"
    ok "payload installed"

    _wrapper="${WORKDIR}/codemux-wrapper"
    cat > "$_wrapper" <<WRAPPER
#!/bin/sh
# Generated by the Codemux installer — do not edit.
# Runs the extracted AppImage payload through its own AppRun so the bundled
# GTK/WebKit libraries and their module paths are set up before exec.
APPDIR="${LIB_DIR}"
export APPDIR
if [ -x "\$APPDIR/AppRun" ]; then
    exec "\$APPDIR/AppRun" "\$@"
fi
exec "\$APPDIR/usr/bin/codemux" "\$@"
WRAPPER
    chmod 755 "$_wrapper"

    maybe_root cp "$_wrapper" "${BIN_DIR}/codemux"
    maybe_root chmod 755 "${BIN_DIR}/codemux"
    ok "launcher installed at ${BIN_DIR}/codemux"

    install_desktop_entry
    warn_if_not_on_path
}

install_desktop_entry() {
    _src_desktop="${LIB_DIR}/usr/share/applications/codemux.desktop"
    _src_icon="${LIB_DIR}/usr/share/icons/hicolor/128x128/apps/codemux.png"
    [ -f "$_src_desktop" ] || return 0

    _apps_dir="${DATA_DIR}/applications"
    _icons_dir="${DATA_DIR}/icons/hicolor/128x128/apps"
    _tmp_desktop="${WORKDIR}/codemux.desktop"

    sed -e "s|^Exec=.*|Exec=${BIN_DIR}/codemux|" \
        -e "s|^TryExec=.*|TryExec=${BIN_DIR}/codemux|" \
        "$_src_desktop" > "$_tmp_desktop" 2>/dev/null || return 0

    maybe_root mkdir -p "$_apps_dir" "$_icons_dir"
    maybe_root cp "$_tmp_desktop" "${_apps_dir}/codemux.desktop"
    if [ -f "$_src_icon" ]; then
        maybe_root cp "$_src_icon" "${_icons_dir}/codemux.png"
    fi
    ok "desktop entry installed"
    return 0
}

warn_if_not_on_path() {
    case ":${PATH}:" in
        *":${BIN_DIR}:"*) return 0 ;;
    esac
    warn "${BIN_DIR} is not on your PATH"
    note "Add it to your shell profile, then reopen your shell:"
    note "  echo 'export PATH=\"${BIN_DIR}:\$PATH\"' >> ~/.profile"
}

# ── Verify + next steps ──────────────────────────────────────────────────────

installed_version() {
    _bin="$1"
    # `codemux --version` is not wired up in the clap parser; `capabilities`
    # is the local, socket-free command that reports the build version.
    _v=$("$_bin" capabilities 2>/dev/null | sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)
    printf '%s' "$_v"
}

verify_install() {
    step "Verifying"
    if [ "$DRY_RUN" = "1" ]; then
        note "would run: codemux capabilities   (reports the installed version)"
        return 0
    fi

    _bin=""
    if [ -x "${BIN_DIR}/codemux" ]; then
        _bin="${BIN_DIR}/codemux"
    else
        _bin=$(command -v codemux 2>/dev/null || true)
    fi
    [ -n "$_bin" ] || die "codemux was not found after install — see ${CODEMUX_RELEASES_URL}"

    _v=$(installed_version "$_bin")
    if [ -n "$_v" ]; then
        ok "codemux ${_v} at ${_bin}"
    elif "$_bin" --help >/dev/null 2>&1; then
        warn "installed at ${_bin}, but could not read its version"
    else
        bad "codemux is installed at ${_bin} but will not run."
        note "Usually a missing system library. Try:"
        note "  ${_bin} --help"
        note "  ldd ${_bin} | grep 'not found'"
        exit 1
    fi
}

print_next_steps() {
    say ""
    printf '%b\n' "${C_BOLD}Codemux is installed.${C_RESET}"
    say ""
    printf '%b\n' "  ${C_BLUE}codemux connect${C_RESET}   pair this machine and open it from any browser"
    note "                  one command: starts the server and prints a link + QR"
    say ""
    printf '%b\n' "  ${C_BLUE}codemux serve${C_RESET}     manual alternative — run headless in the foreground"
    note "                  --scope loopback|tailscale|all, --port 4377"
    say ""
    printf '%b\n' "  ${C_BLUE}codemux${C_RESET}           launch the desktop app (needs a graphical session)"
    say ""
    note "Docs: ${CODEMUX_RELEASES_URL%/releases}#readme"
    report_optional_tools
    say ""
}

# ── Main ─────────────────────────────────────────────────────────────────────

main() {
    while [ $# -gt 0 ]; do
        case "$1" in
            --dry-run) DRY_RUN=1 ;;
            --version) shift; [ $# -gt 0 ] || die "--version needs a value"; CODEMUX_INSTALL_VERSION="$1" ;;
            --version=*) CODEMUX_INSTALL_VERSION="${1#--version=}" ;;
            -h|--help) usage; exit 0 ;;
            *) die "unknown option: $1 (try --help)" ;;
        esac
        shift
    done

    say ""
    printf '%b\n' "${C_BOLD}Codemux installer${C_RESET}"
    [ "$DRY_RUN" = "1" ] && printf '%b\n' "${C_YELLOW}dry run — nothing will be changed${C_RESET}"
    say ""

    step "Checking your system"
    check_platform
    detect_privileges

    FAMILY=$(codemux_detect_family "${CODEMUX_OS_RELEASE_PATH:-/etc/os-release}")
    _pretty=$(codemux_os_release_field "${CODEMUX_OS_RELEASE_PATH:-/etc/os-release}" PRETTY_NAME)
    if [ -n "$_pretty" ]; then
        ok "${_pretty} (${FAMILY} family)"
    else
        ok "distro family: ${FAMILY}"
    fi

    # ── Choose the install method ────────────────────────────────────────
    if [ -n "${CODEMUX_METHOD:-}" ]; then
        METHOD="$CODEMUX_METHOD"
        note "CODEMUX_METHOD=${METHOD} (forced)"
    elif [ -n "${CODEMUX_ARTIFACT:-}" ]; then
        METHOD=$(codemux_kind_from_filename "$CODEMUX_ARTIFACT")
        [ "$METHOD" = "unknown" ] && die "cannot tell what CODEMUX_ARTIFACT is from its name: ${CODEMUX_ARTIFACT}"
        note "CODEMUX_ARTIFACT is a .${METHOD} — installing that"
    else
        METHOD=$(codemux_method_for_family "$FAMILY")
    fi

    # The AUR package is the better Arch install however the artifact arrived,
    # so the pointer is keyed on the family, not on the chosen method.
    if [ "$FAMILY" = "arch" ]; then
        say ""
        printf '%b\n' "  ${C_YELLOW}Arch users:${C_RESET} the packaged build is on the AUR and is the"
        note "recommended install — it upgrades with the rest of your system:"
        note "  yay -S codemux-bin      # or: paru -S codemux-bin"
        note "Continuing with the AppImage install; you can switch to the AUR"
        note "package any time (this installer's files are removed when you do)."
        say ""
    fi
    [ "$METHOD" = "arch" ] && METHOD="appimage"

    if [ "$METHOD" = "deb" ] && ! have apt-get && ! have dpkg; then
        warn "no apt-get/dpkg on this system — falling back to the AppImage"
        METHOD="appimage"
    fi
    if [ "$METHOD" = "rpm" ] && ! have dnf && ! have yum && ! have zypper && ! have rpm; then
        warn "no dnf/yum/zypper/rpm on this system — falling back to the AppImage"
        METHOD="appimage"
    fi
    if [ "$PRIV" = "none" ] && { [ "$METHOD" = "deb" ] || [ "$METHOD" = "rpm" ]; }; then
        warn "no root and no sudo — falling back to a home-directory install"
        METHOD="appimage"
    fi

    resolve_dirs
    report_existing_install
    say ""

    # ── Get the artifact ─────────────────────────────────────────────────
    WORKDIR=$(mktemp -d "${TMPDIR:-/tmp}/codemux-install.XXXXXX")

    _artifact=""
    if [ -n "${CODEMUX_ARTIFACT:-}" ]; then
        case "$CODEMUX_ARTIFACT" in
            http://*|https://*)
                step "Downloading the artifact you pointed at"
                _artifact="${WORKDIR}/$(basename "$CODEMUX_ARTIFACT")"
                download_to "$CODEMUX_ARTIFACT" "$_artifact"
                ;;
            *)
                [ -f "$CODEMUX_ARTIFACT" ] || die "CODEMUX_ARTIFACT not found: ${CODEMUX_ARTIFACT}"
                _artifact=$(cd "$(dirname "$CODEMUX_ARTIFACT")" && pwd)/$(basename "$CODEMUX_ARTIFACT")
                step "Using local artifact"
                ok "$_artifact"
                ;;
        esac
    else
        step "Resolving the release"
        _version=$(resolve_version)
        if [ -n "$(codemux_pinned_version)" ]; then
            ok "version ${_version} (pinned)"
        else
            ok "latest release: ${_version}"
        fi

        _url=$(codemux_artifact_url "$METHOD" "$_version") \
            || die "no artifact of kind '${METHOD}' is published"
        _artifact="${WORKDIR}/$(codemux_artifact_name "$METHOD" "$_version")"

        say ""
        step "Downloading $(codemux_artifact_name "$METHOD" "$_version")"
        [ "$DRY_RUN" = "1" ] || note "$_url"
        download_to "$_url" "$_artifact"
        [ "$DRY_RUN" = "1" ] || ok "downloaded ($(du -h "$_artifact" 2>/dev/null | cut -f1))"
    fi

    say ""

    # ── Install ──────────────────────────────────────────────────────────
    case "$METHOD" in
        deb) install_deb "$_artifact" ;;
        rpm) install_rpm "$_artifact" ;;
        appimage)
            step "Making sure the runtime libraries are present"
            install_dependencies
            say ""
            install_appimage "$_artifact"
            ;;
        *) die "unknown install method: ${METHOD}" ;;
    esac

    say ""
    verify_install
    print_next_steps
}

# Sourced by scripts/install-sh.test.sh, which only wants the pure helpers.
if [ "${CODEMUX_INSTALL_SH_NO_MAIN:-0}" != "1" ]; then
    main "$@"
fi
