# Codemux Control Surfaces — Historical Baseline (May 2026)

> **Status:** historical code audit captured before the Phase 1 / 1.5 / 1.6, Automations, and workspace-archive MCP expansions. Counts, line numbers, and gaps below intentionally describe that baseline and are not current truth. The live desktop catalog is pinned at **55 tools** in `src-tauri/src/mcp_server.rs`; use `docs/features/mcp-server.md` and `docs/reference/CONTROL.md` for current behavior.

Read-only audit feeding `docs/research/vexis-agent-integration.md`. Citations are
`path:line`. Where the plan doc contradicts code I tag it `DOC SAYS X / CODE
SAYS Y`.

## 1. MCP tool inventory

**Source of truth**: `src-tauri/src/mcp_server.rs:80` — `fn register_tools()`.
**Total: 31 tools** (not 29). The test guard at `mcp_server.rs:1083`
explicitly asserts the count and explains the bump:

```rust
// Tool count bumped from 29 → 31 when the mobile/desktop
// viewport tools (browser_viewport, browser_viewport_presets)
// were added.
assert_eq!(tools.len(), 31);
```

**DOC SAYS** 29-tool inventory (integration doc §2). **CODE SAYS** 31.

| # | name | line | purpose | required params |
|---|---|---|---|---|
| 1 | `browser_navigate` | 84 | navigate browser pane to URL | `url` |
| 2 | `browser_snapshot` | 95 | interactive DOM elements + CSS selectors | — |
| 3 | `browser_accessibility_snapshot` | 103 | a11y tree w/ `@e4` refs | — |
| 4 | `browser_click` | 111 | click by ref or CSS selector | `selector` |
| 5 | `browser_fill` | 122 | type into input | `selector`, `value` |
| 6 | `browser_screenshot` | 134 | base64 PNG + viewport dims | — |
| 7 | `browser_console_logs` | 142 | console output | — |
| 8 | `browser_click_at` | 151 | CDP click at coordinates | `x`, `y` |
| 9 | `browser_type_at` | 164 | CDP type at coordinates | `text` |
| 10 | `browser_scroll_at` | 177 | CDP mouse-wheel scroll | `x`, `y` |
| 11 | `browser_key_press` | 191 | CDP key combo | `key` |
| 12 | `browser_drag` | 202 | CDP drag from→to | `start_x`,`start_y`,`end_x`,`end_y` |
| 13 | `browser_click_os` | 215 | ydotool kernel-input click | `x`, `y` |
| 14 | `browser_type_os` | 227 | ydotool type | `text` |
| 15 | `browser_get_styles` | 241 | computed CSS for element | `selector` |
| 16 | `browser_wait` | 252 | wait for selector or text | — (one of) |
| 17 | `browser_evaluate` | 263 | JS eval | `script` |
| 18 | `browser_viewport` | 274 | resize viewport (mobile/tablet/desktop/WxH/reset) | `preset` |
| 19 | `browser_viewport_presets` | 313 | list presets | — |
| 20 | `workspace_list` | 323 | list workspaces | — |
| 21 | `workspace_info` | 331 | active workspace details | — |
| 22 | `workspace_create` | 339 | create workspace at optional path | — |
| 23 | `pane_list` | 350 | panes in active workspace | — |
| 24 | `pane_split_right` | 358 | split vertically | — |
| 25 | `pane_split_down` | 368 | split horizontally | — |
| 26 | `notify` | 379 | push notification to user panel | `message` |
| 27 | `git_status` | 392 | porcelain status | — |
| 28 | `git_diff` | 400 | diff (per-file or all) | — |
| 29 | `git_stage` | 410 | `git add <file>` | `file` |
| 30 | `git_commit` | 421 | commit staged | `message` |
| 31 | `git_push` | 432 | push | — |

## 2. Socket dispatch surface

**Source**: `src-tauri/src/control.rs:528` — `async fn dispatch_request(...)`.
21 commands, dispatched in one big `match request.command.as_str()`:

| # | name | line | calls |
|---|---|---|---|
| 1 | `status` | 530 | inline (returns socket_path + protocol_version) |
| 2 | `get_app_state` | 534 | `AppStateStore::snapshot()` |
| 3 | `create_workspace` | 538 | `commands::workspace::create_workspace_impl` |
| 4 | `split_pane` | 544 | `commands::workspace::split_pane_impl` |
| 5 | `apply_preset` | 556 | `commands::presets::apply_preset` |
| 6 | `create_browser_pane` | 576 | `commands::browser::create_browser_pane_impl` |
| 7 | `open_url` | 588 | `commands::browser::browser_open_url_impl` |
| 8 | `notify` | 600 | `AppStateStore::add_notification` |
| 9 | `write_terminal` | 610 | `terminal::write_to_pty` |
| 10 | `browser_automation` | 618 | dispatches to `stream_input::handle_vision_action` (Tier 2), `os_input::handle_os_action` (Tier 3), or `AgentBrowserManager::run_command` (Tier 1) |
| 11 | `get_project_memory` | 799 | `memory::get_project_memory` |
| 12 | `update_project_memory` | 806 | `memory::update_project_memory` |
| 13 | `add_project_memory_entry` | 820 | `memory::add_memory_entry` |
| 14 | `generate_handoff` | 857 | `memory::generate_handoff_packet` |
| 15 | `rebuild_index` | 864 | `indexing::rebuild_index` |
| 16 | `index_status` | 882 | `indexing::ProjectIndexStore::status` |
| 17 | `search_index` | 886 | `indexing::search_index` |
| 18 | `list_github_issues` | 897 | `github::list_github_issues` |
| 19 | `get_github_issue` | 904 | `github::get_github_issue` |
| 20 | `link_workspace_issue` | 914 | `github::get_github_issue` + `link_workspace_issue` |
| 21 | `rerun_setup` | 949 | `scripts::run_setup_scripts` |

Unknown commands fall through to `Err(format!("Unknown control command: {}", request.command))` (line 972).

## 3. Gap analysis — MCP vs socket

**Socket commands with NO MCP wrapper** (i.e. only reachable via `codemux json
<cmd>` or CLI subcommand):

- `status`, `get_app_state` (the latter is used *internally* by `workspace_list` / `workspace_info` / `pane_list` but is not its own MCP tool)
- `apply_preset`
- `create_browser_pane`, `open_url` (overlap with `browser_automation` action `open`)
- **`write_terminal`** — DOC §2 + §5 say this; **VERIFIED**. `control.rs:610` exists, `mcp_server.rs` has no `terminal_write` / `terminal_read` tool.
- `get_project_memory`, `update_project_memory`, `add_project_memory_entry`
- `generate_handoff`
- `rebuild_index`, `index_status`, `search_index`
- `list_github_issues`, `get_github_issue`, `link_workspace_issue`
- `rerun_setup`

**MCP tools with no socket equivalent** (don't go through `dispatch_request`):

- All 5 git tools (`git_status` / `git_diff` / `git_stage` / `git_commit` / `git_push`) — they shell out to `git` directly via `run_git()` at `mcp_server.rs:874`, never touching the socket.
- `browser_viewport_presets` — returns static data from `browser_viewport::list_presets()` at `mcp_server.rs:684`, no socket call.

Everything else under `mcp_server.rs:501` routes via `call_socket(...)` to
either `browser_automation`, `get_app_state`, `create_workspace`, `split_pane`,
or `notify`.

## 4. Capabilities vs MCP registry "drift"

**DOC SAYS** (§3): "Capabilities JSON drifts from the actual MCP tool registry
(two source-of-truths)."

**CODE SAYS** these are **not the same thing**. The Capabilities JSON
(`cli.rs:566`–`630`) catalogs **CLI subcommands** — not MCP tools. It has no
overlap key-by-key with `register_tools()`. The genuine drift is between
`Capabilities` JSON and the `BrowserCommand` enum (`cli.rs:57`):

| Surface | Browser subcommands listed |
|---|---|
| `BrowserCommand` enum (cli.rs:57) | Create, Open, Snapshot, Click, Fill, Screenshot, ConsoleLogs, **ClickAt, TypeAt, ScrollAt, KeyPress, Drag, ClickOs, TypeOs**, Viewport, ViewportPresets — **16** |
| `Capabilities` JSON (cli.rs:572) | open, snapshot, click, fill, screenshot, console-logs, create, viewport, viewport-presets — **9** |

The 7 coordinate / OS-input subcommands (`click-at`, `type-at`, `scroll-at`,
`key-press`, `drag`, `click-os`, `type-os`) are implemented as real CLI verbs
but missing from the Capabilities JSON. Likewise `json` and `mcp` top-level
subcommands aren't in Capabilities.

Net: the drift is real, but it lives **CLI ↔ Capabilities**, not
**MCP ↔ Capabilities**. The plan doc framing is misleading.

## 5. CommandSet enum status

**Source**: `cli.rs:13`. **12 variants** — doc says "10+", which is accurate
but undersells it:

```rust
pub enum CommandSet {
    App,                                      // 1
    Status,                                   // 2
    Json { command, params },                 // 3
    Notify { message },                       // 4
    Handoff,                                  // 5
    Memory { command: MemoryCommand },        // 6
    Index { command: IndexCommand },          // 7
    Browser { command: BrowserCommand },      // 8
    Issue { command: IssueCommand },          // 9
    Workspace { command: WorkspaceCommand },  // 10
    Capabilities,                             // 11
    Mcp,                                      // 12
}
```

Dispatch is a single `match cli.command { ... }` inside
`maybe_run_cli()` at `cli.rs:160`–`633`. Each arm is hand-written, calls
`send_control_request(...)`, then `println!` the response. The bloat is real:
the function is ~475 lines and every new subcommand adds another arm. Several
arms duplicate the same `unwrap_response` boilerplate (lines 362, 377, 389,
401, 408, 415, ...).

## 6. Agent-chat backend surface

**Feature flag**: `enable_agent_chat` lives in `FeatureFlags` at
`src-tauri/src/observability.rs:47`. Default is **off**
(`observability.rs:266`, `enable_agent_chat: false`). Every Tauri command
below early-returns `"feature_disabled: enable_agent_chat is off"`
(`commands/agent_chat.rs:49`) when the flag is off.

Registered Tauri handlers (`lib.rs:1218`–`1235`, `commands/agent_chat.rs`):

| # | command | signature | line | what it does |
|---|---|---|---|---|
| 1 | `set_agent_chat_beta` | `enabled: bool` | commands/mod.rs:253 | flip the Beta master toggle |
| 2 | `agent_chat_create_pane` | `workspace_id, provider?, cwd?, launch_mode?` | 166 | create `agent_chat` pane in workspace |
| 3 | `agent_chat_close_pane` | `pane_id` | 188 | idempotent close; reaps session |
| 4 | `dev_agent_chat_spawn_test_pane` | — | 220 | debug-only |
| 5 | `agent_chat_start_session` | `pane_id, provider, input: StartSessionInput` | 261 | prime MCPs, start provider session, persist row |
| 6 | `agent_chat_send_turn` | `provider, input: SendTurnInput` | 357 | queue user turn |
| 7 | `agent_chat_interrupt_turn` | `provider, thread_id, turn_id?` | 422 | cancel running turn |
| 8 | `agent_chat_respond_to_request` | `provider, thread_id, request_id, decision` | 440 | answer approval/tool prompt |
| 9 | `agent_chat_set_model` | `provider, thread_id, model?` | 461 | swap model |
| 10 | `agent_chat_set_permission_mode` | `provider, thread_id, mode` | 482 | accept-edits/plan/etc. |
| 11 | `list_chat_provider_capabilities` | `provider` | 527 | model/feature catalog |
| 12 | `agent_chat_stop_session` | `provider, thread_id` | 577 | terminate session |
| 13 | `agent_chat_list_sessions` | `workspace_id, cwd?, limit?` | 637 | history dropdown |
| 14 | `agent_chat_rename_session` | `thread_id, title` | 650 | rename row |
| 15 | `agent_chat_delete_session` | `thread_id` | 667 | delete row |
| 16 | `agent_chat_list_messages` | `thread_id` | 688 | replay transcript |

Events ride on a Tauri channel `agent_chat_event` (`commands/agent_chat.rs:33`,
`spawn_event_bridge` at line 710). There is **no socket equivalent** — these
are Tauri-only IPC. An external MCP tool wrapping them would need to either
proxy through the desktop process or move them to the socket first.

**Stability**: the surface is still moving. Recent commits touching
`commands/agent_chat.rs` (`git log -- src-tauri/src/commands/agent_chat.rs`,
last 16):

```
5e0bec1 feat(merge-resolver,settings): labeled merge CTA + shared model picker
80eed40 fix(stability): stop leaking agent-chat sidecars on workspace/tab/pane close
34af999 feat(agent-chat): Step 13 master Beta Features toggle
0238161 feat(agent-chat,codex): wire-protocol parity + dynamic capability harvest — Step 12 Stage 9
4752e90 feat(agent-chat,opencode): runtime AgentProvider adapter — Step 12 Stage 8
0dcf05b feat(agent-chat,opencode): add OpenCode as third provider — Step 12 Stages 1-3
cb541dc fix(agent-chat,presets,changes-panel): pane-scoped chats, new-tab preset launch, base-branch picker
7bb1c77 feat(agent-chat): step 10 — end-to-end encrypted skills sync
c0ce2eb feat(agent-chat): persist + replay transcripts on session resume
1e913de feat(agent-chat): session history selector + draft surface chrome polish
48b7809 feat(agent-chat): Home landing + capability-driven composer pickers
ab1027c feat(agent-chat): real chat pane UI with streaming, approvals, and permission-mode restart
```

That's 12 substantive commits in this branch's recent history, including
Step 12 Stage 1→8→9 and a Step 13 master toggle. The provider list itself
grew from Claude+Codex to Claude+Codex+OpenCode in commits 0dcf05b → 4752e90
→ 0238161. Wrapping this in an external MCP tool today would chase a moving
target.

**DOC SAYS** (§2 gaps) "`agent_chat_start_session` / `agent_chat_send` — the
brain wants to spawn a Codemux agent-chat session and stream replies. Today
these are Tauri commands only." **VERIFIED** — but the actual command name
is `agent_chat_send_turn`, not `agent_chat_send`.

## 7. auto_mcp_config writer

**Source**: `mcp_server.rs:961` — `pub fn upsert_mcp_config(workspace_dir,
workspace_id)`. Companion at line 1017 — `remove_mcp_config(workspace_dir)`.

What it writes (`mcp_server.rs:942`):

```rust
fn codemux_mcp_entry(workspace_id: &str) -> Value {
    let command = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "codemux".to_string());
    json!({
        "command": command,
        "args": ["mcp"],
        "env": { "CODEMUX_WORKSPACE_ID": workspace_id }
    })
}
```

So the `.mcp.json` entry under key `mcpServers.codemux` is `command =
absolute path to current binary`, `args = ["mcp"]`, `env =
{CODEMUX_WORKSPACE_ID: <ws-id>}`. After writing, the function calls
`crate::git::ensure_git_exclude(workspace_dir, ".mcp.json")` (line 1009).

**Atomicity**: **NOT atomic.** It's `std::fs::write(&mcp_path, &json)` at
`mcp_server.rs:998` — direct overwrite, no temp+rename. A crash mid-write
truncates the file. (Contrast with the vexis-agent pattern the plan doc
praises in §6: "atomic temp+rename, replace-in-place idempotency".)

**Idempotency**: yes for content — re-running with the same `workspace_id`
produces an identical JSON payload, and the test
`upsert_mcp_config` (`mcp_server.rs:1343-1344`) calls it twice in a row to
assert that. Existing `mcpServers` entries from other tools (shadcn etc.)
are preserved (lines 991-994). Invalid JSON aborts the write with a stderr
log (lines 968-975) rather than overwriting.

**Call sites** (`grep upsert_mcp_config`):

- `lib.rs:258` — at startup, for every workspace, when `is_auto_mcp_enabled` is true (setting `auto_mcp_config != "false"`, default true; see `mcp_server.rs:8`)
- `commands/workspace.rs:81, 132, 170, 344, 488` — on workspace create / open / worktree create / branch switch
- `commands/workspace.rs:539, 731` — `remove_mcp_config` on close

## 8. `codemux mcp` command behavior

**Source**: `cli.rs:562`:

```rust
Some(CommandSet::Mcp) => {
    crate::mcp_server::run_mcp_server().await?;
    Ok(true)
}
```

No args, no flags. Reads JSON-RPC requests line-delimited from stdin,
writes responses to stdout, logs to stderr (`mcp_server.rs:897` —
`pub async fn run_mcp_server`). Stdio transport only — no HTTP/SSE.

**Workspace scoping**: per-process via the `CODEMUX_WORKSPACE_ID` env var.
Read at `mcp_server.rs:499`:

```rust
let workspace_id = std::env::var("CODEMUX_WORKSPACE_ID").unwrap_or_default();
```

…and again at `mcp_server.rs:852` for `get_workspace_cwd` (used by git tools
to pick the right cwd). The auto-config writer at `mcp_server.rs:949` embeds
one workspace's id into the `env` block of the `.mcp.json` entry it writes,
so each workspace's `.mcp.json` pins one MCP child process to that
workspace. There is **no per-call workspace argument** — all tool calls in a
given stdio session share whatever `CODEMUX_WORKSPACE_ID` was set when the
child was spawned. The plan doc's open question §7 ("Should `codemux mcp`
honour `CODEMUX_WORKSPACE_ID` from the env (current behaviour) or accept an
arg per call?") accurately describes this.

## Ports surface (for a future `port_list` MCP tool)

`src-tauri/src/ports.rs:122`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortInfo {
    pub port: u16,
    pub pid: u32,
    pub process_name: String,
    pub workspace_id: Option<String>,
    pub label: Option<String>,
}
```

`pub fn detect_listening_ports() -> Vec<PortInfo>` at line 150. Linux:
parses `/proc/net/tcp` + `/proc/net/tcp6`, resolves PIDs via
`/proc/*/fd/`. Windows: `netstat -ano` + `tasklist /NH /FO csv`. Other
platforms return empty. Codemux's internal ranges (`3900-4199`, `>=9222`)
are filtered out via `is_codemux_internal_port` (line 118). `workspace_id`
and `label` come from the static `StaticPortsConfig` (line 131) — so a
`port_list` MCP tool would return `Vec<PortInfo>` directly, no shape work
needed.
