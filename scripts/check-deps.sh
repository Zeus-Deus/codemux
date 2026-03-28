#!/bin/bash
# Check Codemux development dependencies.
# Non-destructive — only reads, never installs or modifies anything.
# Run: bash scripts/check-deps.sh

set -euo pipefail

# --- Colors ---
if [ -t 1 ]; then
    GREEN='\033[0;32m'
    RED='\033[0;31m'
    YELLOW='\033[0;33m'
    BOLD='\033[1m'
    DIM='\033[2m'
    RESET='\033[0m'
else
    GREEN='' RED='' YELLOW='' BOLD='' DIM='' RESET=''
fi

pass_count=0
warn_count=0
fail_count=0

pass()  { echo -e "  ${GREEN}PASS${RESET}  $1"; pass_count=$((pass_count + 1)); }
fail()  { echo -e "  ${RED}FAIL${RESET}  $1"; fail_count=$((fail_count + 1)); }
warn()  { echo -e "  ${YELLOW}SKIP${RESET}  $1"; warn_count=$((warn_count + 1)); }
header() { echo -e "\n${BOLD}$1${RESET}"; }

# --- Distro detection ---
detect_distro() {
    if [ -f /etc/os-release ]; then
        . /etc/os-release
        case "$ID" in
            arch|manjaro|endeavouros) echo "arch" ;;
            ubuntu|debian|pop|linuxmint|zorin) echo "debian" ;;
            fedora|rhel|centos|nobara) echo "fedora" ;;
            *) echo "unknown" ;;
        esac
    else
        echo "unknown"
    fi
}

install_hint() {
    local pkg_arch="$1" pkg_deb="$2" pkg_fed="$3"
    local distro
    distro=$(detect_distro)
    case "$distro" in
        arch)   echo "${DIM}  install: sudo pacman -S ${pkg_arch}${RESET}" ;;
        debian) echo "${DIM}  install: sudo apt install ${pkg_deb}${RESET}" ;;
        fedora) echo "${DIM}  install: sudo dnf install ${pkg_fed}${RESET}" ;;
        *)      echo "${DIM}  install: check your distro's package manager${RESET}" ;;
    esac
}

# --- Version comparison ---
# Returns 0 if $1 >= $2 (major.minor comparison only)
version_gte() {
    local ver="$1" min="$2"
    local ver_major ver_minor min_major min_minor
    ver_major=$(echo "$ver" | cut -d. -f1)
    ver_minor=$(echo "$ver" | cut -d. -f2)
    min_major=$(echo "$min" | cut -d. -f1)
    min_minor=$(echo "$min" | cut -d. -f2)
    if [ "$ver_major" -gt "$min_major" ] 2>/dev/null; then return 0; fi
    if [ "$ver_major" -eq "$min_major" ] 2>/dev/null && [ "$ver_minor" -ge "$min_minor" ] 2>/dev/null; then return 0; fi
    return 1
}

# --- Required checks ---
header "Required — Build will fail without these"

# Rust
if command -v rustc &>/dev/null; then
    rust_ver=$(rustc --version | awk '{print $2}')
    if version_gte "$rust_ver" "1.75"; then
        pass "rustc $rust_ver (>= 1.75)"
    else
        fail "rustc $rust_ver (need >= 1.75)"
        echo -e "$(install_hint 'rustup' 'rustup' 'rustup')"
    fi
else
    fail "rustc not found"
    echo -e "${DIM}  install: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh${RESET}"
fi

# Cargo
if command -v cargo &>/dev/null; then
    pass "cargo $(cargo --version | awk '{print $2}')"
else
    fail "cargo not found (install Rust via rustup)"
fi

# Node.js
if command -v node &>/dev/null; then
    node_ver=$(node --version | sed 's/^v//')
    node_major=$(echo "$node_ver" | cut -d. -f1)
    if [ "$node_major" -ge 20 ] 2>/dev/null; then
        pass "node $node_ver (>= 20)"
    else
        fail "node $node_ver (need >= 20)"
    fi
else
    fail "node not found"
    echo -e "$(install_hint 'nodejs npm' 'nodejs npm' 'nodejs npm')"
fi

# npm
if command -v npm &>/dev/null; then
    pass "npm $(npm --version)"
else
    fail "npm not found"
fi

# git
if command -v git &>/dev/null; then
    pass "git $(git --version | awk '{print $3}')"
else
    fail "git not found"
    echo -e "$(install_hint 'git' 'git' 'git')"
fi

# pkg-config
if command -v pkg-config &>/dev/null; then
    pass "pkg-config"
else
    fail "pkg-config not found"
    echo -e "$(install_hint 'pkg-config' 'pkg-config' 'pkg-config')"
fi

# WebKit2GTK 4.1
if command -v pkg-config &>/dev/null && pkg-config --exists webkit2gtk-4.1 2>/dev/null; then
    webkit_ver=$(pkg-config --modversion webkit2gtk-4.1 2>/dev/null || echo "unknown")
    pass "webkit2gtk-4.1 ($webkit_ver)"
else
    fail "webkit2gtk-4.1 not found"
    echo -e "$(install_hint 'webkit2gtk-4.1' 'libwebkit2gtk-4.1-dev' 'webkit2gtk4.1-devel')"
fi

# GTK3
if command -v pkg-config &>/dev/null && pkg-config --exists gtk+-3.0 2>/dev/null; then
    pass "gtk3"
else
    fail "gtk3 not found"
    echo -e "$(install_hint 'gtk3' 'libgtk-3-dev' 'gtk3-devel')"
fi

# OpenSSL
if command -v pkg-config &>/dev/null && pkg-config --exists openssl 2>/dev/null; then
    pass "openssl"
else
    fail "openssl dev headers not found"
    echo -e "$(install_hint 'openssl' 'libssl-dev' 'openssl-devel')"
fi

# --- Optional checks ---
header "Optional — Enhanced features, not required to build"

# Browser (any one of the candidates)
browser_found=""
for bin in chromium chromium-browser google-chrome-stable google-chrome brave-browser; do
    if command -v "$bin" &>/dev/null; then
        browser_found="$bin"
        break
    fi
done
if [ -n "$browser_found" ]; then
    pass "browser: $browser_found (browser pane)"
else
    warn "no chromium/chrome/brave found (browser pane won't work)"
    echo -e "$(install_hint 'chromium' 'chromium-browser' 'chromium')"
fi

# ripgrep
if command -v rg &>/dev/null; then
    pass "rg (fast code search, Ctrl+Shift+F)"
else
    warn "rg not found (falls back to grep)"
    echo -e "$(install_hint 'ripgrep' 'ripgrep' 'ripgrep')"
fi

# fd
if command -v fd &>/dev/null; then
    pass "fd (fast file search, Ctrl+P)"
else
    warn "fd not found (falls back to find)"
    echo -e "$(install_hint 'fd' 'fd-find' 'fd-find')"
fi

# GitHub CLI
if command -v gh &>/dev/null; then
    pass "gh (GitHub PR integration)"
else
    warn "gh not found (PR features disabled)"
    echo -e "$(install_hint 'github-cli' 'gh' 'gh')"
fi

# AI CLI tools
for tool in claude opencode codex; do
    if command -v "$tool" &>/dev/null; then
        pass "$tool (AI agent preset)"
    else
        warn "$tool not found (agent preset unavailable)"
    fi
done

# ydotool
if command -v ydotool &>/dev/null; then
    if systemctl --user is-active ydotool &>/dev/null; then
        pass "ydotool + ydotoold daemon (Tier 3 browser input)"
    else
        warn "ydotool found but ydotoold daemon not running"
        echo -e "${DIM}  start: systemctl --user enable --now ydotool${RESET}"
    fi
else
    warn "ydotool not found (Tier 3 browser input unavailable, Tier 1/2 still work)"
    echo -e "$(install_hint 'ydotool' 'ydotool' 'ydotool')"
fi

# bubblewrap
if command -v bwrap &>/dev/null; then
    pass "bwrap (process sandboxing)"
else
    warn "bwrap not found (agents run without sandbox)"
    echo -e "$(install_hint 'bubblewrap' 'bubblewrap' 'bubblewrap')"
fi

# --- Summary ---
header "Summary"
total=$((pass_count + warn_count + fail_count))
echo -e "  ${GREEN}${pass_count} passed${RESET}, ${YELLOW}${warn_count} skipped${RESET}, ${RED}${fail_count} failed${RESET} (${total} total)"

if [ "$fail_count" -gt 0 ]; then
    echo -e "\n${RED}${BOLD}Install the failed dependencies before building.${RESET}"
    exit 1
else
    echo -e "\n${GREEN}Ready to build.${RESET} Run: npm install && npm run tauri:dev"
    exit 0
fi
