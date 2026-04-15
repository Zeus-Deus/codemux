# Port Detection

- Purpose: Describe the automatic port detection and management subsystem.
- Audience: Anyone working on port scanning, sidebar port display, or browser-from-port flows.
- Authority: Canonical feature doc for port detection.
- Update when: Detection logic, ignored ports, sidebar behavior, or kill-port flow change.
- Read next: `docs/features/browser.md`, `docs/features/setup-teardown.md`

## What This Feature Is

Codemux automatically detects TCP ports that dev servers open, displays them in the sidebar, and lets users open them in the browser pane or kill the owning process.

## Current Model

`detect_listening_ports` dispatches on the host OS:

- **Linux**: parses `/proc/net/tcp` and `/proc/net/tcp6` for `LISTEN`-state sockets, resolves owning PIDs via `/proc/*/fd/` symlinks, and maps them to process names. Non-root processes can only read their own user's fd dirs, so sockets owned by `root` / `systemd` / etc. are silently dropped at the inode→PID resolution step. This is Linux's natural permission filter for system services.
- **Windows**: shells out to `netstat -ano` (with `CREATE_NO_WINDOW` to suppress the console flash) and `tasklist /NH /FO csv`, then parses both via pure cross-platform `parse_netstat_output` / `parse_tasklist_csv` helpers. The parsers are unit-tested on Linux CI so a Windows runner isn't needed to catch parser regressions. Because `netstat -ano` lists EVERY listening socket regardless of owner, an explicit process-name filter (`WINDOWS_SYSTEM_PROCESS_NAMES`) drops kernel-owned sockets that Linux's permission filter would never have surfaced.
- **Other platforms**: returns an empty list.

Results are filtered to exclude system services and Codemux-internal port ranges on all platforms. The Windows system-process name filter (case-insensitive ASCII match) covers `System` / `Idle` / `smss.exe` / `csrss.exe` / `wininit.exe` / `winlogon.exe` / `services.exe` / `lsass.exe` / `svchost.exe` / `dwm.exe` / `spoolsv.exe` / `SearchIndexer.exe` / `MsMpEng.exe` / `RuntimeBroker.exe` / `dllhost.exe` / `WmiPrvSE.exe` / and ~10 others. User-runnable dev tools (`node.exe`, `python.exe`, browsers, IDE language servers) are intentionally NOT in the filter — only kernel + service-host processes are dropped.

## What Works Today

- automatic detection of listening TCP ports owned by the current user (Linux + Windows)
- sidebar section showing port number, process name, and optional label
- open a detected port in the browser pane
- kill a port's owning process (`kill -9` on Unix, `taskkill /PID {pid} /F` on Windows)
- static port labels via `.codemux/config.json` ports configuration
- filtering: system ports (22, 80, 443, 5432, 3306, 6379, 27017), Codemux internals (3900-4199, 9222+) are excluded
- Windows system-process name filter (`WINDOWS_SYSTEM_PROCESS_NAMES`) drops kernel + service-host owned sockets that `netstat -ano` would otherwise surface (16+ ports on a typical Windows host)
- IPv4 + IPv6 dedup on Windows (services that bind to both `0.0.0.0:port` and `[::]:port` show as one entry)
- exact-port matching (not substring) so a process listening on `:92230` is never confused with `:9223`

## Current Constraints

- macOS port detection is not implemented (returns empty list) — needs a `lsof`-based or `libproc` backend
- polling-based, not event-driven (3-second interval)
- no per-workspace port scoping (shows all user ports globally)
- UDP ports are not detected
- Windows parent-PID walk uses `wmic process ... get ParentProcessId`, which is deprecated on Windows 11 24H2+ — the workspace attribution degrades gracefully to "unassigned" when it fails

## Important Touch Points

- `src-tauri/src/ports.rs` — top-level `detect_listening_ports()` dispatch, `PortInfo` struct, `WINDOWS_SYSTEM_PROCESS_NAMES` constant + `is_windows_system_process()` filter, cross-platform `parse_netstat_output` / `parse_tasklist_csv` pure parsers, Linux `/proc` helpers, Windows `windows_impl` module with `netstat`/`tasklist`/`wmic` I/O wrappers
- `src-tauri/src/commands/mod.rs` — `get_detected_ports`, `kill_port` (branches on `cfg!(windows)`)
- `src/components/layout/sidebar-ports-section.tsx` — sidebar port display and actions
