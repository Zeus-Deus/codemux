# Control Protocol Reference

- Purpose: Canonical reference for Codemux CLI and socket control.
- Audience: Agents, scripts, and developers automating the app.
- Authority: Stable protocol and command reference.
- Update when: Command families, request shapes, or security assumptions change.
- Read next: `docs/features/browser.md`, `docs/features/openflow.md`, `AGENTS.md`

## Transport

- socket path (Linux/macOS): `$XDG_RUNTIME_DIR/codemux.sock` (falls back to `/tmp/codemux-{uid}/codemux.sock` when `XDG_RUNTIME_DIR` is unset)
- named pipe (Windows): `\\.\pipe\codemux-{username}` (`USERNAME` env var, sanitized to alphanumerics + `_-`)
- protocol version: `1`
- transport shape: one JSON request per line, one JSON response per line

## Security Model

- local-user socket only
- same-machine control only
- intended for trusted local automation
- no authentication or capability gates yet

## Request Format

```json
{"command":"status","params":{}}
```

## Response Format

```json
{
  "ok": true,
  "protocol_version": 1,
  "data": {},
  "error": null
}
```

## Command Families

- app and workspace state: `status`, `get_app_state`, `create_workspace`, `split_pane`
- terminal control: `write_terminal`
- notifications: `notify`
- browser control: `create_browser_pane`, `open_url`, `browser_automation`
- memory and handoff: `get_project_memory`, `update_project_memory`, `add_project_memory_entry`, `generate_handoff`
- indexing: `rebuild_index`, `index_status`, `search_index`

## Boundary Notes

- frontend Tauri commands, socket control, and CLI are separate surfaces, but workspace and browser socket actions now reuse the same Rust helper implementations used by the Tauri command layer
- browser automation is centered on the `agent-browser` path
- CLI browser commands use the same internal `agent-browser` execution helpers as the runtime manager

## CLI Examples

```bash
codemux status
codemux notify "Agent needs approval"
codemux json get_app_state
codemux json split_pane '{"pane_id":"pane-1","direction":"horizontal"}'
codemux browser create
codemux browser open https://example.com
codemux browser snapshot
codemux memory show
codemux handoff
codemux index build
codemux logs --tail 200
codemux doctor
```

## Local Diagnostics

`codemux logs` and `codemux doctor` run entirely locally — no running
Codemux instance or control socket needed, so they work precisely when
the app itself is misbehaving.

- `codemux logs [--tail <n>]` prints the last `n` lines (default 200)
  of the desktop app's persistent log file (written via
  tauri-plugin-log to the platform app-log dir, e.g.
  `~/.local/share/com.codemux.app/logs/codemux.log` on Linux).
- `codemux doctor` checks the local environment and prints an
  actionable report: desktop/session info, whether the XDG desktop
  portal file chooser or the zenity fallback is available (file
  dialogs silently failed on portal-less minimal WM setups before the
  issue #95 fix), and where the log file lives. When the portal can't
  serve a dialog it prints a cause-specific remediation that
  distinguishes "portal not installed" from "portal installed but not
  starting" (the common minimal-WM case, where reinstalling packages
  does nothing) — the same message the in-app toast shows.

## Browser Note

From agent terminals, always use explicit `codemux browser ...` subcommands. Do not use `xdg-open`, `open`, or any other system-browser launcher when the goal is to work inside Codemux.
