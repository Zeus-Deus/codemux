# Control Protocol

Codemux exposes a local JSON-over-Unix-socket control API.

## Socket

- path: `$XDG_RUNTIME_DIR/codemux.sock`
- protocol version: `1`
- transport: one JSON request per line, one JSON response per line

## Security model

Current model:

- local-user socket only
- same-machine control only
- intended for trusted local agent tools

Future hardening can add:

- auth token or capability checks
- per-command permission gates
- stronger session scoping

## Request format

```json
{"command":"status","params":{}}
```

## Response format

```json
{
  "ok": true,
  "protocol_version": 1,
  "data": {},
  "error": null
}
```

## Supported commands

- `status`
- `get_app_state`
- `create_workspace`
- `split_pane`
- `open_url`
- `notify`
- `write_terminal`
- `browser_automation`
- `get_project_memory`
- `update_project_memory`
- `add_project_memory_entry`
- `generate_handoff`
- `rebuild_index`
- `index_status`
- `search_index`

## CLI examples

```bash
codemux status
codemux notify "Agent needs approval"
codemux json get_app_state
codemux json split_pane '{"pane_id":"pane-1","direction":"horizontal"}'
codemux memory show
codemux memory add decision "We are keeping local-first memory"
codemux handoff
codemux index build
codemux index search "terminal session"
```

## Agent integration idea

External coding agents can use this control layer to:

- inspect current app/workspace state
- create workspaces and panes
- write into terminal sessions
- open URLs in browser panes
- trigger notifications
- run browser automation

This is the base layer for later integrations with OpenCode, Claude Code, Codex, and similar tools.
