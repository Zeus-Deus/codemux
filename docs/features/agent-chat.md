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
