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

Neither module is reachable from Tauri commands, UI, or any runtime code
paths today. They sit dormant until later tasks add concrete Claude / Codex
adapters and wire a chat pane to them.

## What Works Today

- Stable trait/type surface for provider adapters.
- Reusable JSON-RPC-over-stdio child-process helper with timeout, graceful
  shutdown, bidirectional notifications, server-initiated requests, and
  child-exit cleanup.
- Integration tests (`src-tauri/tests/json_rpc_child.rs`) covering
  request/response roundtrip, both notification directions, server-initiated
  request roundtrip, timeout, child-exit diagnostics, graceful shutdown,
  malformed-input resilience, and 20-way concurrent requests.

## Current Constraints

- No user-visible chat panel yet.
- No concrete Claude or Codex provider implementation yet — the trait has no
  in-tree implementers.
- No Tauri commands exposed.
- No persistence, projection pipeline, or event sourcing.
- No permission / approval UX; the approval event types exist but nothing
  routes them to a human.

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
- `src-tauri/src/json_rpc_child/mod.rs` — the `JsonRpcChild` helper.
- `src-tauri/tests/json_rpc_child.rs` — helper tests.
- `src-tauri/tests/helpers/fake_rpc_child/main.rs` — in-tree JSON-RPC peer
  used as the tests' fixture.

## Notes

- Keep this file about current truth, not future plans. Future chunks (Claude
  adapter, Codex adapter, chat pane, orchestration wiring) will each own
  their own `docs/plans/` entry at time of work.
