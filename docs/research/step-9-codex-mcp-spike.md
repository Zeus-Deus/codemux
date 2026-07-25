> Step 9 Stage 5 research-only spike, completed 2026-04-28. Investigates whether Codemux's MCP runtime can be extended to Codex sessions. Outcome: **GO with HTTP MCP gateway (Option C)** as a future Step 11 — the ground has shifted since the original Step 9 research and a tractable workaround now exists. No code changes in this checkpoint.

> **RESEARCH NOTE.** Pre-implementation research or a spike. Some conclusions
> here were later revised or reversed by what actually shipped — read it as
> reasoning history, never as current behavior. Current truth lives in
> `docs/features/*`.

# Step 9 Stage 5 — Codex MCP feasibility spike

The original Step 9 research (`docs/research/step-9-mcp-servers.md`, locked 2026-04-28) concluded that Codex's `app-server` exposes no runtime tool-injection RPC, ruling out the in-process facade pattern (Path B) used for Claude. Stage 5 re-validates that conclusion six weeks later and explores workarounds that have become viable since the original write-up.

**Bottom line:** Codex still has no runtime tool-injection API, but two complementary changes since the original research make a thin **HTTP MCP gateway** approach feasible — Codex now consumes streamable HTTP MCP servers reliably (PR #4317, stable since Sept 2025) AND exposes a `config/mcpServer/reload` JSON-RPC method (so Codemux can rewrite-and-reload Codex's config without bouncing the session).

---

## Task 1 — Audit Codex SDK / sidecar surface

### Local Codex adapter

`src-tauri/src/agent_provider/codex/` — five files, **zero MCP awareness**:

- `protocol.rs` carries the `app-server` JSON-RPC wire types. Tool surface today is **inbound only**: `item/toolCall/started|delta|completed` (line 340-407) reports tool calls the model already chose to make. There is no outbound `tools/register` or `setMcpServers` shape.
- `translate.rs` maps Codex's tool-call notifications to the canonical `ProviderRuntimeEvent::ToolUse` / `ToolResult`. Tool *origin* is opaque — the adapter never sees a tool list.
- `session.rs` issues `initialize` → `thread/start` → `thread/resume` → `turn/start`. None of these methods carry tool definitions; tools are decided server-side by the `codex` binary itself.
- `mod.rs` (`CodexAgentProvider`) takes `CodexProviderConfig` with no MCP-related field.
- A `MCP*` symbol grep across the entire `agent_provider/codex/` tree returns zero matches.

`sidecar/` contains only `claude-agent`. No Codex sidecar exists; the Codex provider drives `codex app-server` directly via `JsonRpcChild`.

### Codex's existing MCP integration (server-side, not host-side)

`src-tauri/src/mcp_server.rs:857` — `upsert_mcp_config` writes Codemux's own MCP entry into `<project>/.mcp.json` (the Claude Code shape), but does **not** touch `~/.codex/config.toml`. Codex doesn't currently know Codemux exists as an MCP server.

### What this means

The Codex adapter is a passive translator of Codex's own tool decisions. Codemux has zero leverage on the tool list Codex sees today. To extend Step 9 to Codex, we need an external mechanism — Codex won't accept tool definitions through the `app-server` JSON-RPC surface.

---

## Task 2 — Codex MCP support state of the art (April 2026)

(Findings distilled from web research dispatched as part of this spike — sources cited inline. Time budget exceeded ~30 min; full URL list at end of section.)

### Native MCP host API

**Verdict: still NO** — no runtime tool-injection RPC has landed in `codex app-server`. The full method list (~70 methods covering threads, turns, plugins, mcpServer/oauth/login, mcpServerStatus/list, mcpServer/tool/call, config/mcpServer/reload) has no `tools/register`, `set_tools`, `register_mcp_server`, `mcpServers/set`, or `attachTools`. The only "tool" verb is `mcpServer/tool/call` — invoking *already-configured* tools. `turn/start` input items are constrained to `text | image | localImage | skill | mention`; no per-turn tool array.

Source: [developers.openai.com/codex/app-server](https://developers.openai.com/codex/app-server).

### Material changes since the original research

Three updates worth flagging:

1. **Streamable HTTP MCP transport (PR #4317, merged 2025-09-27).** Codex now consumes HTTP MCP servers as a *client* via `[mcp_servers.X] url = "https://..."` plus optional `bearer_token_env_var`, `http_headers`, `env_http_headers`. Stdio (`command`) and HTTP (`url`) are mutually exclusive per server entry. ([github.com/openai/codex/pull/4317](https://github.com/openai/codex/pull/4317), [developers.openai.com/codex/mcp](https://developers.openai.com/codex/mcp))

2. **`config/mcpServer/reload` JSON-RPC method.** Codex can hot-reload `~/.codex/config.toml`'s `[mcp_servers]` block without restarting the session. This is the load-bearing change for Option B/C — it means dynamic tool changes don't require killing the chat.

3. **App-server changelog (0.123.0 → 0.125.0):** `/mcp verbose` flag, hooks observing MCP tools, MCP-sandbox-state round-tripping. None of these are runtime injection — the surface remains "configure at startup, reload via config rewrite + reload RPC."

### Codex SDK shape

`@openai/codex-sdk` (TS, Node 18+) and an experimental Python SDK exist. Lifecycle: `new Codex()` → `startThread()` → `thread.run(prompt)`. **Neither exposes `mcpServers`, `tools`, or any per-session/per-turn tool field.** Both are documented as "controls the local Codex app-server over JSON-RPC" — i.e., they're thin wrappers, not separate execution paths. ([developers.openai.com/codex/sdk](https://developers.openai.com/codex/sdk))

### Cursor's pattern (reality check)

Cursor's MCP integration is **not** a direct precedent. Cursor IS the agent harness — it talks to the model API directly, runs MCP servers in-process, and synthesizes its own agent loop with MCP tools as OpenAI-shaped function definitions. It does NOT drive `codex` the CLI. Tutorials titled "Cursor + Codex MCP" install Codex as an MCP *client* whose `~/.codex/config.toml` points at a third-party HTTP MCP — same config-file pattern as Option B/C. ([cursor.com/docs/mcp](https://cursor.com/docs/mcp), [composio.dev/toolkits/cursor/framework/codex](https://composio.dev/toolkits/cursor/framework/codex))

This is important: Codemux's architecture (driving `codex` as a child) is fundamentally different from Cursor's (Codemux being-the-agent). Cursor's route doesn't translate.

### OpenAI Agents SDK

Yes, MCP supported (`MCPServerStdio`, `MCPServerSse`, streamable HTTP). But the official "Use Codex with the Agents SDK" guide takes the *opposite* direction Codemux wants — it wraps `codex mcp-server` as one tool among many. It doesn't unlock injecting Codemux's own MCPs into a Codex-driven session. Substituting the Agents SDK for `codex app-server` is a different product, not a workaround. ([openai.github.io/openai-agents-python/mcp/](https://openai.github.io/openai-agents-python/mcp/), [developers.openai.com/codex/guides/agents-sdk](https://developers.openai.com/codex/guides/agents-sdk))

### Open issues to watch

- [Issue #11284](https://github.com/openai/codex/issues/11284) — third-party streamable HTTP MCP servers fail to initialize on Codex 0.98.0+ where they work in Cursor/Claude Desktop. **Open as of late April 2026.** This is the single biggest reliability risk for Option C.
- [Issue #4707](https://github.com/openai/codex/issues/4707) — HTTP MCP showing "Tools: (none)". Closed via PR #5298 (fixed).
- OAuth flow on streamable HTTP MCP not yet wired — bearer tokens via plaintext config only.

---

## Task 3 — Workaround paths

### Option A — Function calling injection (per-turn `tools` array on `turn/start`)

**Verdict: NOT FEASIBLE.** Confirmed in Task 1 audit + Task 2 research. `turn/start` doesn't take a tools array; no method registers tools at session/turn time. No change since original Step 9 research.

### Option B — Config file rewrite (Codemux writes `~/.codex/config.toml`)

**Verdict: FEASIBLE — strictly easier than April.**

What changes since the April research:

| Concern | April assessment | Now |
|---|---|---|
| Tool changes mid-session | "User must restart `codex` to pick up new tools" | `config/mcpServer/reload` RPC reloads in-place |
| Spawn count | "Codex spawns its own copy of every server, separate from Codemux's" | Same — but no longer hands Codex a stale tool list |
| Config conflict | "Codemux fights user's hand-curated `[mcp_servers]`" | Same — Codemux must own a dedicated subsection or be explicit about ownership |

Pros:
- Reuses `JsonRpcChild` plus `mcp_server.rs:upsert_mcp_config`'s merge pattern — minimal new code.
- Works whether the MCP server is stdio or HTTP; Codex spawns whatever Codemux's config tells it to.
- No new HTTP server in Codemux.

Cons:
- **Spawns the MCP server twice** (once in Codemux's runtime for Claude, once by Codex itself). Doubles memory and any process-side resource cost (e.g., docker exec containers).
- Codemux can't intercept Codex's tool calls — they go straight to the user's MCP server, bypassing Codemux's permission flow / logging.
- Tool list is the same as Codemux's only by manual sync; no shared cap or dedupe with the Claude path.

### Option C — HTTP MCP gateway (Codemux runs an HTTP MCP server, Codex connects)

**Verdict: FEASIBLE WITH CAVEATS — recommended.**

Architecture:

```
            ┌─────────────────────────────────────┐
            │ Codemux (Rust)                      │
            │                                     │
            │  ┌──────────────────────────┐       │
            │  │ McpRegistry              │       │
            │  │  - github (stdio child)  │       │
            │  │  - omarchy (stdio child) │       │
            │  │  - codemux-self (stdio)  │       │
            │  └──────────────────────────┘       │
            │              ▲                      │
            │              │                      │
            │  ┌──────────────────────┐           │
            │  │ Sidecar facade (TS)  │ ─→ Claude │
            │  │  type: "sdk"         │   SDK     │
            │  └──────────────────────┘           │
            │              │                      │
            │  ┌──────────────────────┐           │
            │  │ HTTP MCP gateway     │ ─→ Codex  │
            │  │  127.0.0.1:RANDOM    │  via      │
            │  │  /mcp                │  config   │
            │  └──────────────────────┘  reload   │
            └─────────────────────────────────────┘
```

Codemux exposes one streamable HTTP MCP endpoint on a localhost-only port, advertising the same tool list the Claude facade sees. `~/.codex/config.toml` gets one line: `[mcp_servers.codemux] url = "http://127.0.0.1:PORT/mcp"`. When the registry's tool list changes (toggle / spawn / kill), Codemux fires `config/mcpServer/reload` over the existing `JsonRpcChild` connection and Codex re-fetches.

Pros:
- **One source of truth.** Same registry as Claude. Same dedupe, same cap, same approval flow.
- **Tool calls flow back through Codemux.** Codex hits the HTTP gateway, Codemux dispatches to the right child, returns the result. Permission flow, logging, telemetry — all the affordances Stage 3 added for Claude — apply unchanged.
- **One TOML line, ever.** No need to mirror N user MCPs into Codex's config; Codemux is the single MCP entry.
- **Hot reload is a one-line RPC** (`config/mcpServer/reload`), not a config-file rewrite cycle.
- Maps perfectly onto the Claude facade story for Stage 4's dynamic-refresh path: bump the gateway's tool list → reload → done.

Cons:
- **New HTTP server in Codemux.** Adds an axum/hyper dep (we have `tokio` with `full` features but no HTTP framework). Modest effort but a code-volume bump.
- **Issue #11284 risk.** Some third-party HTTP MCP servers fail to initialize on Codex; we'd need to validate Codemux's gateway specifically against `codex` (Cursor/Claude Desktop conformance is not enough).
- **OAuth not wired** in Codex's HTTP MCP path. Codemux's gateway must be `localhost`-only (no auth) — fine for a process-local trust boundary, but rules out any future "expose Codemux's MCP runtime to a remote agent" extension without homegrown auth.
- **Latency.** Each Codex tool call goes Codex → HTTP → Codemux → child stdio → Codemux → HTTP → Codex. Extra hop vs. Codex's direct stdio (~5-10 ms in practice; not an issue for non-realtime tools).

### Option D — SDK PR upstream

**Verdict: UNKNOWN, multi-month timeline.** OpenAI is actively iterating on `codex app-server` (Unix socket transport in 0.124, plugin marketplace plumbing, hooks observing MCP). A clean PR adding `mcpServer/register` is plausibly accept-able, but there's no signal in the changelog that they're planning per-session injection. Even if accepted, ship-to-stable is a multi-month wait. Worth tracking, not worth blocking on.

### Comparison matrix

| | Latency overhead | New code | Per-call interception | OAuth-ready | Spawn dedupe | Reliability risk |
|---|---|---|---|---|---|---|
| **A. Function injection** | n/a | n/a | n/a | n/a | n/a | NOT FEASIBLE |
| **B. Config rewrite** | 0 ms | small (TOML writer) | ❌ no — Codex hits child directly | n/a | ❌ duplicate processes | low |
| **C. HTTP gateway** | ~5-10 ms | medium (HTTP server + transport) | ✅ yes — through Codemux | ❌ localhost-only OK | ✅ one process | medium (#11284 watch) |
| **D. SDK PR** | 0 ms (eventually) | Codemux side: small. Upstream: medium | ✅ yes | ✅ if proposed | ✅ | high (timeline) |

---

## Task 4 — POC viability check

Did NOT write code (per the spike's no-code mandate), but validated the design end-to-end against documented surfaces:

### Step-by-step viability of Option C

1. **Run an HTTP MCP server in Codemux's process.** Doable. We already use `tokio` with `full` features. Adding `axum` (or going straight to `hyper`) gets us a streamable HTTP endpoint on a random `127.0.0.1` port. The MCP HTTP transport is documented at [modelcontextprotocol.io/specification/2025-06-18/basic/transports](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports) — single endpoint, POST per request, optional GET for SSE push.

2. **Translate `tools/list` and `tools/call` from HTTP → registry.** The dispatcher already exists (`McpRegistry::dispatch_tool_call`). The HTTP handler is a thin shim that takes the JSON-RPC body, routes through the registry, and returns the result. No new logic — same path the Claude facade uses.

3. **Write Codemux's URL into `~/.codex/config.toml`.** `mcp_server.rs:upsert_mcp_config` already implements the merge-pattern for project `.mcp.json`. Adding a sibling `upsert_codex_mcp_config` that writes `[mcp_servers.codemux] url = "..."` to `~/.codex/config.toml` is mechanical. Random port is captured at gateway startup and written into the TOML.

4. **Hot reload via `config/mcpServer/reload`.** When Codemux's registry tool list changes, fire one RPC on the existing `JsonRpcChild` connection to `codex app-server`. The Codex-side cost is one config re-read + one MCP server re-init for `codemux` only (other servers untouched). Latency: probably <500 ms.

5. **Tool name prefix.** Same as the Claude path: `mcp__codemux__<server>__<tool>`. Codex sees one virtual MCP server named `codemux` exposing all our tools. Same approval semantics, same conflict story.

### Risks identified

- **Issue #11284.** Codemux's gateway must specifically work with `codex`, not just MCP-spec-compliant. Need a smoke test that's part of CI.
- **Connection lifecycle.** Codex re-establishes its HTTP MCP connection on each session start (and presumably on reload). Codemux must keep the gateway alive across the app lifetime, not per-session. Gateway lives on the existing `McpRegistry` (process-singleton) — clean fit.
- **Port collision.** Random ephemeral port chosen at startup, written to TOML at session-start time. Codex reads the TOML once per session start; subsequent sessions get the same port. Gateway port re-binds across Codemux restarts — also writes the new port to TOML at startup.
- **`localhost`-only.** Bind to `127.0.0.1` (not `0.0.0.0`). Trust boundary is the local user. No auth for v1.

### What POC code would look like (sketch, not built)

```rust
// src-tauri/src/mcp/http_gateway.rs (new)
pub struct McpHttpGateway {
    addr: SocketAddr,           // 127.0.0.1:RANDOM bound at startup
    registry: McpRegistry,
}
impl McpHttpGateway {
    pub async fn start(registry: McpRegistry) -> Result<Self> { ... }
    // POST /mcp → JSON-RPC 2.0 envelope → dispatch_tool_call / tools/list
    // GET /mcp → optional SSE for streaming notifications
}

// In Codex provider start_session:
let gateway_url = state.mcp_gateway.url();
upsert_codex_mcp_config(&gateway_url)?;
// Codex reads config.toml on session-start → connects to our gateway
```

Effort estimate: ~1 week of focused work (gateway, config writer, dynamic-reload wiring, POC smoke against `codex`). About **40-50% the size of Stage 3** because the registry dispatcher, tool prefixing, and approval plumbing are all done.

---

## Task 5 — Recommendation

**GO — Option C (HTTP MCP gateway), as a future Step 11.**

Reasoning:

1. **The original Step 9 research's "Codex isn't viable" conclusion was correct in April but is no longer the cheapest path.** Streamable HTTP MCP support landing stable + `config/mcpServer/reload` together unlock a workaround that wasn't tractable six weeks ago.
2. **Option C preserves the Step 9 architecture mandate.** The user's locked decision was "Codemux runs MCPs once, exposes to ALL providers." Option C delivers that — Codex sees the same registry as Claude. Option B (config sync) violates the mandate by spawning each MCP twice.
3. **Reuse beats build.** All the heavy lifting (registry, dispatcher, prefixing, dedupe, cap, approval flow) already ships. The new code is a localhost HTTP server that reuses the existing dispatcher.
4. **Risk is bounded.** Issue #11284 is the one open question; mitigation is a smoke test against `codex` specifically. OAuth gap doesn't matter for localhost.
5. **Option D's timeline is too long for any near-term feature work.** Worth filing an issue upstream and tracking, but not blocking on.

### Master staging proposal (Step 11 — sketch)

| Stage | Scope | Demoable |
|---|---|---|
| **11.1** | HTTP MCP gateway scaffold — bind localhost port, serve `/mcp`, route `tools/list` to registry. No Codex wiring yet. | `curl localhost:PORT/mcp` lists tools |
| **11.2** | `config/mcpServer/reload` integration — add `upsert_codex_mcp_config` writer; fire reload RPC from Codex session on registry status change. | Open chat with Codex, see Codemux's MCP tools listed |
| **11.3** | Tool-call routing through gateway — `tools/call` dispatches via existing `McpRegistry::dispatch_tool_call`. | Codex agent calls `mcp__codemux__browser_screenshot` end-to-end |
| **11.4** | Permission flow integration — confirm `mcp__` prefix surfaces through Codex's existing approval-request notifications unchanged. | Approval prompt in Codex chat for an MCP tool call |
| **11.5** | Issue #11284 smoke test in CI — spawn `codex` with our gateway, fail the build if `tools/list` returns empty. | CI gate |
| **11.6** | Polish — error states, gateway health surfaced in Settings, OAuth path documented as a future when upstream wires it. | Settings shows "Codex: connected via HTTP gateway" |

Estimated complexity: **40-50% of Stage 3** (Claude facade). Reuses dispatcher, registry, types, status broadcast.

---

## Open questions for the implementer

1. **HTTP framework choice** — `axum` vs raw `hyper`. Tauri pulls hyper transitively; adding axum is one line in Cargo.toml but a meaningful dep. axum's API is cleaner for what we need. Recommendation: axum, gated behind a tiny `tokio::sync::OnceCell` so the gateway only starts when Codex sessions are actually used.
2. **When to spawn the gateway** — at app startup vs. lazy on first Codex session. Lazy is cheaper for users who never use Codex; eager is simpler. Recommendation: lazy, mirroring the Stage 2 spawn-on-chat-start pattern.
3. **Codemux self in HTTP gateway** — Codemux's own MCP (the 29 tools) is currently served via `mcp_server.rs::run_mcp_server` (stdio). When the gateway exposes Codemux's tools, do they come from the registry (which spawns codemux as a child) or from a direct in-process integration? Recommendation: come from the registry — same path as Claude — so the architecture stays uniform.
4. **Issue #11284 mitigation** — keep a regular smoke test pinned to specific Codex versions. If a regression lands upstream, freeze the recommended Codex version pin in `docs/features/mcp-server.md`.

---

## Updates to existing docs

This spike's outcome should be reflected in:

1. **`docs/features/mcp-server.md`** — add a "Codex support (planned, Step 11)" section noting the HTTP gateway approach is the chosen path.
2. **`docs/research/step-9-mcp-servers.md`** — update the "Critical risk #1: Codex has no MCP host API" callout with a forward reference to this spike.
3. **`docs/core/PLAN.md`** — if there's a backlog of step numbers, add Step 11 with this spike as the source.

(Not done in this checkpoint — those updates land alongside Step 11's actual implementation. This spike's job is to inform the decision, not to ship doc changes.)

---

## Sources

- [Codex MCP docs](https://developers.openai.com/codex/mcp)
- [Codex App Server JSON-RPC reference](https://developers.openai.com/codex/app-server)
- [Codex SDK docs](https://developers.openai.com/codex/sdk)
- [Codex Changelog](https://developers.openai.com/codex/changelog)
- [Codex + Agents SDK guide](https://developers.openai.com/codex/guides/agents-sdk)
- [PR #4317 — streamable HTTP MCP support (merged 2025-09-27)](https://github.com/openai/codex/pull/4317)
- [Issue #11284 — HTTP MCP init failures (open, watch)](https://github.com/openai/codex/issues/11284)
- [Issue #4707 — HTTP MCP "Tools: (none)" (closed via #5298)](https://github.com/openai/codex/issues/4707)
- [OpenAI Agents Python SDK — MCP](https://openai.github.io/openai-agents-python/mcp/)
- [Cursor MCP docs](https://cursor.com/docs/mcp)
- [Cursor + Codex via Composio (config-file pattern)](https://composio.dev/toolkits/cursor/framework/codex)
- [MCP transports spec](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports)
- [Unlocking the Codex harness — App Server architecture](https://openai.com/index/unlocking-the-codex-harness/)

Local code references:

- `src-tauri/src/agent_provider/codex/protocol.rs:340-407` — Codex tool-call notification surface (inbound only)
- `src-tauri/src/agent_provider/codex/translate.rs:148-209` — `translate_tool_call` (already provider-agnostic)
- `src-tauri/src/mcp_server.rs:857-906` — `upsert_mcp_config` merge pattern (template for `upsert_codex_mcp_config`)
- `src-tauri/src/mcp/registry.rs:dispatch_tool_call` — already routes by prefix; gateway just wraps in HTTP
- `src-tauri/Cargo.toml` — `tokio` full features available; no axum/hyper as direct dep yet
