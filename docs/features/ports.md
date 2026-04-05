# Port Detection

- Purpose: Describe the automatic port detection and management subsystem.
- Audience: Anyone working on port scanning, sidebar port display, or browser-from-port flows.
- Authority: Canonical feature doc for port detection.
- Update when: Detection logic, ignored ports, sidebar behavior, or kill-port flow change.
- Read next: `docs/features/browser.md`, `docs/features/setup-teardown.md`

## What This Feature Is

Codemux automatically detects TCP ports that dev servers open, displays them in the sidebar, and lets users open them in the browser pane or kill the owning process.

## Current Model

The Rust backend parses `/proc/net/tcp` and `/proc/net/tcp6` for listening sockets, resolves owning PIDs via `/proc/*/fd/` symlinks, and maps ports to process names. Results are filtered to exclude system services and Codemux-internal port ranges.

## What Works Today

- automatic detection of listening TCP ports owned by the current user
- sidebar section showing port number, process name, and optional label
- open a detected port in the browser pane
- kill a port's owning process
- static port labels via `.codemux/config.json` ports configuration
- filtering: system ports (22, 80, 443, 5432, 3306, 6379, 27017), Codemux internals (3900-4199, 9222+) are excluded

## Current Constraints

- Linux-only (`/proc` filesystem required)
- polling-based, not event-driven
- no per-workspace port scoping (shows all user ports globally)
- UDP ports are not detected

## Important Touch Points

- `src-tauri/src/ports.rs` — `detect_listening_ports()`, `PortInfo`, `/proc` parsing, ignored port lists
- `src-tauri/src/commands/mod.rs` — `get_detected_ports`, `kill_port`
- `src/components/layout/sidebar-ports-section.tsx` — sidebar port display and actions
