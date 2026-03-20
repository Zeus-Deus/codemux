# Control Protocol Reference

- Purpose: Canonical reference for Codemux CLI and socket control.
- Audience: Agents, scripts, and developers automating the app.
- Authority: Stable protocol and command reference.
- Update when: Command families, request shapes, or security assumptions change.
- Read next: `docs/features/browser.md`, `docs/features/openflow.md`, `AGENTS.md`

## Transport

- socket path: `$XDG_RUNTIME_DIR/codemux.sock`
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
```

## Browser Note

From agent terminals, always use explicit `codemux browser ...` subcommands. Do not use `xdg-open`, `open`, or any other system-browser launcher when the goal is to work inside Codemux.
