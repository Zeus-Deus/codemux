# vexis-agent ↔ Codemux Integration — Research

> **RESEARCH NOTE.** Pre-implementation research or a spike. Some conclusions
> here were later revised or reversed by what actually shipped — read it as
> reasoning history, never as current behavior. Current truth lives in
> `docs/features/*`.

> **Status:** historical pre-implementation research. The Phase 1 / 1.5 / 1.6 work described as gaps below has landed, and the desktop MCP catalog is now 55 tools. Use `docs/features/mcp-server.md` and `docs/reference/CONTROL.md` for current truth; keep the counts below as the baseline that informed the shipped work.

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

**Phase 1.5 (delegation primitives — required before merge).** Phase 1 proved inspection works end-to-end via Telegram, but real-world testing (`docs/research/codemux-phase-1-5-research.md`) found the brain cannot *delegate actual work*: no MCP tool starts an agent in a workspace, no atomic worktree+workspace+agent+prompt flow exists, and `workspace_create` silently drops its `path` argument. Phase 1.5 closes that gap so the target use case ("spin up a worktree in openclaw and start a Claude Code agent there to research computer use") materializes in two MCP calls.

1. **`workspace_create` path-bug fix.** `control.rs:573` hardcodes `None` as `cwd` for `create_workspace_impl`, dropping the `path` the MCP schema advertises. Read `request.params.get("path")` and pass it through. ~3 LOC. Socket patch, not a new tool.
2. **`worktree_create` MCP tool.** Wraps `commands/workspace.rs:277-439::create_worktree_workspace` (Tauri-only today) via a new socket command. Inputs: `repo_path`, `branch`, `new_branch` (default `true`), `base` (default `"main"`), `layout` (`single`/`pair`/`quad`/`six`/`eight`/`shell_browser`/`empty`, default `"single"`), `initial_prompt`, `agent_preset_id`, `pr_number`. One atomic call does git worktree, workspace+layout, PTY spawn, setup scripts, `.mcp.json`, and preset launch with prompt injection via `branch_name::prepare_agent_command`. ~80 LOC.
3. **`preset_apply` MCP tool.** Thin wrap of `apply_preset` (`control.rs:591-610`). Inputs: `workspace_id`, `preset_id`, `override_mode` (`new_tab`/`split_pane`/`current_terminal`/`existing_panes`), `initial_prompt`. Attaches an agent to an existing workspace. ~25 LOC.
4. **`preset_list` MCP tool.** Wraps `get_presets` via a new socket command. Returns `[{preset_id, name, description, kind, is_default, commands_available}]` — `commands_available` reflects `command_binary_exists` (`commands/presets.rs:270-275`) so the brain knows which agents work on this host. ~30 LOC.

Acceptance criterion: the target prompt resolves in two MCP calls.

```
preset_list() → [{preset_id: "builtin-claude", commands_available: true}, ...]
worktree_create(repo_path: ".../openclaw", branch: "computer-use-research",
  new_branch: true, base: "main", layout: "single",
  agent_preset_id: "builtin-claude",
  initial_prompt: "research how they do computer use")
  → "workspace-NN"
```

The worktree lands on disk, the workspace hydrates, Claude Code launches with the prompt as a positional arg, the sidebar emits. Progress via `app_status` / `terminal_read`.

Tool count: **36 → 39**. LOC: ~140.

**Phase 2 (deferred until agent-chat surface stabilizes — independent of Phase 1.5).** Four agent-chat tools: `agent_chat_list_sessions`, `agent_chat_send_turn`, `agent_chat_list_messages`, `agent_chat_start_session`. Deferred because 12 substantive commits have landed since the feature shipped (recon §6) and wrapping a moving target wastes effort; MCP-wrapping also means adding 4–5 new socket commands first. Phase 1.5 does not depend on Phase 2 — `worktree_create` + `preset_apply` + `terminal_read` drive Claude Code without touching the agent-chat pane.

## 6. Patterns to adopt or avoid

**Adopt from vexis-agent.** The atomic-write fix above (Phase 1 #6) cribs directly from `_write_yaml` in `vexis_agent/daemon/mcp.py` — write to `path.with_suffix(path.suffix + ".tmp")`, then `tmp.replace(path)`. Beyond Phase 1: consider a `codemux mcp status` verb that diffs declared vs. on-disk, mirroring vexis-agent's `ServerStatus.fully_wired` check.

**Adopt from Superset.** `pkgutil.walk_packages` auto-mounting means new subcommands cost zero entrypoint edits. Codemux's `CommandSet` enum is starting to bloat (10+ variants); splitting per-feature `cli_*.rs` modules with a registration trait would mirror this in Rust idiom.

**Avoid.** Don't build a separate Codemux↔vexis-agent CLI bridge. MCP is the integration; the CLI is the local-dev escape hatch.

## 7. Open questions for Kaan

- Should `codemux mcp` honour `CODEMUX_WORKSPACE_ID` from the env (current behaviour) or accept an arg per call? vexis-agent spawns one MCP child per workspace today; multi-workspace would need re-think.
- Are HTTP/SSE MCP transports needed? vexis-agent is single-user/local-only — probably not, but worth confirming before Step 11's HTTP gateway lands.
- Per-server `allowed_tools` / `disallowed_tools` — vexis-agent's `mcp-servers.yaml` schema doesn't have this today; add it, or rely on Codemux's own permission flow?
- Does vexis-agent want a Codemux *adapter* (a `Brain` subclass that uses Codemux as its execution backend), or is MCP-as-tool-server enough? My read is MCP-only — vexis-agent stays the brain runtime, Codemux stays the ADE.
