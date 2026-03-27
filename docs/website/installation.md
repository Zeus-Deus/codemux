---
title: Installation
description: Build Codemux from source on Linux with Tauri 2, Rust, and Node.js.
---

# Installation

Codemux runs on Linux. It's built with Tauri 2 and requires Rust and Node.js to compile.

## Requirements

- **OS**: Linux (X11 or Wayland)
- **Rust**: 1.75+ (install via [rustup](https://rustup.rs))
- **Node.js**: 20+ with npm
- **System libraries**: Tauri 2 dependencies for your distro

### Arch Linux

```bash
sudo pacman -S webkit2gtk-4.1 base-devel curl wget file openssl appmenu-gtk-module gtk3 libappindicator-gtk3 librsvg
```

### Ubuntu/Debian

```bash
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file libssl-dev libayatana-appindicator3-dev librsvg2-dev
```

## Build from Source

```bash
git clone https://github.com/user/codemux.git
cd codemux
npm install
npm run build
```

For development mode with hot reload:

```bash
npm run dev
```

This starts the Vite dev server and the Tauri application together.

## First Launch

1. Codemux opens with a default workspace
2. A terminal pane is ready — start typing commands
3. Press `Ctrl+T` to open more tabs, `Ctrl+Shift+D` to split panes
4. Click the `+` in the sidebar to create additional workspaces

## Verification

Run the full test suite to confirm the build:

```bash
npm run verify
```

This runs `cargo check`, `cargo test`, `tsc --noEmit`, and `vitest`.

## Optional Tools

These are not required but enable additional features:

- **gh** (GitHub CLI) — Enables PR creation, CI checks, review comments
- **claude** (Claude Code CLI) — AI commit messages and merge conflict resolution
- **codex** / **opencode** — Alternative AI CLI tools
- **fd** — Fast file search (`Ctrl+P`)
- **ripgrep** — Fast code search (`Ctrl+Shift+F`)
- **ydotool** — OS-level browser input for stealth automation (Tier 3). Requires `ydotoold` daemon running. Only needed for bypassing anti-bot detection.
- **docker** / **docker compose** — Container management for setup/teardown scripts
