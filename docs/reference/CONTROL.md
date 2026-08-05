# Control Protocol Reference

- Purpose: Canonical reference for Codemux CLI and socket control.
- Audience: Agents, scripts, and developers automating the app.
- Authority: Stable protocol and command reference.
- Update when: Command families, request shapes, or security assumptions change.
- Read next: `docs/features/browser.md`, `docs/features/workflow-orchestration.md`, `AGENTS.md`

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

`control.rs` dispatches 41 commands. `codemux capabilities` prints the authoritative
machine-readable listing — prefer it over this table when scripting.

- app and workspace state: `status`, `get_app_state`, `create_workspace`, `create_worktree_workspace`, `activate_workspace`, `close_workspace`, `split_pane`, `close_pane`
- workspace archive: `archive_workspace`, `unarchive_workspace`, `list_archived_workspaces`
- terminal control: `write_terminal`, `read_terminal`
- presets: `get_presets`, `apply_preset`
- notifications: `notify`
- browser control: `create_browser_pane`, `open_url`, `browser_automation`
- memory and handoff: `get_project_memory`, `update_project_memory`, `add_project_memory_entry`, `generate_handoff`
- indexing: `rebuild_index`, `index_status`, `search_index`
- ports: `port_list`
- GitHub issues: `list_github_issues`, `get_github_issue`, `link_workspace_issue`
- setup scripts: `rerun_setup`
- automations: `automation_list`, `automation_get`, `automation_create`, `automation_update`, `automation_set_enabled`, `automation_delete`, `automation_runs` (note: the socket uses `automation_set_enabled`; the MCP surface instead exposes `automation_pause`/`automation_resume`)
- web remote access: `web_remote_enable` (turn remote access on; optional `scope` = `all|tailscale|loopback` and `port`), `web_remote_disable` (turn it off, severing live connections), `web_remote_pair` (mint a one-time pairing code; errors if remote access is disabled), `web_remote_set_relay` (flip the from-anywhere relay mode on a running instance — persists and starts/stops the iroh endpoint + device registration in lockstep, same path as the Settings toggle). All same-machine only — the SSH-in path, no GUI needed. `web_remote_enable` returns the resulting status plus the recommended reachable endpoint, so you can immediately `web_remote_pair`.

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
codemux remote enable
codemux remote enable --scope tailscale --port 4377
codemux remote disable
codemux remote pair
codemux remote pair --name "Kitchen iPad"
codemux serve
codemux serve --scope loopback --port 4377
codemux serve --scope tailscale --relay
codemux login
codemux login --email user@example.com
codemux login --token <bearer>
codemux whoami
codemux logout
codemux connect
codemux connect status
codemux connect off
codemux logs --tail 200
codemux doctor
```

The examples above are a sampler, not the full surface. Other subcommands:
`codemux capabilities` (JSON listing of every command — the authoritative
source), `codemux app`, `codemux mcp` (stdio MCP server), `codemux pty-daemon
--socket <path>`, `codemux workspace rerun-setup [workspace_id]`, `codemux
issue list|view|link`, `codemux memory set|add`, `codemux index status|search`,
and the full `codemux browser` set beyond `create|open|snapshot`: `click`,
`fill`, `screenshot`, `console-logs`, `click-at`, `type-at`, `scroll-at`,
`key-press`, `drag`, `click-os`, `type-os`, `viewport`, `viewport-presets`
(documented in `docs/reference/BROWSER-AGENT-COMMANDS.md`).

`codemux remote enable [--scope all|tailscale|loopback] [--port N]` turns web
remote access on (binding the server) and prints the resulting port, bind
scope, and recommended reachable endpoint — so you can immediately run
`codemux remote pair`. `--scope`/`--port` while it is already running rebind
the listener (dropping live connections); with no flags on an already-running
server it just reports the current status. `codemux remote disable` turns it
off and severs every live connection. Both require the desktop app to be
running (a clear "Failed to connect to Codemux control endpoint" error prints
otherwise).

`codemux remote pair` prints a scannable terminal QR of the pairing URL plus
the raw link, token, and expiry — pair a phone/laptop over SSH without
opening the desktop GUI. Requires remote access to be enabled first (via
`codemux remote enable` or `Settings → Remote Access`).

`codemux serve [--scope all|tailscale|loopback] [--port N] [--relay]` runs
Codemux headless as a web-remote server — **no desktop GUI**. Unlike the
`remote *` subcommands (which are control-socket round-trips to a *running*
desktop app), `serve` is itself a long-lived foreground process: it boots the
full backend headless, binds the web-remote server through the same shared
enable path, prints a scannable pairing QR + link, and runs until Ctrl-C /
SIGTERM. Flags mirror `codemux remote enable` (`--scope` same three values);
`--relay` also enables the from-anywhere iroh transport. Ideal over SSH on a
machine with no display. It refuses to start (non-zero exit) if a GUI or
another `serve` already holds this machine's control endpoint; the GUI
reciprocally refuses while `serve` is running. Because `serve` runs its own
control server, `codemux remote pair` from another SSH session mints fresh
codes against it. See `docs/features/web-remote-access.md` §
"Headless server mode".

`codemux login` / `logout` / `whoami` are **local-only** account commands — no
control socket, no running instance required (unlike the `remote *`
round-trips). They persist the same cached auth the GUI writes; see
`docs/features/auth.md` § "CLI Sign-In (headless)".

`codemux connect` is the one-command remote bootstrap: sign in if needed,
persist web-remote + relay config, then either flip relay on a *running*
instance over the control socket (`web_remote_set_relay`) or — with nothing
running — install the `codemux.service` systemd **user** unit running
`codemux serve`, enable it now + at boot, and `loginctl enable-linger` so it
survives logout. `codemux connect status` reports account/config/unit/instance
state; `codemux connect off` stops and removes the unit and turns relay mode
off (staying signed in). See `docs/features/web-remote-access.md` §
"One-command bootstrap".

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
