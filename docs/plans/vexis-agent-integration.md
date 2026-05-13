# vexis-agent ↔ Codemux Integration — Research

- Purpose: Decide the integration shape between vexis-agent (Telegram-bridged brain runtime) and Codemux (ADE).
- Authority: Research notes; not a build plan.
- Read next: `docs/features/mcp-server.md`, `docs/reference/CONTROL.md`.

## 1. What each source actually does

**vexis-agent.** Python daemon that pipes Telegram messages through an agent CLI (claude-code by default; opencode optional). The brain ABC (`vexis_agent/core/brain/base.py`) is the contract: `respond` / `astream`, `spawn_aux`, `write_mcp_config`, plus session/transcript/healthcheck plumbing. Tools are reached two ways: (a) bash dispatch wrappers (`vexis-bg`, `vexis-browse`) that shell out and talk to the daemon's Unix socket at `$XDG_RUNTIME_DIR/vexis-agent/vexis-agent.sock`, or (b) MCP servers declared in `~/.vexis/mcp-servers.yaml` and propagated by `setup_wizard.write_all_mcp_configs()` into per-brain native files (`<workspace>/.mcp.json` for claude-code; merged into `opencode.json` `mcp:` block for opencode). Propagation is atomic (temp + rename), idempotent, replays on `vexis-agent mcp refresh`. `McpServerSpec(name, command, args, env)` is the canonical shape.

**Superset CLI.** `superset/cli/main.py` mounts subcommands via `pkgutil.walk_packages` over `superset/cli/`, so every per-feature file registers its own `@click.group()` automatically — adding a feature means dropping a file, not editing the entrypoint. Help text comes from docstrings; errors raise `click.ClickException` so Click renders them uniformly. No `--json` flag, no autocomplete sugar — Superset relies on Click defaults.

**Codemux today.** CLI is `clap`-derived `Subcommand`s in `src-tauri/src/cli.rs` (`Status`, `Browser`, `Memory`, `Index`, `Issue`, `Workspace`, `Notify`, `Handoff`, `Capabilities`, `Mcp`, plus a raw `Json` escape hatch). Every arm thin-wraps `send_control_request` → JSON-RPC over the Unix socket at `$XDG_RUNTIME_DIR/codemux.sock` (named pipe on Windows). The socket dispatch in `control.rs:dispatch_request` is the actual surface: ~25 commands. The MCP server (`mcp_server.rs`, 31 tools per `mcp_server.rs:1083`) re-delegates to the same socket. Auto-config writes Codemux's MCP entry into Claude Code's `.mcp.json` (controlled by `auto_mcp_config`).

## 2. MCP shape

The 31-tool inventory (`docs/features/mcp-server.md`; `mcp_server.rs:1083` pins it) already covers browser (Tier 1/2/3 + info + viewport), workspace (`workspace_list`, `workspace_info`, `workspace_create`), pane (`pane_list`, `pane_split_right`, `pane_split_down`), `notify`, and a 5-tool git block. From vexis-agent's perspective, **most of what we want already exists**. Genuine gaps for a brain controlling Codemux:

- `terminal_write` / `terminal_read` — send text to a pane, read scrollback. The socket exposes `write_terminal`; not yet an MCP tool.
- `workspace_open` / `workspace_switch` — focus an existing workspace (vs. only listing/creating).
- `agent_chat_start_session` / `agent_chat_send_turn` — the brain wants to spawn a Codemux agent-chat session and stream replies. Today these are Tauri commands only.
- `app_status` — wrap the existing `status` socket command (running workspaces, focused pane).
- `port_list` — surface detected dev-server ports (the data is already in `ports.rs`).

Tool naming should keep the `mcp__codemux__*` prefix the Anthropic SDK already enforces; input schemas should mirror the existing 31 (lowercase snake_case keys, `required` arrays, descriptive long-form `description`). Return shapes should stay JSON-object — vexis-agent's brains hand raw JSON back to the model.

## 3. CLI shape

The CLI is sufficient as a fallback; not as the primary integration. What's missing for shell-out use:

- No `--json` / `--quiet` flag — `pretty` JSON is always printed; agents parse fine but humans pay the cost both ways.
- No `terminal` / `pane` / `agent-chat` subcommands (the socket has the calls, CLI doesn't expose them).

No new CLI work is needed if we go MCP-first.

## 4. Recommendation: MCP-first, full stop

vexis-agent's native model is "drop one line in `~/.vexis/mcp-servers.yaml`, restart, the brain has the tools." Claude Code (the default brain) consumes MCP servers natively through `<workspace>/.mcp.json`. Codemux already ships `codemux mcp` as a stdio JSON-RPC server with 31 tools and an `auto_mcp_config` writer. The integration is **zero-code on the Codemux side and one config line on the vexis-agent side**:

```yaml
servers:
  - name: codemux
    command: codemux
    args: ["mcp"]
```

`vexis-agent mcp refresh` rewrites `.mcp.json` and the brain picks it up on next session. This beats CLI shell-out because (a) claude-code's tool-use loop handles MCP tools first-class (typed inputs, schema validation, permission prompts), (b) vexis-agent already has the propagation machinery, and (c) the existing Codemux MCP plan (Priority 3 in `.claude/skills/codemux-features/SKILL.md`) is already done — `docs/features/mcp-server.md` is the source of truth.

## 5. MVP scope (MCP-first) — two phases

**Phase 1 (MVP — low-friction, wraps existing socket paths).** Five new tools plus one atomic-write fix:

1. `terminal_write` (pane_id, text) — wraps the existing `write_terminal` socket command at `control.rs:610`.
2. `terminal_read` (pane_id, lines) — NEW socket command; mirrors `write_terminal`'s shape.
3. `workspace_open` (workspace_id) — NEW socket command (or extend `apply_preset`); focus an existing workspace.
4. `app_status` — wraps the existing `status` socket command at `control.rs:530` (running workspaces, focused pane, app version).
5. `port_list` (workspace_id) — surfaces detected dev-server ports using `PortInfo` from `ports.rs:122` (already serializable).
6. **Fix:** `upsert_mcp_config` at `mcp_server.rs:998` — switch from `std::fs::write` to atomic temp+rename+fsync. Recon §7 confirmed this is not atomic today; a crash mid-write leaves a half-written `.mcp.json` that brains read on the next session.

LOC budget: ~200 LOC in `mcp_server.rs` (5 new tools) + ~80 LOC in `control.rs` (2–3 new socket commands: `terminal_read`, `workspace_open`, possibly `app_status` depending on whether the existing `status` shape suffices) + ~20 LOC for the atomic-write fix.

**Phase 2 (deferred until agent-chat surface stabilizes).** Four agent-chat tools: `agent_chat_list_sessions`, `agent_chat_send_turn`, `agent_chat_list_messages`, `agent_chat_start_session`. Deferred because 12 substantive commits have landed since the feature shipped (recon §6) — OpenCode provider, capability harvest, Step 13 master toggle — and wrapping a moving target wastes effort. Compounding cost: agent-chat has no socket equivalent today, so MCP-wrapping means adding 4–5 new socket commands first. Revisit once the Tauri-command surface settles.

## 6. Patterns to adopt or avoid

**Adopt from vexis-agent.** The atomic-write fix above (Phase 1 #6) cribs directly from `_write_yaml` in `vexis_agent/daemon/mcp.py` — write to `path.with_suffix(path.suffix + ".tmp")`, then `tmp.replace(path)`. Beyond Phase 1: consider a `codemux mcp status` verb that diffs declared vs. on-disk, mirroring vexis-agent's `ServerStatus.fully_wired` check.

**Adopt from Superset.** `pkgutil.walk_packages` auto-mounting means new subcommands cost zero entrypoint edits. Codemux's `CommandSet` enum is starting to bloat (10+ variants); splitting per-feature `cli_*.rs` modules with a registration trait would mirror this in Rust idiom.

**Avoid.** Don't build a separate Codemux↔vexis-agent CLI bridge. MCP is the integration; the CLI is the local-dev escape hatch.

## 7. Open questions for Kaan

- Should `codemux mcp` honour `CODEMUX_WORKSPACE_ID` from the env (current behaviour) or accept an arg per call? vexis-agent spawns one MCP child per workspace today; multi-workspace would need re-think.
- Are HTTP/SSE MCP transports needed? vexis-agent is single-user/local-only — probably not, but worth confirming before Step 11's HTTP gateway lands.
- Per-server `allowed_tools` / `disallowed_tools` — vexis-agent's `mcp-servers.yaml` schema doesn't have this today; add it, or rely on Codemux's own permission flow?
- Does vexis-agent want a Codemux *adapter* (a `Brain` subclass that uses Codemux as its execution backend), or is MCP-as-tool-server enough? My read is MCP-only — vexis-agent stays the brain runtime, Codemux stays the ADE.
