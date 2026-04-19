# Agent Chat

- Purpose: Describe the current capability and constraints of the agent-chat
  subsystem.
- Audience: Anyone working in or around the chat panel feature area.
- Authority: Canonical feature-level reality doc.
- Update when: Behavior, constraints, expectations, or major touch points
  change.
- Read next: `docs/plans/agent-chat.md` (when it lands), related reference
  docs.

## What This Feature Is

Agent Chat is the planned GUI surface that will let users talk to CLI coding
agents (Claude Code, Codex) through a conversational pane instead of a raw
terminal. When complete it will share orchestration with the existing
openflow system but surface a streaming chat UX — messages, tool approvals,
file-diff previews — over the existing subprocess-backed runners.

## Current Model

**Scaffolding only.** No user-visible behaviour is wired up yet. The work
landed in this checkpoint is purely backend plumbing so future chunks can
stand on a stable contract:

- `src-tauri/src/agent_provider/` defines the `AgentProvider` trait plus the
  shared type vocabulary (`ProviderKind`, `ProviderSession`,
  `ProviderRuntimeEvent`, `ApprovalDecision`, `ProviderError`, ...) that
  every concrete provider adapter must speak.
- `src-tauri/src/json_rpc_child/` implements a reusable
  `JsonRpcChildProcess` helper for long-lived subprocesses that speak
  newline-delimited JSON-RPC 2.0 over stdio.
- `src-tauri/src/agent_provider/codex/` implements the first concrete
  provider — a `CodexAgentProvider` that drives the `codex app-server`
  subprocess through the shared JSON-RPC helper and translates its
  notifications into the canonical event stream.

None of this is reachable from Tauri commands or UI today. It sits dormant
until later tasks wire a chat pane and feature flag on top.

## What Works Today

- Stable trait/type surface for provider adapters.
- Reusable JSON-RPC-over-stdio child-process helper with timeout, graceful
  shutdown, bidirectional notifications, server-initiated requests, and
  child-exit cleanup.
- Codex provider end-to-end: spawn → init handshake → `thread/start` (with
  `thread/resume` fallback) → turn dispatch → streaming notifications and
  tool-approval requests → canonical event broadcast.
- Auth probes (`probe_installed`, `probe_authenticated`) that shell out
  to `codex --version` / `codex auth status` and classify the output.
- Integration tests (`src-tauri/tests/json_rpc_child.rs` and
  `src-tauri/tests/codex_adapter.rs`) covering the helper and the Codex
  adapter respectively. The Codex adapter tests are backed by a scripted
  fake fixture at `src-tauri/tests/helpers/fake_codex_app_server/main.rs`
  so no real `codex` binary is required.

## Current Constraints

- No user-visible chat panel yet.
- No Claude adapter yet; Claude integration will live in a sibling
  submodule once the SDK sidecar piece is designed.
- No Tauri commands exposed.
- No persistence, projection pipeline, or event sourcing.
- No permission / approval UX; approval events flow through the stream
  but nothing routes them to a human.
- The event broadcaster uses a bounded channel (default 1024) — slow
  subscribers lose old events. This is deliberate; downstream UI must
  treat the stream as live-only.

## Important Touch Points

- `src-tauri/src/agent_provider/mod.rs` — module re-exports.
- `src-tauri/src/agent_provider/types.rs` — newtypes, enums, input/output
  structs.
- `src-tauri/src/agent_provider/events.rs` — `ProviderRuntimeEvent` and its
  sub-enums.
- `src-tauri/src/agent_provider/errors.rs` — `ProviderError` plus
  `SerializableProviderError`.
- `src-tauri/src/agent_provider/provider.rs` — the `AgentProvider` trait
  itself.
- `src-tauri/src/agent_provider/codex/mod.rs` — `CodexAgentProvider`.
- `src-tauri/src/agent_provider/codex/protocol.rs` — wire-level types for
  the `codex app-server` JSON-RPC protocol.
- `src-tauri/src/agent_provider/codex/translate.rs` — pure translation
  functions from Codex notifications / server-initiated requests to
  `ProviderRuntimeEvent`.
- `src-tauri/src/agent_provider/codex/session.rs` — per-thread session
  state plus background tasks that forward events.
- `src-tauri/src/agent_provider/codex/auth.rs` — auth/installed probes.
- `src-tauri/src/json_rpc_child/mod.rs` — the `JsonRpcChild` helper.
- `src-tauri/tests/json_rpc_child.rs` — helper tests.
- `src-tauri/tests/codex_adapter.rs` — Codex adapter integration tests.
- `src-tauri/tests/helpers/fake_rpc_child/main.rs` — in-tree JSON-RPC peer
  used as the JsonRpcChild fixture.
- `src-tauri/tests/helpers/fake_codex_app_server/main.rs` — scripted
  fixture that impersonates the `codex app-server` subprocess.

## Notes

- Keep this file about current truth, not future plans. Future chunks (Claude
  adapter, Codex adapter, chat pane, orchestration wiring) will each own
  their own `docs/plans/` entry at time of work.

## Sidecar (claude-agent)

Location: `sidecar/claude-agent/`.

**Why it exists.** The Claude integration is built on a SDK that only
runs under a JavaScript runtime. Rather than reverse-engineer that
SDK's wire protocol and maintain a Rust port, we run it inside a tiny
TypeScript subprocess and talk to it over JSON-RPC. This is the
officially supported integration path, and it means Codemux's Rust
side stays provider-agnostic.

**Current state.** Scaffold only. The sidecar implements a single
`ping` method that echoes its params back with a server timestamp.
No SDK dependency yet — that lands in a follow-up task on top of a
known-good foundation.

**Toolchain.** [Bun](https://bun.sh) 1.3+ is the sole dependency. Bun
handles install, test, and `bun build --compile` to produce a
standalone binary per target platform.

**How to build locally.** From the repo root:

```sh
bash scripts/build-claude-sidecar.sh
```

or directly:

```sh
cd sidecar/claude-agent
bun install
bun run build:all
```

The per-target binary is staged at
`src-tauri/binaries/codemux-claude-sidecar-<target-triple>`, which is
where Tauri's `externalBin` bundling picks it up. The Rust integration
tests (`src-tauri/tests/sidecar_ping.rs`) look for the same path and
skip cleanly (with a build hint) if the binary is missing.

**How it's shipped.** `tauri.conf.json`'s `bundle.externalBin` array
includes `binaries/codemux-claude-sidecar`. Tauri's packager picks up
`binaries/codemux-claude-sidecar-<triple>` per target and embeds it
into the AppImage / deb / rpm / NSIS installer. The release workflow
(`.github/workflows/release.yml`) installs Bun and pre-stages the
binary so tauri-action finds it before bundling. CI
(`.github/workflows/ci.yml`) does the same, with a zero-byte
placeholder fallback for constrained runners.

**Protocol.** Newline-delimited JSON-RPC 2.0 over stdin/stdout. The
Rust side spawns the sidecar through
[`JsonRpcChild`](../../src-tauri/src/json_rpc_child/mod.rs) — the same
helper the Codex adapter uses — so adding new methods is a matter of
registering handlers in `sidecar/claude-agent/src/main.ts`.

## Claude Agent SDK integration

The sidecar (`sidecar/claude-agent/`) now hosts Anthropic's Claude
Agent SDK in-process. All Claude inference goes through the SDK's
`query()` — Codemux's Rust side never talks to Anthropic directly.

### ToS boundary

Three hard rules, enforced by `sidecar/claude-agent/scripts/check-tos-boundary.sh`
(run on every `bun test` and as a standalone CI step):

1. **No credential reads.** The sidecar must not open, stat, or
   reference `.claude.json`, `~/.anthropic/`, or any file path that
   could contain an OAuth token.
2. **No Anthropic HTTP requests.** The sidecar must not reference
   `api.anthropic.com` or `anthropic.com`. The SDK makes these requests
   itself; the sidecar is a transport only.
3. **No direct `claude` inference.** All inference runs through
   `@anthropic-ai/claude-agent-sdk`'s `query()`. The sole exception is
   `src/auth-probe.ts`, which is allow-listed for `claude --version`
   and `claude auth status` subprocess calls.

A fourth rule — no `ANTHROPIC_API_KEY` / `CLAUDE_CODE_OAUTH_TOKEN` env
reads — is enforced by the same check.

Run the check manually:

```sh
cd sidecar/claude-agent
bun run check-tos
```

### Session lifecycle

One `ClaudeSession` per chat thread, always in streaming-input mode.
The prompt argument to `query()` is an `AsyncPromptQueue<SDKUserMessage>`
that user-facing `send-turn` RPCs push into. A single background task
iterates the returned `Query` and forwards every message to the Rust
side as a `sdk-message` notification — no classification or
translation happens in the sidecar.

Two side-channel notifications are emitted in addition to the raw
`sdk-message` for the two tool uses production integrations special-case:

* `plan-proposed` — fires when an assistant `ExitPlanMode` tool_use
  block lands (and from the permission bridge before denying the tool).
* `user-input-requested` — fires when an assistant `AskUserQuestion`
  tool_use block lands.

### Exposed RPCs

| Method | Purpose |
|---|---|
| `start-session` | Spawn a new ClaudeSession. |
| `send-turn` | Queue a user message onto the session. |
| `interrupt` | Halt the current turn (`query.interrupt`). |
| `set-model` | Swap the session's default model. |
| `set-permission-mode` | Change the session's permission mode. |
| `respond-to-request` | Resolve a pending `canUseTool` approval. |
| `respond-to-user-input` | Answer an `AskUserQuestion` prompt. |
| `initialization-result` | Read the SDK's cached init payload. |
| `stop-session` | Close a session; idempotent. |
| `probe-installed` | Shell out to `<binary> --version`. |
| `probe-authenticated` | Shell out to `<binary> auth status`. |
| `ping` | Liveness probe from the scaffold. |

Deliberately NOT exposed (per the integration research): the ~16
other `Query` methods (`setMaxThinkingTokens`, `applyFlagSettings`,
`supportedCommands`, `supportedModels`, `supportedAgents`,
`mcpServerStatus`, `getContextUsage`, `reloadPlugins`, `accountInfo`,
`rewindFiles`, `seedReadState`, `reconnectMcpServer`,
`toggleMcpServer`, `setMcpServers`, `streamInput`, `stopTask`).
These ship as follow-ups only when UI calls for them.

### Options construction

Exactly 15 SDK `Options` fields are populated (`cwd`, `model`,
`pathToClaudeCodeExecutable`, `settingSources: ["user","project","local"]`,
`effort` (cast through `unknown` for forward-compat), `permissionMode`,
`allowDangerouslySkipPermissions` (only with `bypassPermissions`),
`settings` (when non-empty), `resume`, `sessionId`,
`includePartialMessages: true`, `canUseTool`, `env: process.env`,
`additionalDirectories` (when non-empty), `extraArgs` (when non-empty)).
The other 30+ fields in the SDK's Options surface are intentionally
left unset — they become features when the UI surfaces them.

### Testing

The sidecar ships with 38 Bun tests: 11 ping (scaffold), 18 session
unit tests using a `FakeQuery` injected via `setQueryFactoryForTests`,
and 9 permissions tests exercising the canUseTool bridge. The Rust
side has 7 end-to-end integration tests (`sidecar_sdk.rs`) that spawn
the compiled binary and cover the paths that don't require real
Anthropic auth. Testing a real session is a manual smoke test — it
requires a logged-in `claude` binary and live network egress, so it
lives outside CI.

## Claude adapter

`src-tauri/src/agent_provider/claude/` implements `AgentProvider` by
driving the claude-agent sidecar via `JsonRpcChild`. Mirror of the
Codex adapter's structure (`mod.rs`, `protocol.rs`, `translate.rs`,
`session.rs`, `auth.rs`) with one addition — `sidecar_path.rs`
resolves the bundled sidecar binary at runtime.

### Architecture

One sidecar subprocess per chat thread. Deliberately NOT multiplexed:

- Per-session memory isolation.
- One session's sidecar crashing doesn't affect the others.
- Simpler state model — nothing cross-session inside the sidecar.

The adapter spawns the bundled binary (at
`src-tauri/binaries/codemux-claude-sidecar-<triple>` or a path from
`CODEMUX_CLAUDE_SIDECAR_PATH`), sends `start-session` with the
user's claude binary path, then wires two background tasks: one
consuming the sidecar's notification broadcast, one consuming its
incoming-request mpsc (currently unused — the sidecar doesn't issue
server-initiated requests).

### Event translation

The sidecar forwards SDK messages opaquely as JSON. `translate.rs`
does all classification on the Rust side, which keeps the sidecar's
surface tiny and lets the Rust side evolve independently. Two paths:

- Sidecar-specific notifications (`session-configured`,
  `request-opened`, `plan-proposed`, `user-input-requested`,
  `session-ended`, `session-error`) map directly to trait events.
- SDK messages are structurally classified by `type` (and `subtype`
  for `system:*`). 15+ known shapes; unknown variants always surface
  as `RuntimeWarning` with the raw payload preserved. The
  notification task wraps translation in `catch_unwind` so a
  translation bug can't silently kill the event stream.

### Configuration

```rust
let provider = ClaudeAgentProvider::new(ClaudeProviderConfig {
    sidecar_binary: None,       // None => resolve at runtime
    claude_binary: None,        // None => "claude" on PATH
    event_channel_capacity: 1024,
}).await?;
```

`CODEMUX_CLAUDE_SIDECAR_PATH` overrides the search for testing and
manual override. The capacity default is 1024; smaller tests can use
less.

### Testing

27 integration tests in `src-tauri/tests/claude_adapter.rs` drive
the adapter against a `fake_claude_sidecar` binary that impersonates
the real sidecar's RPCs without involving the SDK. Script-driven
notifications let each test choreograph the exact event sequence.
Two additional tests use the REAL compiled sidecar for
`probe-installed` / `probe-authenticated` with a mock `claude`
binary.

43 unit tests in `translate.rs` / `protocol.rs` / `sidecar_path.rs`
cover SDK-message classification, notification mapping, approval
decision translation, and binary-path resolution.

### Dogfood smoke test

Run manually against a real `claude` CLI + real Anthropic auth:

```sh
# One-time: build the sidecar for the host platform.
bash scripts/build-claude-sidecar.sh

# Run the ignored dogfood test.
cargo test --manifest-path src-tauri/Cargo.toml \
  --test claude_adapter claude_real_session -- --ignored --nocapture
```

The test starts a session, sends "Say hi.", and asserts a content
delta arrives within 60 seconds. Never runs in CI.

## Known follow-ups

- **Recoverable thread-resume snippets.** The substring list in
  `agent_provider/codex/protocol.rs` (`RECOVERABLE_THREAD_RESUME_ERROR_SNIPPETS`)
  is inferred from an upstream reference and should be verified against
  real `codex app-server` error output. A mismatch degrades gracefully —
  the resume simply fails instead of falling back to a fresh start — but
  refining the list will give a nicer UX.
- **Turn-start parameter plumbing.** `CodexAgentProvider::send_turn`
  currently populates only the `model` field on the wire. The
  `TurnStartParams` struct already models `service_tier`, `effort`, and
  `collaboration_mode`; the adapter's public API needs matching
  overrides once the UI wants to expose them.
- **`JsonRpcChild::shutdown` is now `&self` and idempotent.** All
  callers can share the handle via `Arc<JsonRpcChild>` and invoke
  shutdown without ownership gymnastics. The first call runs the full
  EOF-then-kill sequence; subsequent calls short-circuit via an internal
  `AtomicBool` and return `Ok(())` immediately.
- **Image attachments in `send-turn`.** The sidecar RPC currently
  accepts only `text` and an optional `modelOverride`. When the UI
  needs multi-modal input, extend the RPC with an `images` array and
  build `SDKUserMessage.content` with `tool_result`-style image blocks.
- **Full AskUserQuestion UX.** The side-channel
  `user-input-requested` notification surfaces the questions, and
  `respond-to-user-input` accepts answers, but the translation to a
  richer UI shape ships with the real chat pane. The current
  implementation allows the SDK to continue with the given answers as
  `updatedInput`.
- **Unused SDK `Query` methods.** 16 methods are deliberately not
  exposed as RPCs (`setMaxThinkingTokens`, `applyFlagSettings`,
  `supportedCommands`, `supportedModels`, `supportedAgents`,
  `mcpServerStatus`, `getContextUsage`, `reloadPlugins`, `accountInfo`,
  `rewindFiles`, `seedReadState`, `reconnectMcpServer`,
  `toggleMcpServer`, `setMcpServers`, `streamInput`, `stopTask`). Add
  them piecemeal as UI features require.
- **Claude image attachments.** `ClaudeAgentProvider::send_turn`
  only forwards `text` and optional `modelOverride` to the sidecar's
  `send-turn`. The `SendTurnInput.images: Vec<ImageInput>` field
  exists on the trait but is currently ignored. Wire it when the UI
  needs multi-modal input.
- **Claude AskUserQuestion full flow.** The adapter surfaces
  `plan-proposed` and `user-input-requested` as
  `RequestOpened { request_kind: "plan" | "user-input" }`. Answering
  plan mode and filling in structured AskUserQuestion answers
  requires UI-side work plus `respond-to-user-input` RPC plumbing —
  the sidecar method is implemented, but nothing calls it yet.
- **Claude dogfood testing.** Before shipping the Claude provider,
  run the `claude_real_session` ignored test end-to-end on a
  developer machine with a logged-in `claude` CLI. The test covers
  a real content-delta round-trip. Add it to the release checklist
  for any user-facing Claude changes.
