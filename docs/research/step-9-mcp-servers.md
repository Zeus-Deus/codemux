> Step 9 implementation research, completed 2026-04-28. Locked design decisions plus concrete file:line evidence for the cross-provider MCP server runtime that follows Step 8 (attachments). No code changes in this checkpoint — research only.

> **RESEARCH NOTE.** Pre-implementation research or a spike. Some conclusions
> here were later revised or reversed by what actually shipped — read it as
> reasoning history, never as current behavior. Current truth lives in
> `docs/features/*`.

# Step 9 Research Deliverable — Cross-Provider MCP Servers

This document is the research deliverable for Step 9 of the agent-chat track. The work mirrors Step 7 (skills) in shape — a Settings section listing user-installed integrations with toggles, tool counts, and live status — but inverts Codemux's existing relationship with MCP. Today Codemux **is** an MCP server (`docs/features/mcp-server.md:9`); Step 9 makes it an MCP **host/client** that spawns user-installed servers and routes their tools into Claude and (eventually) Codex chat sessions.

The research was run as three parallel audits — a Codemux-internals reusability sweep, a protocol + cross-provider-injection brief, and a runtime/UI/conflicts design memo. Findings are reconciled below. Locked decisions are explicit; open questions sit at the end of each task and are summarised in the closing section.

---

## Task 1 — Existing Codemux MCP infrastructure audit

**Verdict:** plumbing is in much better shape than expected. The JSON-RPC child helper is production-ready and the existing `.mcp.json` writer is directly reusable for the inverse (read) direction. The only ground-up build is the in-memory tool registry plus the SDK facade.

### Codemux's own MCP server (server-side)

`src-tauri/src/mcp_server.rs` (1340 lines) implements a full MCP 2.0 server:

- JSON-RPC framing structs at `src-tauri/src/mcp_server.rs:20-70` (`JsonRpcRequest`, `JsonRpcResponse`, `JsonRpcError`) plus standard error codes (`PARSE_ERROR=-32700`, `METHOD_NOT_FOUND=-32601`, …) at lines 64-68.
- Protocol version constant `"2024-11-05"` at line 411.
- Single `dispatch()` async function at `src-tauri/src/mcp_server.rs:398-437` routing `initialize`, `tools/list`, `tools/call`, `ping`. Notifications (no `id`) return `None`; requests return `Some(JsonRpcResponse)`.
- 680-line `handle_tool_call()` match at `src-tauri/src/mcp_server.rs:443-699`.
- Stdio loop `run_mcp_server()` at `src-tauri/src/mcp_server.rs:793-829` reading newline-delimited JSON from stdin.
- Auto-config writer `upsert_mcp_config()` at `src-tauri/src/mcp_server.rs:857-906` merges Codemux's own server entry into the project `.mcp.json` while preserving existing siblings. The same merge code is reusable for **reading** other servers out of the same file.

CLI entry at `src-tauri/src/cli.rs:44` (`Mcp` variant). Auto-config gate at `src-tauri/src/mcp_server.rs:11` reads the `auto_mcp_config` setting.

**Reusability gap.** This module is server-side (it accepts incoming JSON-RPC). Step 9 needs a **client** that issues `initialize`/`tools/list`/`tools/call` to a child. The framing structs are reusable; the dispatch loop is the wrong shape and gets replaced by a different orchestrator.

### Existing JSON-RPC child-process helper

`src-tauri/src/json_rpc_child/mod.rs` (716 lines) is exactly the helper Step 9 needs. Surface:

- `JsonRpcChild::spawn(SpawnConfig)` at line 251 — pipes stdin/stdout/stderr, `kill_on_drop(true)`, env overlay, cwd, default timeout.
- `request(method, params)` at line 418, `request_with_timeout(...)` at line 424, `notify(method, params)` at line 475.
- `incoming_requests()` at line 490 — server-initiated requests routed through an mpsc receiver (taken once); `respond(id, result)` at line 500 supports both success and error responses.
- `notifications()` at line 524 — broadcast subscriber.
- `shutdown()` at line 536 — graceful EOF then kill, 2s budget (`GRACEFUL_SHUTDOWN_TIMEOUT` at line 41); the helper is now `&self` and idempotent (`docs/features/agent-chat.md:496-500`).
- Reader task at `src-tauri/src/json_rpc_child/mod.rs:251-336` already drains `BufReader<ChildStdout>` continuously — **stdio buffer management is already solved**.
- `STDERR_TAIL_CAPACITY = 8 KB` at line 38; `ChildExited { stderr_tail }` at lines 107-112 surfaces the last stderr to the consumer. Reuse this for status badges.

The helper supports concurrent in-flight requests by id, so a single shared child can fan out multiple chat threads' calls without queuing on the application side.

### Sidecar's relationship to MCP

The sidecar **deliberately does not forward MCP today**. No references to `mcpServers`, `setMcpServers`, `reconnectMcpServer`, or `toggleMcpServer` exist in `sidecar/claude-agent/src/`. `buildQueryOptions()` at `sidecar/claude-agent/src/session.ts:152-198` constructs the SDK `Options` object and never sets `mcpServers`. The 16 deliberately-unexposed SDK methods listed in `docs/features/agent-chat.md:227-232` include the MCP methods.

The fake-query test at `sidecar/claude-agent/test/fake-query.ts:196-202` includes stubs for the SDK's MCP methods, confirming the SDK supports them — they're just not wired through.

The permission bridge at `sidecar/claude-agent/src/permissions.ts:96-204` is **MCP-agnostic by accident**: `classifyToolKind()` at lines 59-87 heuristically buckets tools by name (`bash` → command, `read` → file-read, etc.) and everything else → `"other"`. MCP tools land in `"other"` today; an explicit `if (toolName.startsWith("mcp__")) return "mcp"` arm is the only change needed for richer approval cards.

### Codex's tool model

Codex's `app-server` JSON-RPC protocol exposes `initialize`, `thread/start`, `thread/resume`, `turn/start`, `turn/interrupt`, `thread/read`, `thread/rollback`, `account/read` plus inbound notifications/approval requests (`src-tauri/src/agent_provider/codex/protocol.rs`). **There is no `tools/register`, `setMcpServers`, or any runtime tool-injection method.** Tool calls arrive as `NotificationMessage::ToolCall { method, params }` server-sent (line 343, 407); `translate_tool_call()` at `src-tauri/src/agent_provider/codex/translate.rs:148-209` extracts `toolName`, `toolUseId`, `input`. Approval routing via `ProviderRuntimeEvent::RequestOpened` with kinds `command|file-read|file-change|other` (lines 243-256) is provider-agnostic.

Critically, Codex **does** read its own MCP config independently from `~/.codex/config.toml`'s `[mcp_servers]` table (see Task 3). Codex spawns those children itself. There is no documented surface for an external host to inject extra tools mid-session.

### `.mcp.json` discovery today

Today, Codemux **only writes** `.mcp.json` to register itself. There is no client-side MCP loader. `upsert_mcp_config()` is called from workspace lifecycle commands (`src-tauri/src/commands/workspace.rs:75, 126, 164, 338, 482`); removal at lines 533, 663. The settings flag `auto_mcp_config` at `src/stores/settings-store.ts:15` is a machine-local boolean toggle.

The format Codemux currently writes is the canonical Anthropic shape:

```json
{
  "mcpServers": {
    "codemux": {
      "command": "/abs/path/to/codemux",
      "args": ["mcp"],
      "env": { "CODEMUX_WORKSPACE_ID": "workspace-id" }
    }
  }
}
```

### Reusability matrix

| Subsystem | Step 9 fit | Status |
|---|---|---|
| `JsonRpcChild` helper (`src-tauri/src/json_rpc_child/mod.rs`) | Yes | **Reusable as-is.** Drives arbitrary MCP servers without modification. |
| MCP framing structs (`mcp_server.rs:20-70`) | Yes | **Reusable.** Lift into a shared `json_rpc` module so both server and client share types. |
| Server dispatch loop (`mcp_server.rs:398-437`) | Reference only | **Wrong direction.** Replaced by a client-side request orchestrator. |
| `.mcp.json` upsert / merge (`mcp_server.rs:857-906`) | Yes | **Reusable** for the read direction; the JSON-merge code is ergonomic and preserves siblings. |
| Sidecar `buildQueryOptions` (`session.ts:152-198`) | Yes | **Needs one new field** (`mcpServers`). Everything else stays. |
| `canUseTool` bridge (`permissions.ts:96-204`) | Yes | **MCP-agnostic already.** Only `classifyToolKind` adds an `"mcp"` arm. |
| Codex protocol/translate | No | **Codex has no tool-injection API.** See Task 4 critical risk. |
| `auto_mcp_config` setting | Partial | Stays as a local-only toggle; new MCP enable/disable state lives in a sibling store mirroring `skills-store.ts`. |

---

## Task 2 — MCP protocol fundamentals

Sources: [MCP Lifecycle](https://modelcontextprotocol.io/specification/2025-06-18/basic/lifecycle), [Transports](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports), [Tools](https://modelcontextprotocol.io/specification/2025-06-18/server/tools).

### Stdio framing

**Newline-delimited JSON-RPC 2.0** over stdio. Spec: "Messages are delimited by newlines, and **MUST NOT** contain embedded newlines." Stderr is free-form logs (this matches what Codemux's existing `mcp_server.rs:793-829` does, and what `JsonRpcChild` already expects). MCP does **not** use LSP-style `Content-Length:` headers — important to confirm because the helper would have rejected them.

### Lifecycle

1. `initialize` (request, client → server) — exchanges `protocolVersion`, `capabilities`, `clientInfo`/`serverInfo`.
2. `notifications/initialized` (notification, client → server) — client signals ready.
3. `tools/list` and `tools/call` — request/response pairs.
4. **No `shutdown` method.** Stdio shutdown is "close child stdin → wait → SIGTERM → SIGKILL." `JsonRpcChild::shutdown` already implements this exact pattern (`src-tauri/src/json_rpc_child/mod.rs:536` with `GRACEFUL_SHUTDOWN_TIMEOUT` 2s).

### Tool descriptor shape

Verbatim from the spec:

```json
{"jsonrpc":"2.0","id":1,"result":{"tools":[{
  "name":"get_weather",
  "title":"Weather Information Provider",
  "description":"Get current weather information for a location",
  "inputSchema":{
    "type":"object",
    "properties":{
      "location":{"type":"string","description":"City name or zip code"}
    },
    "required":["location"]
  }
}],"nextCursor":"next-page-cursor"}}
```

Optional fields: `title`, `outputSchema`, `annotations`. `nextCursor` is paginated tool listing — most servers fit in a single page but the cursor must be honored.

### Streamable HTTP transport

Single endpoint accepting POST (each request = one HTTP POST) and optionally GET (opens an SSE server-push stream). Sessions tracked via the `Mcp-Session-Id` header. Replaces the deprecated 2024-11-05 HTTP+SSE transport. **Codemux v1: stdio only.** HTTP is for hosted MCPs (GitHub, Linear, Sentry); defer until v1 ships.

### Capability families

Servers expose: `tools`, `resources` (readable URIs, subscribable), `prompts` (parameterized templates), `logging`, `completions`. Clients expose: `roots` (filesystem scope), `sampling` (server-asks-client-to-call-LLM), `elicitation` (server-asks-client for structured input). Cursor 3 surfaces `tools`, `resources`, `prompts`. **Codemux v1 advertises tools-only.**

### Capability negotiation

Each side advertises capabilities in `initialize`. The spec is firm: "Both parties MUST… only use capabilities that were successfully negotiated." Codemux declares an empty client capabilities object — no `sampling`, no `roots`, no `elicitation` — and skips the entire callback machinery for v1. This is an important defensive choice: it prevents misbehaving servers from issuing callbacks Codemux can't deliver.

### Notifications

`notifications/cancelled` and `notifications/progress` are SHOULD, not MUST. v1 emits `cancelled` on user-abort and ignores progress.

### How does the Claude SDK handle MCP tools today?

`Options.mcpServers: Record<string, McpServerConfig>` per the SDK typings (`sidecar/claude-agent/node_modules/@anthropic-ai/claude-agent-sdk/browser-sdk.d.ts:33`). The variants are:

```ts
type McpServerConfig =
  | { type?: "stdio"; command: string; args?: string[]; env?: Record<string, string> }
  | { type: "sse"; url: string; headers?: Record<string, string> }
  | { type: "http"; url: string; headers?: Record<string, string> }
  | { type: "sdk"; name: string; instance: McpServer }; // in-process
```

The `"sdk"` variant takes a live in-process `McpServer` instance built via `createSdkMcpServer({ name, version?, tools })`. **This is what makes Path B (below) tractable.**

---

## Task 3 — Config file formats per provider

### Claude Code project-scoped `.mcp.json`

(Claude Desktop's `claude_desktop_config.json` shares the `mcpServers` shape but lives at `~/Library/Application Support/Claude/claude_desktop_config.json`. Claude Code user-scope is `~/.claude.json` keyed under the project, not a standalone file.) Source: [code.claude.com/docs/en/mcp](https://code.claude.com/docs/en/mcp).

```json
{
  "mcpServers": {
    "github": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-github"],
      "env": { "GITHUB_TOKEN": "${GITHUB_TOKEN}" }
    },
    "linear": { "type": "http", "url": "https://mcp.linear.app/mcp" }
  }
}
```

### Cursor `~/.cursor/mcp.json`

Claude-compatible — same `mcpServers` key, same per-server shape. Source: [docs.cursor.com](https://cursor.com/docs).

```json
{ "mcpServers": { "fs": { "command": "mcp-fs", "args": ["--root", "/home/me"] } } }
```

### Codex `~/.codex/config.toml`

Source: [github.com/openai/codex/blob/main/docs/config.md](https://github.com/openai/codex/blob/main/docs/config.md).

```toml
[mcp_servers.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
supports_parallel_tool_calls = true
default_tools_approval_mode = "approve"
```

### OpenCode

Search result: OpenCode's config uses an `mcp` key (not `mcpServers`), schema defined in `packages/opencode/src/config/mcp.ts` upstream. Inner `ConfigMCP.Info` shape was not directly inspected by the research agents — **mark `Unknown — confirm with maintainer`**. v1 should defer OpenCode integration.

### Proposed unified internal representation

```rust
struct McpServerSpec {
    id: ServerId,           // user-visible alias from config: "github", "linear"
    transport: Transport,   // Stdio { command, args, env, cwd } | Http { url, headers } | Sse { url, headers }
    source: ConfigSource,   // Claude(Path) | Codex(Path) | Cursor(Path) | Codemux(Path)
    enabled: bool,          // user toggle, persisted in mcp-store
}
enum Transport {
    Stdio { command: String, args: Vec<String>, env: HashMap<String, String>, cwd: Option<PathBuf> },
    Http  { url: String, headers: HashMap<String, String> },
    Sse   { url: String, headers: HashMap<String, String> },
}
```

Each provider's config is parsed into this shape on load. Codemux's own unified config (open question — see closing section) lives at a path like `~/.codemux/mcp.json` or in `synced-settings`; merging strategy is "last config wins on alias conflict, with a Settings-surfaced collision warning."

---

## Task 4 — Cross-provider tool injection

### Claude path

Two architectural options for routing user-installed MCPs into Claude chats:

- **Path A — pure pass-through.** Codemux extends `SessionStartInput` to carry `mcpServers`, populates `Options.mcpServers` in `buildQueryOptions` (`sidecar/claude-agent/src/session.ts:152-198`), and lets the Claude SDK fork the children itself. One PR, simple. **But the SDK spawns one set of children per session, contradicting the "Codemux runs MCPs once, exposes to ALL providers" mandate.** Each open chat thread gets its own copy of every MCP server. Codemux cannot mediate, log, or share.
- **Path B — in-process facade.** Codemux owns the spawn (Rust side, via `JsonRpcChild`) and presents itself to the Claude SDK as a single in-process MCP server using the SDK's `type: "sdk"` server config (`Options.mcpServers["codemux"] = { type: "sdk", name: "codemux", instance: McpServer }`). The sidecar registers one in-process `McpServer` whose tool callbacks fan out to Rust over a new RPC. Each shared MCP child is spawned once per app lifetime; Claude sessions register only the facade; tool calls cross sidecar → Rust → real child.

**Lock Path B for v1.** Path A's compounding cost (one set of MCP children per chat session, plus zero sharing across providers) makes it the wrong shape for the user's mandate. Path B reuses `JsonRpcChild` for the real spawn and only adds the sidecar facade on top.

New code lives in:
- **Sidecar:** `sidecar/claude-agent/src/mcp-facade.ts` — one in-process `McpServer` registered as `Options.mcpServers["codemux"]`. Its `tools/list` and `tools/call` callbacks forward to Rust via two new RPC methods on the existing `JsonRpcChild` channel (`mcp/list-tools`, `mcp/call-tool`).
- **Rust:** `src-tauri/src/mcp_runtime/` (new module) housing `McpRegistry` plus the per-call dispatch. Reuses `JsonRpcChild` for every user-installed child.

Existing `canUseTool` bridge at `sidecar/claude-agent/src/permissions.ts:96-204` does not change. MCP tools surface with `mcp__<server>__<tool>` names (Task 6); add an `if (toolName.startsWith("mcp__")) return "mcp"` arm to `classifyToolKind` (lines 59-87) for richer approval cards.

### Codex path

**Critical risk.** Codex's `app-server` JSON-RPC protocol has no `setMcpServers`-equivalent. The 16 unexposed SDK methods called out at `docs/features/agent-chat.md:227-232` are Claude SDK methods, not Codex. Codex reads its own MCP config independently from `~/.codex/config.toml`'s `[mcp_servers]` table and spawns its own children.

Three options were evaluated:

- **(a) Codemux writes/munges `~/.codex/config.toml`** before spawning `codex app-server`, mirroring the same MCP commands the user wants shared. Codex spawns its own copy of each server. The "spawn once" mandate is impossible without upstream changes.
- **(b)** Wait for Codex to ship runtime tool injection. No such proposal as of 2026-04.
- **(c)** Intercept the Codex agent's text output and proxy tool calls. Too brittle — reject.

**Recommendation: defer Codex MCP to a research spike (Stage 5).** Ship Step 9 as Claude-only in Stages 1–4, surface a "Not available on Codex" badge per server in Settings, and time-box a 1-week investigation into Codex's actual surface before committing to Option (a). Option (a) is the likely landing spot but the "spawn once" goal will downgrade to "configured once, spawned twice."

### Cross-provider sharing — what's actually safe

The MCP spec models a session as logically related interactions between **one** client and **one** server, with state (auth tokens, working dir, per-session memory) scoped to that session. Streamable HTTP makes this explicit via `Mcp-Session-Id`. Stdio is implicitly one-session-per-subprocess.

**Routing two providers' tool calls through one MCP session merges their state.** Examples:

- Auth-token caches populated by Claude's first call get used by Codex's next call.
- A server with per-session `cwd` (filesystem MCPs) cannot serve two providers in different working directories.
- Subscribed `resources/listChanged` notifications arrive on one stream; Codemux must re-emit to both providers.

What's safe: **one server *process*, two MCP *sessions*** — for HTTP servers, two `Mcp-Session-Id`s against the same backend; for stdio servers, two child processes. For v1 Codemux runs one stdio child per server (Path B serves Claude only) so this is moot — but the moment Codex MCP lands (Stage 5) we will need to spawn two stdio children per shared server, not one.

---

## Task 5 — Lifecycle management

**Spawn timing.** Spawn enabled servers **lazily on first chat session start, then keep alive process-wide**. Concretely: when `agent_chat_start_session` runs, the registry checks for unspawned-but-enabled servers and brings them up before the provider session is handed back. Avoids paying the spawn cost at app startup for users who never open chat, while staying in the warm-cache pattern Cursor relies on.

*Rejected:* App startup (wastes resources). Per-thread (defeats pooling). On first tool registration mid-turn (creates a stall — bad UX).

**Kill timing.** **App shutdown only**, with one exception: a server toggled OFF in Settings is killed immediately. No idle timeout for v1.

*Rejected:* Idle timeout (premature optimization). Per-thread teardown (wipes registries built across sessions).

**Crash strategy.** **Mark dead, surface a red-dot status badge in Settings, do NOT auto-restart.** Restart loops mask config errors; a one-click "Restart" button per row is enough. Track exit code + last 8 KB of stderr (already provided by `JsonRpcChild`'s `STDERR_TAIL_CAPACITY` at `src-tauri/src/json_rpc_child/mod.rs:38` and surfaced via `ChildExited { stderr_tail }` at lines 107-112).

*Rejected:* Exponential backoff (debugging nightmare; users see flapping). Always restart (same problem).

**Pooling.** **One server instance shared across all chat threads of one provider.** This is the user's mandate within Path B for Claude. Across providers (Claude + Codex), see Task 4 — at the protocol level you want one session per provider, which means one stdio child per provider for stateful servers. For v1 (Claude-only) this collapses to one child per server.

**stdio buffer management.** Already solved. `JsonRpcChild` always launches a continuous reader task on `BufReader<ChildStdout>` (`src-tauri/src/json_rpc_child/mod.rs:251-336`). No new engineering.

**Hot reload.** Toggle in Settings = **immediate restart of that single server.** Off → graceful EOF then SIGKILL after 2s. On → spawn. Editing the underlying config file requires a manual "Refresh" — file-watching every MCP config across providers is scope creep.

*Rejected:* Wait-for-next-session (the user explicitly compares to Cursor where toggles take effect immediately).

---

## Task 6 — Tool registry

### Data shape

```rust
struct McpRegistry {
    servers: HashMap<ServerId, ServerHandle>,
    tools: HashMap<String /* prefixed name */, (ServerId, ToolDescriptor)>,
}
struct ServerHandle {
    id: ServerId,                              // alias from config
    spec: McpServerSpec,                       // transport + source
    state: Arc<RwLock<ServerState>>,           // Idle | Running | Error | Disabled
    child: Option<Arc<JsonRpcChild>>,          // None until lazy-spawn
    tools: Vec<ToolDescriptor>,                // raw, pre-prefix
    last_error: Option<String>,
    started_at: Option<Instant>,
}
```

Lives on `ProviderRegistry` so the existing event broadcaster (`docs/features/agent-chat.md:407-423`) can surface server-state changes on the same `agent_chat_event` Tauri channel.

### Persistence

Mirror `src/stores/skills-store.ts` exactly. The **server list itself is hydrated lazily from disk on every Settings mount** (forced refresh, see `src/stores/skills-store.ts:52-119`); only the **enable/disable preferences** persist via zustand `persist` middleware (`disabledServerIds` field in a new `mcp-store.ts`). `auto_mcp_config` already lives at `src/stores/settings-store.ts:15`; per-MCP toggles join it as machine-local state (NOT in synced settings — MCP installs reference local commands, so cloud sync is meaningless).

### Conflict resolution

**Always prefix tool names with `mcp__<server_id>__<tool_name>` at registration time.** Two MCPs exposing `create_issue` become `mcp__github__create_issue` and `mcp__linear__create_issue` — no collisions possible. Server-level alias collisions (two configs declaring the same server name) are rejected at config-load time with a Settings-surfaced error; user picks a different alias.

This prefix is the same convention the Anthropic SDK uses internally for MCP tools — it's load-bearing for Task 8 (permission-flow integration) because the prefix becomes the discriminator that prevents native and MCP tools from colliding in the approval store.

*Rejected:* Refuse-second (too rigid). User-pick prompt (interrupts startup).

### Tool count cap

**Cap at 50 tools** across enabled MCPs. Soft warning at 30. Each tool descriptor lands in every turn's system prompt; bloating that hurts the model's recall on first-party tools. Cursor's 40 is reasonable; Codemux is slightly more permissive because the agent already lives at a high count (29 native tools per `docs/features/mcp-server.md:29`).

When the user's enabled set exceeds 50, the Settings banner reads **"X tools over cap — last-loaded server's tools disabled until you trim."** Never silently truncate.

### Schema validation

Validate `tools/list` responses against the MCP tool-descriptor shape (`name` is a string, `description` is a string, `inputSchema` is an object) but **don't deep-validate `inputSchema` as JSON Schema.** A malformed schema is the model's problem — the server returns a bad tool, the model calls it, the server rejects, the user sees a tool-error in chat. Deep validation is expensive and would refuse legitimate-but-unusual schemas (recursive types, custom keywords). Security risk: a server can register a tool named e.g. `"rm -rf /"`; the prefix mitigation (above) plus Step 6's approval flow (Task 8) bound the blast radius.

---

## Task 7 — Tool call interception design

### Claude (Path B, in-process facade)

Where the new code lives:

- **Sidecar:** `sidecar/claude-agent/src/mcp-facade.ts` — one in-process `McpServer` registered in `Options.mcpServers["codemux"] = { type: "sdk", name: "codemux", instance }`. Its tool callback forwards to Rust via new RPCs (`mcp/list-tools`, `mcp/call-tool`) on the existing `JsonRpcChild` channel.
- **Rust:** `src-tauri/src/mcp_runtime/` — handles those RPCs by looking up the right `Arc<JsonRpcChild>` from the registry and forwarding `tools/list` / `tools/call`.

The existing send pipeline is **untouched** for native Claude tools. MCP tools go through the same `canUseTool` bridge (`sidecar/claude-agent/src/permissions.ts:96-204`) as any other SDK tool — the SDK doesn't distinguish in-process from remote MCP servers. The only sidecar change is one extra arm in `classifyToolKind` (line 59-87) for the `mcp__` prefix.

### Codex

Bluntly: **interception is not feasible at the JSON-RPC layer for v1.** Codex spawns its own MCP children from `~/.codex/config.toml`'s `[mcp_servers]` table. Codemux writing that file (Task 4 Option (a)) gets us the right tools available, but Codemux can't sit between Codex and the children. Tool calls land directly in the Codex-spawned process; approval flows through Codex's own `requestApproval` notification.

For v1 ship Claude-only and surface a "Codex MCP not available" badge in the Settings rows. Stage 5 is the research spike to confirm this finding and to investigate whether `[mcp_servers]` rewriting is acceptable UX.

---

## Task 8 — Permission/approval flow integration

The permission persistence layer is **the SDK's own settings files** — `~/.claude/settings.json`, `<project>/.claude/settings.json`, `<project>/.claude/settings.local.json` — read by `list_tool_permissions` at `src-tauri/src/commands/permissions.rs:26-51`. Decision key is `(scope, behavior, tool_name, rule_content?)` per `PermissionRule` at `src-tauri/src/commands/permissions.rs:8-17` and the SDK-shaped writer at `src/lib/agent-chat/permission-rules.ts:17-62`. **Codemux never persists approvals itself**; it builds `updatedPermissions` payloads and lets the SDK write them.

### Schema fit

**MCP tools fit unchanged** because of the `mcp__<server>__<tool>` prefix from Task 6. A user's "For All Projects" decision on `mcp__github__create_issue` writes a rule with `tool_name: "mcp__github__create_issue"` to `~/.claude/settings.json`.

The collision the orchestrator worried about — *"user said For All Projects to `read_file` for native tool, MCP server adds different `read_file`, approval carries over incorrectly"* — **cannot happen** because the MCP version is named `mcp__<server>__read_file`, never bare `read_file`. The prefix IS the discriminator.

### Codex side

Codex doesn't read `~/.claude/settings.json`. The existing Codex approval flow is per-session via `ApprovalDecision::AllowForSession` (`src-tauri/src/agent_provider/codex/protocol.rs:591-595`). Since v1 is Claude-only this is moot. When Codex MCP lands: write a parallel `~/.codex/permissions.json` maintained directly by Codemux (Codex has no equivalent surface), gated behind a Codex-only code path in `commands/permissions.rs`.

---

## Task 9 — Cross-provider tool name conflicts

Scenario: user has Claude configured to load MCP server `github` natively (via `~/.claude/claude_desktop_config.json`). Codemux **also** loads `github` via its unified config. Agent sees `mcp__github__create_issue` from two sources.

**Recommendation: prefix-only resolution + Settings-surfaced conflicts section.** Codemux always loads its servers as `mcp__<cmx_alias>__*`. Where the alias collides with a server already loaded by Claude's native config, the user sees a "Conflicts" subsection in Settings (mirror `ConflictsSection` at `src/components/settings/skills-section.tsx:257-316`) listing both and offering inline "Disable in Codemux" / "Open Claude config" affordances.

*Why not strip Claude's native config (Option 1 of the orchestrator's draft):* violates the principle that Codemux is a host, not the canonical config owner. Users edit `claude_desktop_config.json` for their own reasons; silent mutation is hostile.

*Why not skip-loading-on-collision (Option 2):* removes user agency over which copy wins.

*Why not warn-and-prompt (Option 3):* interrupts every startup.

The prefix already resolves the runtime collision (Task 6). The conflicts section is a UX courtesy.

---

## Task 10 — Settings UI design

Mirror `src/components/settings/skills-section.tsx:50-255` one-to-one.

**Header.** "MCP Servers" title + Refresh button (force-refetch tool list from each running server, bypass any TTL). Same `RotateCw` icon, same disabled-while-loading pattern (`src/components/settings/skills-section.tsx:165-178`).

**Codemux-native row.** Pinned-to-top, non-toggleable. Shows the built-in `codemux` MCP (29 tools, `docs/features/mcp-server.md:29`). Subtitle: "Always on — built-in workspace control." The existing `auto_mcp_config` toggle at `src/components/settings/settings-view.tsx:1071-1075` MOVES into this row's overflow menu as "Auto-write Claude config" — Step 9 is the right moment to consolidate.

**User MCP rows.** One per server. Layout:

- Left: alias + status dot (`idle` | `running` | `error` | `slow`).
- Middle: "12 tools" summary + first 3 tool names truncated.
- Right: `Switch` toggle + hover-reveal `View tools` / `Open config` / (no Restart in v1).

State source: `useMcpStore` (new zustand store, mirrors `useSkillsStore`). Rows hydrate from a `list_mcp_servers` Tauri command that reads configs and queries running children for live tool lists.

**View tools.** Modal, mirror `SkillViewModal` at `src/components/settings/skills-section.tsx:249-252`. Inline expander would balloon long lists (50-tool cap means real users hit 50). Modal shows tool name, description, JSON Schema collapsed by default.

**Open config.** Open in our built-in editor pane (CodeMirror, `docs/features/file-editor.md:9-22`), not VS Code — the file is JSON config the user wants side-by-side with the Settings panel. Emit a Tauri event that opens the file in a new editor tab in the active workspace.

**Restart-server button.** Skip in v1. Hot-reload (toggle off → on) accomplishes the same thing in two clicks and keeps the row clean. Reconsider if dogfood reveals users can't intuit the toggle pattern.

**Status semantics.**

- `idle`: enabled, not yet spawned (lazy-spawn).
- `running`: child alive, last `tools/list` succeeded.
- `error`: child exited or last `tools/list` errored — show stderr tail in tooltip.
- `slow`: `tools/list` initial response took **> 5 seconds**, OR any subsequent `tools/call` took > 5 s; clears after the next sub-5 s call. (>2 s would flag too aggressively for `npx`-spawned servers that pull deps on first run.)

---

## Task 11 — Master staging proposal

The orchestrator's 6-stage draft is roughly right, but two stages are not independently demoable. Revised:

### Stage 1 — Config parsing + read-only Settings UI (no spawn)

Parse `~/.claude/claude_desktop_config.json`, project `.mcp.json` files, `~/.codex/config.toml`'s `[mcp_servers]` block, and a new Codemux-native unified config. Surface them in Settings with toggles that flip a zustand value only. **Demoable:** "Settings shows my installed MCPs from all providers, dimmed because nothing is wired yet." `enable_mcp_runtime` feature flag stays OFF.

### Stage 2 — Backend runtime + tool registry, headless

`mcp_runtime` module spawns enabled servers, runs `initialize` + `tools/list`, populates `McpRegistry`. Verify via a debug Tauri command that returns the registry. Status indicators in the Settings UI light up. **Demoable:** "Toggle a server, see its tools listed, see status go green." No tool calls executed yet.

### Stage 3 — Claude SDK facade + interception

Sidecar `mcp-facade.ts` registers as `Options.mcpServers["codemux"]` with `type: "sdk"` and forwards to Rust. End-to-end: a chat with Claude can call `mcp__github__create_issue`; approval flows through the existing `canUseTool` bridge. **Demoable end-to-end. This stage flips the user-visible feature flag ON.**

### Stage 4 — Settings UI polish

"View tools" modal, conflicts section, slow-status detection, error tooltips, "Open config" in editor pane. Splits cleanly from Stage 1's read-only list because each affordance is additive. **Demoable** as a UX upgrade.

### Stage 5 — Codex injection (research spike, not a build)

Time-boxed 1-week investigation into Codex's tool registration story. Two outcomes:

- **(a)** Codex has a hidden surface — build the equivalent of Stage 3 for Codex.
- **(b)** Codex requires a fork or upstream wait — ship "MCP available on Claude only" with a roadmap link, plus the `~/.codex/config.toml` rewriter so users still get the same servers (separately spawned by Codex).

The orchestrator's Stage 5 ("cross-provider injection") implicitly assumed Codex would cooperate. It very likely won't, and that's the single largest delivery risk.

### Stage 6 — Polish

50-tool cap enforcement (warning + cap behavior), settings doc updates, telemetry on tool-call latency, dogfood smoke test mirroring `claude_real_session` pattern (`docs/features/agent-chat.md:325-338`).

### Complexity vs Step 7

**Larger.** Step 7 (skills) was a pure-read filesystem scanner with localStorage preferences and a slash popup — no subprocess, no IPC, no per-call interception. Step 9 introduces:

- A long-lived runtime (Stage 2).
- A sidecar protocol extension (Stage 3) that touches the Claude SDK's MCP surface.
- Permission-flow integration through three layers (Tauri command → sidecar → SDK).
- A hard research dependency on a third-party tool (Stage 5).

Estimate: **1.5–2× Step 7's effort.** Step 8 (attachments) was probably the closest comparable; Step 9 is bigger than Step 8 because of the runtime piece.

---

## Critical risks

1. **Codex has no MCP host API.** Stage 5 likely resolves to "Claude-only ships, Codex deferred / config-rewritten only." Half of "cross-provider" collapses for v1. **(Resolved 2026-04-28.)** The Stage 5 spike at `docs/research/step-9-codex-mcp-spike.md` reconfirmed that Codex still has no runtime tool-injection RPC, but identified two material changes since the original research: streamable HTTP MCP transport landed stable in Codex (PR #4317, Sept 2025) and `config/mcpServer/reload` lets Codemux hot-reload Codex's config without bouncing the session. Recommendation: Step 11 ships an HTTP MCP gateway in Codemux that Codex consumes as a single MCP entry, reusing the Step 9 registry. Estimated 40-50% of Stage 3's complexity.
2. **SDK in-process MCP server semantics may have undocumented edges.** Path B in Task 7 assumes the SDK's `Options.mcpServers["x"] = { type: "sdk", instance: McpServer }` accepts an in-process JS object whose tool callbacks forward async to Rust. The `browser-sdk.d.ts` types confirm the surface but real-world behavior on tool errors, concurrent calls, and large tool lists needs Stage 3 dogfooding before locking the architecture. Fallback: spawn a tiny stdio shim binary per Claude session that proxies to Rust over a unix socket — workable but adds a binary to the build.
3. **Tool count bloat hits the model's instruction-following.** Even with the 50-cap, 29 native + ~20 MCP tools is a lot. Plan dogfooding on a real chat with at least 3 MCPs enabled before flipping the user-visible flag in Stage 3.
4. **Pooling the MCP child across chat threads serializes calls within one server.** A slow `mcp__playwright__navigate` blocks every other thread's playwright call. Acceptable for v1 but add a per-server in-flight gauge to telemetry from day one.
5. **Hot-reload race during an active turn.** User toggles a server off mid-turn while a `tools/call` is in flight. Must drain pending calls before SIGKILL, or the SDK sees a torn-off result and may stall the turn. `JsonRpcChild::shutdown` already does graceful-EOF-then-kill (`src-tauri/src/json_rpc_child/mod.rs:536`); test this path explicitly.
6. **Cross-provider state-merge if Codex MCP eventually lands.** Per Task 4, sharing one stdio MCP child across Claude and Codex sessions merges per-session state (auth tokens, cwd). v1 ducks this because Codex MCP is deferred; if Stage 5 outcome (a) lands, we will need one stdio child per provider, not one shared.

## Open questions for the user

1. **Is "Claude-only v1" acceptable** if Stage 5 confirms Codex can't host custom MCPs, or do we hold the entire feature until Codex parity ships?
2. **Where should the unified Codemux MCP config live** — `~/.codemux/mcp.json` (machine-local), an `mcp_servers` block in synced settings (cloud-shared), or piggyback on `~/.claude/claude_desktop_config.json` (which `mcp_server.rs:887-906` already mutates)? Recommendation: machine-local file; cloud sync is meaningless for paths that reference local binaries.
3. **Should disabled MCPs still show their tool list** in Settings (read-only), or hide it until enabled? Skills shows greyed-out rows when disabled (`src/stores/skills-store.ts:128-131`); recommend mirroring.
4. **Approval-card display name:** users see `mcp__github__create_issue` raw, or do we strip the `mcp__github__` prefix in display while keeping it in the persisted rule? Affects Step 6 UI's `displayName` derivation.
5. **Cap policy:** is 50 tools right? Tighter (Cursor's 40) reduces context bloat but rejects more user installs.
6. **Resources and prompts (MCP capability families beyond tools):** in scope for v1 or defer? Recommendation: defer. Tools-only keeps the surface bounded.
7. **HTTP / SSE transports:** stdio-only for v1 or include HTTP for hosted MCPs (Linear, Sentry, etc.)? Recommendation: stdio-only for v1; hosted MCPs are coming but the test surface is much larger.
8. **OpenCode scope:** v1 or v2? If v1, we need to confirm `ConfigMCP.Info` shape upstream — research agents marked it as `Unknown — confirm with maintainer`.

---

## Notes

- This document is the locked Step 9 research deliverable. Stage-by-stage implementation plans land in dedicated `docs/plans/` entries when the work begins.
- Update `docs/features/mcp-server.md` when Step 9's first stage flips the feature flag, since that doc currently describes Codemux only as an MCP server (not a host). The host story is additive.
- The 1500/2500-word caps applied to source research agents kept findings tight; this consolidated deliverable preserves the concrete file:line references and reconciles two minor disagreements (Path A vs B for Claude — Path B locked; pooling semantics — confirmed safe for Claude-only v1, flagged as risk for Codex extension).
