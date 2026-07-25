# MCP Server

- Purpose: Describe Codemux's two-sided MCP integration — Codemux as an
  MCP **server** (exposing 55 control-plane tools to external agents)
  AND as an MCP **host** (running user-installed MCP servers and
  forwarding their tools into agent-chat sessions).
- Audience: Anyone working on agent integration, MCP tooling, or
  control-surface expansion.
- Authority: Canonical MCP feature doc.
- Update when: Tools are added/removed, transport changes, host runtime
  semantics change, or a new agent provider gains MCP support.
- Read next: `docs/reference/CONTROL.md`, `docs/features/agent-chat.md`,
  `AGENTS.md`

## What This Feature Is

Codemux occupies both sides of the Model Context Protocol:

1. **MCP server** (original role) — Codemux runs `codemux mcp`, a
   JSON-RPC 2.0 server over stdio that exposes 55 tools (browser,
   workspace, pane, git, notification, terminal, app status, ports,
   worktree create, presets, issues, automations) so external agents
   can drive Codemux programmatically.
2. **MCP host / client** (Step 9) — Codemux's agent-chat runtime
   discovers user-installed MCP servers from every supported
   provider's config files, spawns them as child processes, manages
   their lifecycle, and forwards their tools into agent sessions
   through an in-process facade.

The two sides share the same wire framing (JSON-RPC 2.0 over
newline-delimited stdio per the
[2024-11-05 spec](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports))
but live in separate modules: `src-tauri/src/mcp_server.rs` for the
server, `src-tauri/src/mcp/` plus `sidecar/claude-agent/src/mcp-bridge.ts`
for the host runtime.

## Server side: `codemux mcp` (desktop)

### Transport

- **Protocol**: JSON-RPC 2.0 over stdio (one JSON object per line)
- **Protocol version**: `2024-11-05`
- **Launch**: `codemux mcp` starts the server; agents connect via their
  MCP client config

> **Headless variant**: `codemux-remote mcp` is a sibling binary that
> ships a smaller 12-tool catalog (no panes, no browser) and forwards
> over HTTP+bearer to a local `codemux-remote serve` daemon instead of
> the desktop's Unix-socket control transport. Same wire protocol,
> different tool set, different control transport. The desktop's push
> flow auto-registers it into user-level agent configs on the host
> on every `serve` startup. `remote/mcp_register.rs` writes exactly two
> targets: `~/.claude.json`, and `~/.vexis/mcp-servers.yaml` when
> `~/.vexis` exists — **not** `~/.codex/config.toml` or `~/.cursor/mcp.json`. See `docs/features/remote-hosts.md` for
> the daemon, manifest, auth, and provisioning details, and
> `docs/plans/mcp-on-remote.md` for the design rationale and the
> deferred desktop-side steps (extract `codemux_core`, pull-workspace
> UI, `--host` CLI flag, migrate desktop transport to HTTP+manifest).

### Auto-Configuration

On startup, Codemux can automatically write its MCP server config into
`<workspace_dir>/.mcp.json` so agents discover it without manual setup.
That file is the only thing `upsert_mcp_config` writes — there is no
`claude_desktop_config.json` writer anywhere in the repo. This is controlled by the
`auto_mcp_config` setting (default: enabled). Users can toggle it in
Settings → Editor & Workflow → Agent.

### Tool Routing

Most tools delegate to the Codemux control transport (Unix socket at
`$XDG_RUNTIME_DIR/codemux.sock` on Linux/macOS, named pipe at
`\\.\pipe\codemux-{username}` on Windows), reusing the same Rust helper
implementations as the Tauri command layer and CLI. Git tools shell out
to `git` in the workspace directory. The workspace is resolved from
`CODEMUX_WORKSPACE_ID` env var.

### Tools (55)

The Phase 1 / 1.5 / 1.6 vexis-agent integration tools (terminal,
workspace lifecycle, ports, worktree, presets, issues) and the eight
`automation_*` tools are checked into the registry. A test guard in `mcp_server.rs`
(`assert_eq!(tools.len(), 55)`) pins the count — bump it whenever a tool is
added or removed.

| Category | Count | Tools |
|---|---|---|
| Browser — Tier 1: DOM-based | 7 | `browser_navigate`, `browser_snapshot`, `browser_accessibility_snapshot`, `browser_click`, `browser_fill`, `browser_screenshot`, `browser_console_logs` |
| Browser — Tier 2: CDP/Vision-based | 5 | `browser_click_at`, `browser_type_at`, `browser_scroll_at`, `browser_key_press`, `browser_drag` |
| Browser — Tier 3: OS-level Input | 2 | `browser_click_os`, `browser_type_os` |
| Browser — Info & Evaluation | 3 | `browser_get_styles`, `browser_wait`, `browser_evaluate` |
| Browser — Viewport | 2 | `browser_viewport`, `browser_viewport_presets` |
| Workspace | 8 | `workspace_list`, `workspace_info`, `workspace_create`, `workspace_open` (Phase 1), `workspace_close` (Phase 1.6), `workspace_archive`, `workspace_unarchive`, `workspace_archive_list` (see `docs/features/workspace-archive.md`; `workspace_close` refuses worktree deletion on protected roots and honors `force_delete` for the dirty-worktree guard) |
| Pane | 4 | `pane_list`, `pane_split_right`, `pane_split_down`, `pane_close` (Phase 1.6) |
| Notification | 1 | `notify` |
| Git | 5 | `git_status`, `git_diff`, `git_stage`, `git_commit`, `git_push` |
| Terminal | 2 | `terminal_write`, `terminal_read` (Phase 1) |
| App / runtime | 2 | `app_status`, `port_list` (Phase 1) |
| Worktree / presets | 3 | `worktree_create`, `preset_apply`, `preset_list` (Phase 1.5) |
| Issues | 3 | `issue_list`, `issue_get`, `issue_link_workspace` (Phase 1.6) |
| Automations | 8 | `automation_list`, `automation_get`, `automation_create`, `automation_update`, `automation_delete`, `automation_pause`, `automation_resume`, `automation_runs` |

## Host side: cross-provider MCP runtime (Step 9)

### Architecture

Per the locked decisions in `docs/research/step-9-mcp-servers.md` and the
implementation across Stages 1–6:

```
┌─────────────────────────────────────────────────┐
│ Codemux (Rust)                                   │
│                                                  │
│  ┌────────────────────────┐                      │
│  │ McpRegistry            │                      │
│  │  - codemux-self (stdio)│                      │
│  │  - omarchy-kb (stdio)  │ ← user config files  │
│  │  - other user MCPs…    │                      │
│  └─────────┬──────────────┘                      │
│            │ tools/list, tools/call              │
│            ▼                                     │
│  ┌──────────────────────────────────┐            │
│  │ Sidecar facade (TS, in-process)  │ ─────→ Claude SDK
│  │  Options.mcpServers["codemux"]   │            │
│  │  type: "sdk"                     │            │
│  └──────────────────────────────────┘            │
│                                                  │
│  ┌──────────────────────────────────┐            │
│  │ HTTP gateway (planned, Step 11)  │ ─────→ Codex CLI
│  │  127.0.0.1:RANDOM/mcp            │            │
│  └──────────────────────────────────┘            │
└─────────────────────────────────────────────────┘
```

Codemux runs each user-installed MCP server **once** (process-singleton
inside Codemux) and exposes their tools through a unified registry. The
Claude SDK consumes them via an in-process MCP server (the "facade").
Tool calls round-trip back to Codemux's runtime so all Stage 3 plumbing
— permission flow, logging, error wrapping — applies uniformly.

### Discovery

`McpRegistry::prime_for_chat` walks every supported config path and
parses the union, discovering servers users have configured anywhere.

| Path | Source enum | Notes |
|---|---|---|
| `~/.codemux/mcp.json` | `CodemuxUser` | Codemux's canonical user config — written by Stage 4 polish (file-based today; UI add/edit deferred) |
| `<project>/.codemux/mcp.json` | `CodemuxProject` | Project-scoped Codemux MCPs |
| `~/.claude.json` (top-level `mcpServers`) | `ClaudeUser` | The file Claude Code updates when running `claude mcp add --scope user` |
| `~/.claude.json` `projects.<path>.mcpServers` | `ClaudeLocal` | Per-user-per-project local scope |
| `<project>/.mcp.json` | `ClaudeProject` | Project-checked-in scope (also where Codemux's auto-write lives) |
| `~/.cursor/mcp.json` | `CursorUser` | Cursor's format = Anthropic's |
| `<project>/.cursor/mcp.json` | `CursorProject` |  |

Codemux's own MCP server (the 55-tool one above) is **always-on**: a
hardcoded entry in `codemux_self_config()` that's pinned to the top of
the registry and not user-toggleable.

### Dedupe + conflicts

Identical configs across multiple sources collapse to one row with a
merged `sources: Vec<McpConfigSource>`. Same name with different
configs (e.g. two versions of `omarchy-kb`) stay as two separate rows
with an inline source disambiguator. The Codemux entry never merges
with anything — it always stays its own row to preserve the always-on
badge.

Tool prefix is `mcp__<server>__<tool>` for every tool, matching the
Anthropic SDK convention so persisted approval rules in
`~/.claude/settings.json` work without a separate code path. The prefix
also prevents collisions between user-installed MCPs and Claude Code's
native MCPs (e.g., `mcp__claude_ai_Google_Drive_*` from claude.ai
managed connectors vs. `mcp__codemux__*` from our facade).

### Lifecycle

- **Lazy spawn.** Children are NOT started at app boot. They start when
  the user opens Settings → MCP Servers (which calls `primeMcpRuntime`)
  or when the first agent-chat session is started — whichever happens
  first. Stage 3's bug fix in `agent_chat_start_session` waits up to
  8 s for the prime to complete before snapshotting the tool list, so
  no tools-list races even on a fresh Codemux launch.
- **App-shutdown kill.** `tauri::RunEvent::Exit` calls
  `McpRegistry::shutdown_all`, which fans out parallel
  `JsonRpcChild::shutdown` calls (graceful EOF then SIGKILL with a 2 s
  budget per child).
- **No auto-restart.** A crashed server stays in `Errored` with the
  stderr tail captured for the Settings tooltip. The user restarts
  manually via the Restart button on errored rows.
- **Hot toggle.** Disabling a server in Settings or the `+` popup stops
  the child immediately and emits `update-mcp-tools` to live Claude
  sessions so the agent loses access without a chat restart.

### Tool registration with Claude SDK

`sidecar/claude-agent/src/mcp-bridge.ts` builds an in-process
`McpServer` instance via the Claude Agent SDK's `createSdkMcpServer`
helper. Each tool's handler issues a `mcp-tool-call` JSON-RPC request
back to the Rust side via the bidirectional protocol added in Stage 3
(`sidecar/claude-agent/src/upstream-rpc.ts`). Rust's `spawn_incoming_requests_task` in
`agent_provider/claude/session.rs` routes `mcp-tool-call` requests to
`McpRegistry::dispatch_tool_call`, which forwards `tools/call` to the
backing child and pipes the result back.

JSON-Schema → Zod conversion is handled by `jsonSchemaToZodShape` in
the bridge (object/string/number/boolean/array/enum/nested object;
unknown shapes fall back to `z.unknown()`). This is good enough for
Anthropic's first-party MCPs; pathological schemas degrade to permissive
input validation.

### Dynamic refresh (Stage 4)

`spawn_mcp_refresh_task` per Claude session subscribes to the
registry's `broadcast::Sender<McpServerRuntime>` and pushes
`update-mcp-tools` to the sidecar whenever a server transitions state.
Debounced 250 ms so toggling several servers in quick succession
collapses to one refresh. The sidecar calls `query.setMcpServers({
codemux: ... })` to swap the in-process facade in place — no chat
restart needed.

### 50-tool cap

`apply_tool_cap` in `mcp/runtime.rs` caps the agent-visible tool list
at 50 total. Codemux's tools are protected (always survive the cap);
user MCP tools fill the remaining slots. The Settings UI shows a
yellow banner above the server list when the cap engages, plus a
`capped` badge on rows whose tools were dropped.

### Permission flow

MCP tool calls flow through Step 6's existing `canUseTool` bridge in
the sidecar's `permissions.ts`. The `mcp__` prefix carries through
unchanged, so persisted approval rules (Allow Once / For This Project /
For All Projects) match what Anthropic's SDK writes natively. No new
approval-store code; the prefix IS the discriminator.

## Settings UI

Settings → Editor & Workflow → MCP Servers.

- **Codemux row**: pinned to top, "always on" badge, no toggle, shows
  "Running, 55 tools" once primed.
- **User MCP rows**: grouped by source (`Claude · User`, `Cursor · User`,
  `Codemux · User`, etc.) with inline `Switch` toggles, live status
  badges, and "View tools" hover-reveal that opens
  `McpToolModal`.
- **Status badges**: `discovered` / `starting…` / running tool count /
  `errored` (red dot, hover for stderr tail) / `stopped`. Slow start
  (`> 3 s` in starting state) surfaces "slow start — taking longer than
  usual."
- **Cap banner**: appears when `dropped_count > 0`; shows "X of N tools
  registered" and recommends disabling servers to reclaim slots.
- **Conflict UI**: deduped multi-source rows show "also: Cursor · User"
  inline; same-name-different-config rows show a disambiguator with
  hover tooltip.
- **Restart button**: appears on errored rows.

## `+` popup integration (Stage 4)

Composer's `+` (attach) popup gained an `MCP Servers…` entry that
pivots to a submenu listing every discovered server with inline
`Switch` toggles. The submenu shares the same zustand store as
Settings, so toggling from either place is consistent. A bottom row
"Open MCP Settings" jumps to the Settings page via a
`codemux:open-settings` window event.

## What Works Today

- Cross-provider MCP server runtime (Claude-side) — Stage 1–6 of Step 9.
- Codemux's own MCP server with 55 tools (the 52-tool automation-era inventory plus `workspace_archive`, `workspace_unarchive`, and `workspace_archive_list`).
- Discovery from Codemux / Claude / Cursor config paths with dedupe.
- Lazy spawn on first chat session start (or Settings panel mount,
  whichever first), bounded await so chat-start isn't slowed by a
  cold prime.
- Dynamic tool refresh — toggling a server mid-session updates the
  agent's tool list within ~250 ms via SDK `setMcpServers`.
- 50-tool cap with Codemux protection + UI banner.
- Permission system integration — MCP tool calls flow through the same
  Allow Once / For Project / For All Projects approval UI as native
  tools.
- Settings panel with status badges, toggles, restart, tool-list modal,
  cap banner, conflict UI.
- `+` popup MCP Servers submenu with inline toggles.
- Auto-config writer for Codemux's own server (the original role) into
  Claude Code's `.mcp.json` (controlled by `auto_mcp_config` setting).

## Current Constraints

- **Claude only.** The Claude SDK's `Options.mcpServers["codemux"]` /
  `type: "sdk"` mechanism is the path; Codex doesn't expose a runtime
  tool-injection API in `codex app-server`. Codex MCP is planned for
  Step 11 via an HTTP gateway approach — see
  `docs/research/step-9-codex-mcp-spike.md`.
- **Stdio transport only** for spawning user-installed MCPs. HTTP /
  SSE transports are parsed and surfaced in Settings (so users see
  their HTTP MCPs listed) but `start_mcp_server` rejects them with
  `"HTTP transport not supported in v1"` until Step 11 motivates
  building an HTTP MCP client.
- **No add/edit from UI.** Users add servers by editing
  `~/.codemux/mcp.json` (or any other supported config path) directly.
  In-app add/edit is deferred to Step 10+.
- **Auto-config writes to Claude-specific config files.** Other agent
  platforms need manual setup; this is the original-role behavior and
  unchanged by Step 9.
- **JSON-Schema → Zod converter is lossy.** Pathological schemas
  (`oneOf` discriminated unions, recursive `$ref`, exotic keywords)
  fall back to `z.unknown()`. The MCP server itself still validates
  upstream, so this is permissive sidecar-side validation, not a
  safety hole.

## Important Touch Points

### Server side (original role)

- `src-tauri/src/mcp_server.rs` — JSON-RPC server, 55-tool registry,
  `upsert_mcp_config` config writer
- `src-tauri/src/cli.rs` — `codemux mcp` CLI entry point
- `src-tauri/src/agent_browser.rs` — DOM snapshot script used by
  `browser_snapshot`
- `src-tauri/src/control.rs` — Socket control server that MCP tools
  delegate to
- `src/stores/settings-store.ts` — `auto_mcp_config` setting

### Host side (Step 9)

- `src-tauri/src/mcp/mod.rs` — `McpServerConfig`, `McpConfigSource`,
  `McpTransport`, `dedupe_servers`, `source_rank`
- `src-tauri/src/mcp/paths.rs` — Config-path enumeration across
  providers
- `src-tauri/src/mcp/parser.rs` — `parse_mcp_config_file` +
  `parse_claude_wrapped_config` (handles `~/.claude.json` multi-section
  format)
- `src-tauri/src/mcp/codemux_self.rs` — Hardcoded always-on entry
- `src-tauri/src/mcp/runtime.rs` — `start_mcp_server` (handshake),
  `apply_tool_cap`, `McpServerHandle`, `McpServerStatus`, `McpTool`
- `src-tauri/src/mcp/registry.rs` — `McpRegistry`, lazy spawn,
  shutdown_all, dispatch_tool_call, status broadcast
- `src-tauri/src/commands/mcp.rs` — Tauri commands:
  `list_mcp_servers`, `get_mcp_runtime_status`, `set_mcp_disabled_ids`,
  `prime_mcp_runtime`, `start_mcp_server_cmd`, `stop_mcp_server_cmd`,
  `restart_mcp_server_cmd`, `list_mcp_tools`,
  `list_mcp_tools_with_cap_info`, `list_mcp_tools_for_server`
- `src-tauri/src/agent_provider/claude/protocol.rs` —
  `METHOD_UPDATE_MCP_TOOLS`, `UpdateMcpToolsParams`, `McpToolEntry`
- `src-tauri/src/agent_provider/claude/session.rs` —
  `spawn_incoming_requests_task` (handles `mcp-tool-call` from
  sidecar), `spawn_mcp_refresh_task` (debounced dynamic refresh),
  `collect_mcp_tools` (snapshot at session start)
- `sidecar/claude-agent/src/rpc.ts` — `parseLine` discriminated union
  (incoming vs response)
- `sidecar/claude-agent/src/upstream-rpc.ts` — Outbound JSON-RPC for
  sidecar → Rust callbacks
- `sidecar/claude-agent/src/mcp-bridge.ts` — `buildCodemuxMcpServer`,
  `jsonSchemaToZodShape`, in-process facade
- `sidecar/claude-agent/src/methods/index.ts` — `start-session`
  validates `mcpTools[]`, `update-mcp-tools` dynamic refresh handler
- `sidecar/claude-agent/src/session.ts` — `buildQueryOptions` populates
  `Options.mcpServers["codemux"]`, `updateMcpTools` calls
  `query.setMcpServers`
- `src/stores/mcp-store.ts` — Zustand `disabledIds` with localStorage
  persist + backend sync
- `src/hooks/use-mcp-runtime.ts` — Tauri-event-driven runtime status
- `src/components/settings/mcp-section.tsx` — Settings UI: groups,
  rows, status badges, cap banner, conflict UI, slow-start indicator
- `src/components/settings/mcp-tool-modal.tsx` — Tool-list modal
- `src/components/chat/Composer.tsx` — `+` popup MCP submenu
  (`attachSubmode === "mcp"`) with inline `Switch` toggles
- `src/components/chat/SlashCommandPopup.tsx` — `rightAdornment`
  support for inline trailing controls

## Roadmap

- **Step 10 — Skills sync** (planned). Mirrors the Step 9 cross-provider
  pattern for skills. See `docs/features/agent-chat.md` follow-ups.
- **Step 11 — Codex MCP via HTTP gateway** (planned). Codemux exposes
  one localhost HTTP MCP server, writes `[mcp_servers.codemux] url =
  "..."` to `~/.codex/config.toml`, hot-reloads via
  `config/mcpServer/reload`. Reuses the entire Step 9 registry +
  dispatcher. See `docs/research/step-9-codex-mcp-spike.md` for the spike
  research and staging proposal.
