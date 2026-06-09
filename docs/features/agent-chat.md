# Agent Chat

- Purpose: Describe the current capability and constraints of the agent-chat
  subsystem.
- Audience: Anyone working in or around the chat panel feature area.
- Authority: Canonical feature-level reality doc.
- Update when: Behavior, constraints, expectations, or major touch points
  change.
- Read next: `docs/features/multi-provider-chat.md`,
  `docs/features/skills-sync.md`, `docs/features/mcp-server.md`,
  `docs/plans/step-8-attachments.md`,
  `docs/plans/step-13-beta-toggle-research.md`.

## What This Feature Is

Agent Chat is the in-app GUI surface that lets users talk to CLI coding
agents (Claude Code, Codex, OpenCode) through a conversational pane instead
of a raw terminal. It surfaces a streaming chat UX — messages, tool
approvals, plan proposals, AskUserQuestion panels, image and file
attachments, slash commands, mode pills — over subprocess-backed runners.

## Beta Gate (Step 13)

The feature is **OFF by default**. Two persisted feature flags gate the
entire Step 6–12 surface:

- `enable_agent_chat` — gates the chat pane kind, its Tauri command surface,
  the provider registry (Claude + Codex + OpenCode), and the MCP host
  runtime.
- `enable_lazy_workspace_creation` — gates the lazy-workspace path: sidebar
  `+` and boot-into-Home open a client-side chat draft instead of eagerly
  materialising a workspace; the draft is promoted on first message send.

Both flags default to `false`. The Settings → Beta Features section flips
them together (see `src/components/settings/beta-features-section.tsx`).
Turning Beta off triggers a plain-quit (no auto-restart) to keep user data
intact across the legacy/Beta UI swap. The legacy main-branch experience
(preset bar, terminal panes, empty-state splash) is byte-identical when
both flags are off.

See `docs/plans/step-13-beta-toggle-research.md` for the toggle scoping and
`docs/plans/step-13-ui-smoke-checklist.md` for the operator-verified gate.

## Current Model

The chat pane stack:

- **`src-tauri/src/agent_provider/`** — `AgentProvider` trait + shared types
  (`ProviderKind`, `ProviderSession`, `ProviderRuntimeEvent`,
  `ApprovalDecision`, `ProviderError`, …). Three concrete adapters:
  `claude/`, `codex/`, `opencode/`.
- **`src-tauri/src/json_rpc_child/`** — reusable `JsonRpcChildProcess`
  helper for long-lived subprocesses that speak newline-delimited
  JSON-RPC 2.0 over stdio. Used by both Claude (against the bundled
  Bun-compiled sidecar) and Codex (against `codex app-server`).
- **`sidecar/claude-agent/`** — Bun-compiled TypeScript subprocess that
  hosts Anthropic's Claude Agent SDK. Codemux never talks to Anthropic
  directly; the sidecar is a transport-only bridge that runs SDK
  `query()` in-process and forwards messages.
- **`src/components/chat/`** — chat pane UI: `AgentChatPane`, `Composer`
  (with `+` popup, `@` mention popup, slash command popup, image
  paste/drop), `ChatTranscript`, `MessageList`, `ToolCallCard` + per-tool
  bodies, `PlanProposalBlock`, `ComposerPendingInputPanel` for
  AskUserQuestion, `ThinkingIndicator`, `PermissionRequestBlock`,
  `ModePill`, `SessionSelector`, `DraftChatSurface`, `ChatHomeLanding`,
  `DebugCleanupBanner`, `DebugExitDialog`, and the picker family under
  `src/components/chat/pickers/`.

## What Works Today

- **Three end-to-end providers** behind one unified picker:
  - **Claude** — Claude Agent SDK via the Bun-compiled `claude-agent`
    sidecar (JSON-RPC over stdio).
  - **Codex** — `codex app-server` subprocess, JSON-RPC over stdio.
  - **OpenCode** — Rust-direct HTTP against a managed `opencode serve`
    child (`kill_on_drop`, generated `OPENCODE_SERVER_PASSWORD`).
  - All three render in a single 2-column picker (provider rail + searchable
    model list); favorites persist via zustand + `localStorage`.
- **Streaming chat UX**: messages, tool approvals (per-tool body rendering),
  plan proposals (`ExitPlanMode`), AskUserQuestion panels, thinking
  indicator, debug-mode banner + exit dialog.
- **Mode pills**: Ask / Allow always / Plan / Debug, with Shift+Tab cycling
  and silent-restart on pill removal.
- **Attachments** via `+` and `@`: files, folders, GitHub issues + PRs,
  images via paste / drop / picker. Inline chips, send-time injection,
  expand, caps, gif guard, chip tooltips. See
  `docs/plans/step-8-attachments.md`.
- **Slash command popup** with cross-provider parsing.
- **Cross-provider skill system**: watcher, conflicts, disable, refined
  compat. End-to-end encrypted sync (see `docs/features/skills-sync.md`).
- **MCP host runtime** (Step 9): Codemux discovers user-installed MCP
  servers across Codemux / Claude / Cursor paths, spawns each child once,
  exposes tools to the Claude SDK via an in-process facade with dynamic
  `setMcpServers` refresh. Settings panel + composer `+` popup surface
  enable/disable + status badges + tool list modal + 50-tool cap warning.
  See `docs/features/mcp-server.md`.
- **Permissions settings page** with per-tool body rendering and
  `AllowAlways` rule persistence.
- **Session lifecycle**: transcripts persist + replay on session resume;
  session history selector; permission-mode mid-session restart;
  pane-scoped chats; new-tab preset launch; base-branch picker; stop-click
  restarts the session so the next turn works.
- **Reusable JSON-RPC-over-stdio child-process helper** with timeout,
  graceful shutdown, bidirectional notifications, server-initiated
  requests, and child-exit cleanup.
- **Auth probes** (`probe_installed`, `probe_authenticated`) for each
  provider.
- **Integration tests** covering each adapter, the JSON-RPC helper, the
  Claude sidecar (38 Bun tests + 27 fake-sidecar Rust tests + 43
  translate/protocol unit tests), and the picker UI (Vitest).
- **Codex spawn race fix**: probe spawns retry on ETXTBSY (text file busy);
  `interrupt_turn_sends_turn_interrupt` and
  `auth_probe_unauthenticated_matches_common_patterns` are gated to Unix
  in CI to dodge the `fake_codex_app_server` helper-binary build race
  under cargo's parallel scheduler.

## Transcript virtualization (issue #77)

The transcript body (`MessageList.tsx`) is virtualized with
`react-virtuoso` (MIT — chosen over `@tanstack/react-virtual` and
`react-window` for its built-in dynamic row measurement and
bottom-anchoring; the commercially licensed `@virtuoso.dev/message-list`
package is NOT used). Only the on-screen window of rows is mounted, so
a 5,000-message session (the reducer cap) scrolls like a short one.

Contract preserved from the pre-virtualization renderer:

- **Stable keys + memo rows.** Per-slot keys are still `slot.item.id`
  / `run:<first-id>`, and rows render through `MessageRowMemo` — a
  streaming token mutates exactly one row.
- **Stick-to-bottom.** `MessageList` owns the scroller now (Virtuoso
  must control it). Pinned-ness is tracked from real scroll events
  (≤ 80 px from the bottom); after every transcript change (or
  thinking-pulse toggle), if pinned, it snaps to the tail via
  `scrollToIndex(LAST, end)`. Content growth alone never unpins, so
  auto-scroll never fights a user reading history.
- **Variable heights.** Tool-run collapses expand in place as a single
  virtual row; Virtuoso re-measures via ResizeObserver.
- **Thinking pulse** renders as the last virtual row (not a footer) so
  the tail snap keeps it visible while streaming.

`ChatTranscript` is now a thin shell that derives `showThinking` and
sizes the list. jsdom tests wrap renders in `VirtuosoMockContext`
(see `MessageList.test.tsx` / `MessageList.virtualization.test.tsx`).

## Current Constraints

- **Beta-gated.** The chat pane is hidden unless the user opts in via
  Settings → Beta Features. See "Beta Gate" above.
- **Single instance per provider.** A user with multiple Codex accounts or
  multiple OpenCode connections sees them collapsed under one rail entry.
  Multi-instance lifting is planned for v2 (the `ProviderInstanceId` shim
  already exists at `src-tauri/src/agent_provider/instance.rs`).
- **No keyboard shortcuts on the picker.** `Ctrl+1..9` collides with
  workspace switching; deferred until a non-colliding namespace is decided.
- The event broadcaster uses a bounded channel (default 1024) — slow
  subscribers lose old events. This is deliberate; downstream UI must
  treat the stream as live-only.
- **Image attachments in `send-turn`** currently route through the
  `images` array on user turns; the SDK paths are wired but
  multi-modal-everywhere is still settling.
- **OpenCode credential management lives in OpenCode itself.** Codemux
  never reads or writes upstream API keys.

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
where Tauri's `resources` glob picks it up. The Rust integration
tests (`src-tauri/tests/sidecar_ping.rs`) look for the same path and
skip cleanly (with a build hint) if the binary is missing.

**How it's shipped.** `tauri.conf.json`'s `bundle.resources` array
includes `binaries/codemux-claude-sidecar-*`. Tauri's packager picks
up the per-triple variant and embeds it into the AppImage / deb / rpm
/ NSIS installer under `usr/lib/codemux/binaries/` on Linux and next
to `codemux.exe` under `binaries/` on Windows. (It originally shipped
as an `externalBin` under `usr/bin/`; moved to a resource because
linuxdeploy's patchelf step corrupts the ~100 MB bun-compiled binary
during AppImage bundling — see commit 025fa19.) At runtime, the
`setup()` hook in `src-tauri/src/lib.rs` resolves the resource via
`AppHandle::path().resource_dir()` and pins the resolved path into
the `CODEMUX_CLAUDE_SIDECAR_PATH` env var so the adapter (which has
no `AppHandle` access at construction time) can find it. The release
workflow (`.github/workflows/release.yml`) installs Bun and
pre-stages the binary so tauri-action finds it before bundling. CI
(`.github/workflows/ci.yml`) does the same, with a zero-byte
placeholder fallback for constrained runners.

**Packagers note.** Downstream packagers (AUR, custom distros) MUST
copy the sidecar from the Tauri resource layout (`usr/lib/codemux/
binaries/codemux-claude-sidecar-<triple>` on Linux) into their own
package — repackaging only `usr/bin/codemux` will leave the agent-
chat Claude provider unable to find its sidecar and the first send
fails with `provider_not_configured: Claude`.

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

## Pane kind registration

The pane tree now carries an `AgentChat` variant alongside `Terminal`,
`Browser`, and `Split`. The enum lives in
`src-tauri/src/state/state_impl.rs` and mirrors into
`src/tauri/types.ts` so the frontend's discriminator pattern stays
consistent:

```rust
PaneNodeSnapshot::AgentChat {
    pane_id: PaneId,
    title: String,
    thread_id: Option<String>,
    provider: Option<ProviderKind>,
    cwd: Option<String>,
}
```

All optional fields default to `None` through `#[serde(default)]`, so
older persisted layouts (or test fixtures produced before this
variant landed) deserialize cleanly. Three serde round-trip unit
tests in the state module guard the schema:
`agent_chat_pane_serde_round_trips`,
`agent_chat_pane_defaults_round_trip`, and
`agent_chat_pane_deserializes_without_optional_fields`.

Pane creation is available on `AppStateStore::create_agent_chat_pane`,
which inserts the new pane by splitting the workspace's currently
active pane horizontally — the same insertion path
`create_browser_pane` uses. `AppStateStore::agent_chat_thread_id` /
`set_agent_chat_thread_id` read and assign the bound thread id so the
Tauri command layer can write it back after `start_session` without
another provider round-trip.

The pane renderer at `src/components/chat/AgentChatPane.tsx` is now the
full chat UI (transcript, composer with `+`/`@`/slash popups, mode pill,
streaming indicators, content blocks, inline approvals, plan proposal
panel, AskUserQuestion panel, debug-mode banner). The pane header lives
in `AgentChatPaneHeader.tsx` and surfaces session controls + the
multi-provider model picker. The empty-state composer (before a session
exists) lives in `DraftChatSurface.tsx`; the "no panes" home landing
lives in `ChatHomeLanding.tsx`.

## Tauri command surface

All commands gated on the `enable_agent_chat` feature flag. Lifecycle
commands return a `feature_disabled: ...` string when the flag is
off; session commands also return a `provider_not_configured: ...`
string when the target provider is missing from the registry.

| Command | Purpose |
|---|---|
| `agent_chat_create_pane` | Insert a new chat pane in a workspace, returns the new pane id. |
| `agent_chat_close_pane` | Close a chat pane. Idempotent — double-close is a no-op. |
| `dev_agent_chat_spawn_test_pane` | Debug-only. Spawns a chat pane in the active workspace for manual QA from the browser devtools. |
| `agent_chat_start_session` | Start a provider session, writing the returned thread id back onto the pane. |
| `agent_chat_send_turn` | Queue a user turn on a thread. |
| `agent_chat_interrupt_turn` | Halt the currently-running turn. |
| `agent_chat_respond_to_request` | Resolve a pending approval / input request. |
| `agent_chat_set_model` | Swap a thread's model mid-session. |
| `agent_chat_set_permission_mode` | Swap a thread's permission mode mid-session. |
| `agent_chat_stop_session` | Gracefully close a session. Idempotent. |

Provider errors are serialized as `SerializableProviderError` JSON so
the UI can inspect the error subtype (e.g.
`{"kind":"not_authenticated", ...}`) instead of parsing a free-form
string.

## Event bridge

Every registered provider's canonical event stream is routed to the
frontend over **per-thread Tauri Channels** (issue #75 — Tauri's
recommended mechanism for high-throughput streaming, mirroring the
PTY output path). When a pane binds to a thread, the
`useAgentChatEvents(threadId, handler)` hook
(`src/hooks/use-agent-chat-events.ts`) invokes
`attach_agent_chat_output(thread_id, channel)`; on unmount it calls
`detach_agent_chat_output(thread_id, subscription_id)`. The backend's
`AgentChatChannelRegistry` (`commands/agent_chat.rs`) maps each
thread id to its attached channels and `forward_event` sends each
thread-scoped event — including the high-frequency `content_delta`
token stream — only to that thread's channels, so a pane never
receives (or filters) another thread's traffic. Multiple panes may
attach to the same thread; dead channels (webview reload without
detach) fail on send and are pruned lazily.

Only **threadless** events (global `RuntimeWarning`s with no owning
pane) still go out on the legacy `agent_chat_event` broadcast bus,
with an empty `ThreadId`.

Replay semantics: transcript-mutating events are persisted to
`agent_chat_messages` (unchanged), so a late-attaching or resumed
pane hydrates history from the DB via `agent_chat_list_messages`
while the channel carries only live deltas. Partial deltas are never
persisted — they're superseded by their `item_completed`.

The bridge is a thin loop: one background Tokio task per provider,
each consuming the provider's `event_stream()` and routing each
event through `forward_event`. `broadcast::error::RecvError::Lagged`
is already swallowed by each provider's event-stream helper, so slow
subscribers never crash the loop — they just drop old events.

## Run-start rollback checkpoints (issue #80, opt-in)

When `git.agent_checkpoint_enabled` is on (Settings → Git; default
**off**), `agent_chat_start_session` fires a **background** snapshot
of the workspace working tree after the session is live — nothing on
the first-token path awaits it. The snapshot uses a scratch index
(`GIT_INDEX_FILE`), so it captures modified **and untracked** files
without ever touching the user's real index or worktree, and is
pinned under `refs/codemux/checkpoints/<thread>`; the commit + HEAD
hashes are recorded on the `agent_chat_sessions` row
(`checkpoint_commit` / `checkpoint_head`).

The pane header shows a "Restore checkpoint" action (History icon,
hover-reveal) when the thread has a recorded checkpoint. Restore is
**tree-only**: a safety snapshot of the current state is pinned under
`refs/codemux/pre-restore/<thread>` first, then
`git read-tree --reset -u` + `git clean -fd` make the tree match the
snapshot — files created after the checkpoint are removed, ignored
files and branch refs/commits are untouched. Commands:
`agent_chat_get_checkpoint`, `agent_chat_restore_checkpoint`; git
helpers in `src-tauri/src/git.rs`
(`git_create_workspace_checkpoint` / `git_restore_workspace_checkpoint`).
Design notes: `docs/plans/agent-run-checkpoints.md`.

## Feature flag

The new flag `enable_agent_chat` lives on the existing `FeatureFlags`
struct in `src-tauri/src/observability.rs`. It defaults to `false`;
the entire provider registry (Claude + Codex adapters) skips
initialisation while it's off, saving the memory.

Three ways to flip it on locally:

1. **UI:** once the chat pane ships in Step 4 the settings panel will
   expose this. For now, call the existing
   `update_feature_flags` Tauri command from the browser devtools:

   ```js
   await window.__TAURI__.invoke("update_feature_flags", {
     flags: {
       unstable_openflow: true,
       unstable_browser_automation: true,
       unstable_indexing: true,
       enable_agent_chat: true,
     },
   });
   ```

2. **Config file:** edit `.codemux/observability.json` in the project
   root, set `feature_flags.enable_agent_chat: true`, restart.

3. **Fresh project:** the default store is persisted lazily, so a
   brand-new Codemux project has no file yet. Start the app once
   (to create the file), then edit it and restart.

After the flag is on, open the browser devtools and call:

```js
await window.__TAURI__.invoke("dev_agent_chat_spawn_test_pane");
```

to insert a stub chat pane in the active workspace.

## Dev affordances

A dev-only "Spawn chat pane" button lives in the window title bar in
debug builds when `enable_agent_chat` is on. It invokes
`dev_agent_chat_spawn_test_pane` to drop a chat pane into the active
workspace. Useful for quick manual testing without going through the
sidebar `+` flow.

Under `npm run dev` (plain-browser mock), the seeded
**agent-chat-demo** workspace carries an `agent_chat` pane bound to
`MOCK_CHAT_THREAD_ID`. The mock hydrates a ~790-row transcript through
the real reducer (`agent_chat_list_messages`), streams a simulated
reply on `agent_chat_send_turn`, and exposes
`window.__codemuxChatMock.streamReply()` for on-demand streaming —
the standing harness for transcript-virtualization and scroll-pinning
work.

## Step 9 — Cross-provider MCP server runtime (shipped)

Codemux now hosts user-installed MCP servers as first-class
infrastructure: discovers configs across Codemux/Claude/Cursor paths,
spawns each server once, exposes their tools to Claude SDK sessions
through an in-process facade, and surfaces enable/disable controls in
both Settings and the composer's `+` popup. Stages 1–6 (config
discovery → backend runtime → SDK facade → polish → Codex spike →
cleanup) all shipped on this branch.

See `docs/features/mcp-server.md` for the canonical feature description
and `docs/plans/step-9-mcp-servers.md` for the original research +
locked design decisions. The Stage 5 spike at
`docs/plans/step-9-codex-mcp-spike.md` recommends Step 11 as the path
to extend MCP host support to Codex via an HTTP gateway.

## Roadmap (next steps)

- **Step 10 — Skills sync** (LANDED, Stages 1-6). See
  `docs/features/skills-sync.md`.
- **Step 10.5 — Project-scoped skills sync** (PLANNED, ~3-5 days).
  Sync skills tied to specific git repos in addition to the user-global
  ones already shipping. Schema is additive (`project_remote_url_hash`).
  Trickiest piece is URL canonicalization.
- **Step 11 — Codex MCP via HTTP gateway** (PLANNED). Codemux exposes
  a localhost streamable HTTP MCP endpoint, writes
  `[mcp_servers.codemux] url = "..."` into `~/.codex/config.toml`, and
  hot-reloads via the `config/mcpServer/reload` RPC when the registry
  changes. Reuses the entire Stage 1–4 stack. Spike at
  `docs/plans/step-9-codex-mcp-spike.md`.
- **Step 12 — Multi-provider chat** (LANDED, Stages 1-9). See
  `docs/features/multi-provider-chat.md`.
- **Step 13 — Agent Chat Beta toggle** (LANDED). See
  `docs/plans/step-13-beta-toggle-research.md`.
- **Promote agent-chat from Beta to default-on** once dogfooding
  settles. Both feature flags would default to `true`; the legacy paths
  stay in tree as a fallback for a release cycle before being removed.

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
