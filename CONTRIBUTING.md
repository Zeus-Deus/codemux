# Contributing to Codemux

## Quick Start

```bash
git clone git@github.com:Zeus-Deus/codemux.git
cd codemux

# Check that system dependencies are installed
bash scripts/check-deps.sh

# Install npm dependencies (also patches agent-browser via postinstall)
npm install

# Run frontend type checks
npm run check

# Launch the desktop app in dev mode
npm run tauri:dev
```

## System Requirements

### Toolchain

- **Rust 1.75+** via [rustup](https://rustup.rs): `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
- **Node.js 20+** with npm
- **Tauri CLI** installed automatically as an npm devDependency (`@tauri-apps/cli`)

### System Libraries

Tauri 2 requires platform-specific system libraries for WebKit, GTK, and TLS.

#### Arch Linux

```bash
sudo pacman -S webkit2gtk-4.1 base-devel curl wget file openssl appmenu-gtk-module gtk3 libappindicator-gtk3 librsvg pkg-config
```

#### Ubuntu / Debian

```bash
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file libssl-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev pkg-config
```

#### Fedora

```bash
sudo dnf install webkit2gtk4.1-devel openssl-devel curl wget file libappindicator-gtk3-devel librsvg2-devel gcc-c++ make gtk3-devel pkg-config
```

### Dependency Check Script

Run `bash scripts/check-deps.sh` to verify all required and optional dependencies. The script is read-only and never installs anything.

## Build and Run

### Dev Mode (Full Desktop App)

```bash
npm run tauri:dev
```

This starts the Vite dev server on port 1420 and launches the Tauri desktop window with hot reload. The WebKitGTK renderer is selected by the app itself (`src-tauri/src/webview_tuning.rs`), not by the dev script.

### Dev Mode (X11 Fallback)

```bash
npm run tauri:dev:x11
```

Forces X11 backend via `GDK_BACKEND=x11`. Use this if you have rendering issues on your Wayland compositor.

### Frontend Only

```bash
npm run dev
```

Starts only the Vite dev server on `localhost:1420`. Useful for iterating on React/Tailwind UI without the Tauri shell.

### Verification

```bash
npm run check
CARGO_BUILD_JOBS=2 cargo check --manifest-path src-tauri/Cargo.toml
```

Use the smallest relevant check while iterating. Run affected frontend tests with
`npm run test -- <test-file>` and affected Rust tests with
`cargo test -j 2 --manifest-path src-tauri/Cargo.toml <filter> -- --test-threads=2`.
CI owns the full repository-wide suite.

### CLI

```bash
npm run cli -- <subcommand>        # Run CLI commands via cargo
npm run build:cli                   # Build and install to ~/.local/bin/codemux
```

## Optional Dependencies

These are not required to build or run Codemux but enable additional features. All degrade gracefully when absent.

| Binary                          | Feature                       | Fallback                                                |
| ------------------------------- | ----------------------------- | ------------------------------------------------------- |
| `chromium` / `chrome` / `brave` | Browser pane                  | Multiple candidates tried in order; error if none found |
| `rg` (ripgrep)                  | Code search (`Ctrl+Shift+F`)  | Falls back to `grep`                                    |
| `fd`                            | File search (`Ctrl+P`)        | Falls back to `find`                                    |
| `gh`                            | GitHub PR integration         | PR features disabled                                    |
| `claude`                        | Claude Code AI agent          | Other agents or skip                                    |
| `opencode`                      | OpenCode AI agent             | Other agents or skip                                    |
| `codex`                         | Codex AI agent (legacy)       | Other agents or skip                                    |
| `ydotool` + `ydotoold`          | Tier 3 OS-level browser input | CDP-based Tier 1/2 still work                           |

## Known Gotchas

- **WebKit2GTK version**: Must be 4.1 specifically, not 4.0 or 6.0. The package name varies by distro.
- **Wayland GPU rendering**: the app sets `WEBKIT_DMABUF_RENDERER_FORCE_SHM=1` itself, which keeps accelerated compositing while avoiding the GBM/EGL and Wayland protocol errors. If startup still dies in the renderer, run with `CODEMUX_WEBKIT_COMPAT=1` for the legacy CPU path (slower scrolling) and report it.
- **X11 fallback**: Some Wayland compositors need `GDK_BACKEND=x11`. Use `npm run tauri:dev:x11`.
- **agent-browser sidecar**: The `agent-browser` binary is bundled as a Tauri sidecar. Run `bash scripts/copy-agent-browser.sh` after `npm install` to copy it into `src-tauri/binaries/`. If browser automation breaks after a package update, re-run the copy script.
- **Stale CLI binary**: `npm run build:cli` copies the binary to `~/.local/bin/codemux`. This can shadow the dev build if you forget it's there. Remove it with `rm ~/.local/bin/codemux` when you don't need it.
- **`.mcp.json` is auto-generated**: Codemux writes `.mcp.json` per-workspace at runtime with the current binary path. It's in `.gitignore` — never commit or manually edit it.
- **Auth in dev mode**: If the auth API (`api.codemux.org`) is unreachable, the app auto-bypasses auth with a dev placeholder user. No account needed for local development. To point to a local auth API, set `CODEMUX_API_URL=http://localhost:3000`.
- **Claude Code hooks in `~/.claude/settings.json`**: Codemux writes hook entries to this file on startup for agent status tracking. If Claude Code shows a settings error, check this file for entries under the `hooks` key pointing to `~/.codemux/hooks/notify.sh`. The hooks are harmless — they silently no-op when Codemux isn't running.

## Project Layout

```
src/            React + Tailwind v4 + shadcn frontend
src-tauri/      Rust backend — Tauri 2, CLI, PTY, browser, agent providers
scripts/        Build and patch helper scripts
```

### Frontend Conventions

- All Tauri IPC goes through typed wrappers in `src/tauri/commands.ts` — never import `@tauri-apps/api` directly
- State management: zustand stores in `src/stores/`
- UI primitives: shadcn in `src/components/ui/`, app components in `src/components/layout/`
- Path alias: `@/*` maps to `./src/*`

### Backend Conventions

- Tauri commands split by domain in `src-tauri/src/commands/`
- App state in `src-tauri/src/state/`
- Browser runtimes: `src-tauri/src/agent_browser.rs` (primary), `src-tauri/src/browser.rs` (legacy CDP)
- Workflow orchestration: `src/components/workflow/`

## Documentation

- `README.md` covers the product and installation.
- This file covers contributor setup and repository conventions.
- `AGENTS.md` contains concise defaults for coding agents.
- User-facing product documentation is maintained in the separate Codemux website repository.
- Current code and tests are authoritative for implementation behavior.

## Commit Conventions

Use conventional commit prefixes based on the type of change:

- `feat:` — new feature
- `fix:` — bug fix
- `docs:` — documentation only
- `refactor:` — code restructuring without behavior change
- `chore:` — build, tooling, or maintenance
- `test:` — test additions or changes

## Submitting Changes

1. Fork the repository and create a feature branch
2. Make your changes
3. Run the relevant focused checks; the full suite runs in CI
4. Submit a pull request with a clear description of the change
