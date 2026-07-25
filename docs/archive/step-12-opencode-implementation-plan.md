# Step 12 — OpenCode Integration Implementation Plan

> **ARCHIVED.** This plan's work has landed; it is kept as the implementation
> record and reasoning trail, not as current truth. For how this behaves today
> read the relevant `docs/features/*` doc (see `docs/INDEX.md`).

- Purpose: Concrete scoping deliverable answering nine GO/MODIFY/DEFER questions for adding OpenCode as a third chat provider in Codemux's GUI.
- Audience: Decision-maker reviewing whether to commit to Step 12 now, modify the scope, or defer.
- Authority: Pre-build planning checkpoint + post-shipping summary. The body below was the original scoping doc; the **Final State** header at the top reflects what actually shipped.
- Update when: A scoping fact below is wrong, a deferred follow-up gets reactivated, or the multi-provider feature needs a re-scoping pass.
- Read next: `docs/features/multi-provider-chat.md` for the current behaviour, `docs/research/step-12-opencode-research.md` for the reference-impl research backing this plan, `docs/archive/step-12-ui-smoke-checklist.md` for the operator smoke.

## Final State (2026-05-02)

**Status: SHIPPED. Stages 1-7 complete; Stage 5 (multi-instance) explicitly deferred to v2.**

Multi-provider chat (Claude + Codex + OpenCode) is live. The original scoping body below remains as a historical record of what was considered and what was decided pre-build — the actual delivery diverged from the TL;DR's "defer" recommendation. Per the "Stages 1-3 only" alt path was rejected mid-build; instead the full Stages 1-4 + 6 + 7 path landed (Stage 5 multi-instance explicitly skipped).

### What shipped

| Stage | Status | Highlights |
|---|---|---|
| 1 | ✅ Stages 1-3 in commit `0dcf05b` | `ProviderKind::OpenCode` enum + match-arm exhaustiveness across registry/dispatcher/DB; `agent_provider/opencode/{discovery,client,capabilities}` modules; `ProviderInstanceId` shim; Tauri commands `opencode_check_availability` + `opencode_ping`. |
| 2 | ✅ same commit | `OpenCodeServer` (`kill_on_drop`, generated 32-char `OPENCODE_SERVER_PASSWORD`, stdout-line ready detection) + singleton `OpenCodeServerManager` + `list_models()` against `GET /provider`. Live-verified against OpenCode 1.14.31 — 116 providers / 4,354 models harvested. |
| 3 | ✅ same commit | `ChatModelInfo.sub_provider: Option<String>`; `harvest_opencode_capabilities` flattens upstream wire shape into chat-side `ChatModelInfo` with namespaced `${provider_id}/${model_id}` slugs and connected-only filter. `provider-capabilities-store` now has 3 slots + parallel `refreshAll` + exhaustive `selectCapabilities`. |
| 4 | ✅ uncommitted at time of writing | `MultiProviderModelPicker` — 2-column popover (48px provider rail + searchable model list), search collapses provider grouping, federated subtitle line `OpenCode · {sub_provider}`, three empty states (no-match / OpenCode-not-installed / no-connected-providers). `ENABLE_PROVIDER_PICKER` flipped to `true`; Codex finally selectable in the GUI. |
| 5 | ⏭️ DEFERRED to v2 | Multi-instance per provider (multiple Codex accounts, multiple OpenCode connections). `ProviderInstanceId` shim from Stage 1 keeps the lift small when v2 lands. |
| 6 | ✅ uncommitted at time of writing | Favorites — zustand `persist` store (`codemux:picker-favorites:v1`), `${provider}::${modelId}` keys, hover-reveal star button with triple-stop click isolation, favorites bubble to the top of both rail-only and cross-provider search lists. |
| 7 | ✅ this stage | Docs (this file + feature doc + UI smoke checklist), STATUS/PLAN updates, dead-code audit, OpenCode install-hint copy in the empty state. |

### Cumulative test deltas across Step 12

- Rust lib: **+29 tests** vs. pre-Step-12 (1093 → 1122). New tests are concentrated under `agent_provider::opencode::*` (server lifecycle, manager idempotency, client + flatten, capabilities harvest), `agent_provider::instance::*` (forward-compat shim), and one `state::state_impl` test pinning the all-three-providers serde round-trip.
- Frontend Vitest: **+44 tests across 3 new files** vs. pre-Step-12 (1456 → 1500). Stage 3 added 8 (`provider-capabilities-store.test.ts`); Stage 4 added 20 (`MultiProviderModelPicker.test.tsx`); Stage 6 added 9 + 7 (`picker-favorites-store.test.ts` + `MultiProviderModelPicker.test.tsx` favorites suite).
- Pre-existing flakes: two integration tests in `tests/codex_adapter.rs` continue to flake under cargo's parallel scheduler (`fake_codex_app_server` helper-binary build race); both pass cleanly in isolation or with `--test-threads=1`. Unrelated to Step 12.

### Deferred follow-ups

1. **v2 — Multi-instance per provider.** Multiple Codex accounts or multiple OpenCode connections collapsed under one rail entry today. `ProviderInstanceId` already exists; the wire format already serialises as a bare slug (`"claude"` / `"codex"` / `"opencode"`).
2. **v2 — Picker keyboard shortcuts.** `Ctrl+1..9` collides with workspace switching. Slot-based jumps + a non-colliding namespace (`Cmd+Shift+1..N` or in-popover-only) need a deliberate keybinding decision.
3. **Future cleanup — OpenFlow capabilities convergence.** OpenFlow's `list_models_for_tool` Tauri command keeps a hardcoded model registry separate from the chat-side capabilities harvest; tracked by a parity-by-comment dependency at `src-tauri/src/agent_provider/codex/capabilities.rs:33`. Convergence would let us drop the duplication, but it touches OpenFlow's CLI-launcher invariants and is not blocking anything today.

---

## TL;DR — original recommendation (pre-build, kept for history)

**Defer Step 12 behind Step 11 (Codex MCP HTTP gateway) and Step 10.5 (project-scoped skills sync).** OpenCode integration is real engineering — three weeks-to-month elapsed, two new transport surfaces, a picker rewrite, plus credential UX — and the audience wanting "OpenCode in Codemux's chat GUI" overlaps heavily with users already comfortable in a terminal. Step 11 unblocks an existing user surface (Codex MCP); Step 10.5 unblocks paying-customer behavior (project-scoped sync). Step 12 broadens the funnel but doesn't unblock anyone today. Build it, but not next.

If the scope is reduced to "Stages 1-3 only" (backend driver + live model harvest + sub_provider field, no picker rewrite, no per-instance config), this becomes a ~1-week feature that ships value much sooner — see §6 Stage 3.5 alt path.

The recommendation above was overridden — the full picker rebuild landed alongside the backend work. See "Final State" at the top of this doc.

---

## 1. Scope of disruption to existing code

The `feature/agent-chat` branch already separates concerns cleanly via `AgentProvider` trait + `JsonRpcChild` + `ProviderRegistry`. Mechanical adds outnumber design work, but the sidecar adapter shape is the one that genuinely diverges.

### 1.1 Mechanical changes (just add a third arm)

| File:line | What changes |
|---|---|
| `src/tauri/types.ts:560` | `AgentChatProviderKind = "claude" \| "codex" \| "opencode"`. |
| `src-tauri/src/agent_provider/types.rs:17-24` | Add `OpenCode` variant to `ProviderKind`. |
| `src-tauri/src/commands/agent_chat.rs:309-312, 487-494` | Match arms in serializer + capabilities dispatcher. |
| `src-tauri/src/commands/agent_chat.rs:70-117` | `ProviderRegistry` gains a third `RwLock<Option<Arc<dyn AgentProvider>>>` field for OpenCode. |
| `src/components/chat/provider-logo.tsx:11,16` | Add OpenCode icon + label to `PROVIDER_ICON_MAP` and `PROVIDER_LABEL`. |
| `src/components/chat/pickers/ProviderPicker.tsx:20-23` | Add OpenCode entry to `PROVIDERS` array. |
| `src/lib/agent-chat/capability-defaults.ts:19` | Add OpenCode default model fallback. |
| `src/stores/provider-capabilities-store.ts:11-52` | Add `opencode: ProviderChatCapabilities \| null` field + matching `refresh` arm. |
| `src/components/overlays/new-workspace-dialog.tsx` | Optional: wire OpenCode preset. (The recent commit `c81119f` deliberately hides chat_agent presets from the CLI launcher; mirror that gate.) |

These are 1-2 line edits each.

### 1.2 Real design work

| File:line | Why it's not mechanical |
|---|---|
| `src/components/chat/AgentChatPane.tsx:1684` | `if (provider === "claude") { … }` branch — gates restart-on-effort-change behavior. OpenCode reasoning is per-`session.promptAsync` call; restart semantics need a deliberate decision. |
| `src/lib/agent-chat/chat-pane-plans.ts:102` | `restart: provider === "claude"` — ditto, plus must decide whether OpenCode's "agent" or "variant" selectors warrant restart. |
| `src/lib/agent-chat/chat-pane-plans.ts:170-175` | Codex uses `effortOverride` (no `set-model` RPC); OpenCode uses `variant` per-call AND `agent` per-call. New code path. |
| `src-tauri/src/agent_provider/` (new module) | New driver implementation — does NOT fit `JsonRpcChild`. See §2. |
| `src/components/chat/ComposerFooter.tsx` | Currently hosts `ProviderPicker` + `ModelPicker` + `ReasoningPicker` + `PermissionModePicker` + `ModePill`. Adding `variant` and `agent` chips for OpenCode means either (a) a generic descriptor renderer (TraitsPicker pattern from the reference impl) or (b) per-provider chip groups. Real UX decision. |
| `src/tauri/types.ts:572-587` (`ChatModelInfo`) | Adding `sub_provider: string \| null` is non-breaking, but consumers must be updated to display it. |
| `src-tauri/src/skills/...` and Step 9 MCP runtime | OpenCode handles its own MCP servers via its own config. Codemux's facade work is Claude-only; OpenCode integration is independent. See §7 risks. |

### 1.3 What stays untouched

- `src-tauri/src/json_rpc_child/mod.rs` — keep for Claude/Codex only.
- `src-tauri/src/agent_provider/provider.rs:29-93` — `AgentProvider` trait is broad enough that an HTTP+SDK driver implements it without changes. This is the architectural payoff of Step 6+.
- Capability resolution layer: `src/lib/agent-chat/model-resolution.ts` already matches the reference-impl shape (`resolveEffort`, `resolveContextWindow`, prompt-injected efforts).
- `src/stores/agent-chat-store.ts`, persistence, hooks, MCP runtime, skills sync engine.

### 1.4 Test surface impact

Existing tests that pattern-match on provider kind:

```
src/lib/agent-chat/chat-pane-plans.test.ts
src/lib/agent-chat/model-resolution.test.ts
src/lib/agent-chat/capability-defaults.test.ts
src/components/chat/AgentChatPane.test.tsx
src/components/chat/Composer.test.tsx
```

Each gains 1-3 OpenCode cases. Net: ~10-15 new TS tests in Stage 3, ~30-50 new Rust tests across Stages 1-2 (driver lifecycle, harvest, error mapping). See §6 per-stage estimates.

---

## 2. OpenCode SDK integration

### 2.1 SDK surface

`@opencode-ai/sdk@1.3.15` v2 (per `/tmp/<reference>/apps/server/package.json:32`). It is a TypeScript HTTP client over OpenCode's local HTTP server. Key calls used by the reference impl:

| Call | Shape | Used for |
|---|---|---|
| `client.provider.list()` | one-shot await, returns `ProviderListResponse` with `connected: string[]` and `all: Array<{id, name, models: Record<string, ModelInfo>}>` | model harvest |
| `client.app.agents()` | one-shot await, returns `Agent[]` | agent picker descriptors |
| `client.session.promptAsync({sessionID, model, agent?, variant?, parts})` | fire-and-forget ACK | turn dispatch |
| `client.event.subscribe(undefined, {signal})` | async iterable subscription | streaming events |
| `client.session.abort({sessionID})` | one-shot await | turn cancel |
| `client.session.messages({sessionID})` | one-shot await | history rehydrate |

Event union from the subscription: `message.updated`, `message.removed`, `message.part.delta`, `message.part.updated`, `permission.asked`, `permission.replied`, `question.asked`, `question.replied`, `question.rejected`, `session.status`, `session.error`. Reference event handler implementation: `/tmp/<reference>/apps/server/src/provider/Layers/OpenCodeAdapter.ts:656-949`.

### 2.2 CLI dependency

**Yes, requires `opencode` binary on PATH.** The reference impl spawns it as a local HTTP server:

```
opencode serve --hostname=127.0.0.1 --port=<auto>
```

then parses stdout for `"opencode server listening on http://..."` and regex-extracts the URL. Lifetime is bound to the spawn scope; SIGTERM → 1s sleep → SIGKILL on shutdown.

`OPENCODE_CONFIG_CONTENT={}` is set in env to override config (the reference impl uses `{}` to opt into upstream defaults; we may want a different stance — see §3).

Minimum version `1.14.19` (`OpenCodeProvider.ts:375-402`). The reference impl does NOT bundle OpenCode; the user installs it (homebrew, AUR, npm `@opencode-ai/cli`, etc.).

### 2.3 Credentials

Two layers:

- **Server password** — HTTP Basic auth on the local server (`Authorization: Basic base64("opencode:${serverPassword}")`). Stored in the reference impl's settings. We can default to no password (local loopback only) and surface as advanced setting.
- **Upstream provider keys** (OpenAI, Anthropic, OpenRouter, Google) — the reference impl does **not** read or store these. OpenCode reads them from `~/.config/opencode/` and env vars. Codemux should follow this — do not touch upstream creds.

UX implication: a Codemux user with OpenCode installed but unconfigured will see "0 upstream providers connected" and an empty model list. We need a clear pointer to OpenCode's auth flow (`opencode auth login`), not a custom Codemux-side credential UI. **No API-key-management UX in Codemux for OpenCode.** Big scope reducer.

### 2.4 Streaming protocol

HTTP server + async iterable event stream. NOT Server-Sent Events in the typical sense — the SDK abstracts the underlying transport (mix of HTTP request/response and a long-poll/WS subscription stream).

### 2.5 Fit with `JsonRpcChild`

**Does not fit.** JsonRpcChild is line-delimited JSON-RPC 2.0 over stdin/stdout pipes. OpenCode is HTTP server + SDK. The async iterable subscription cannot be re-shaped into request/response pairs without rewriting the SDK.

Three implementation options for Codemux:

- **(a) Port the SDK calls to Rust over HTTP** — talk to OpenCode's HTTP server directly from `src-tauri/`. Reasonable if OpenCode publishes an OpenAPI spec. Estimated 8-12 days for the driver + streaming. **Recommended** — keeps Codemux's "Rust owns providers, sidecars are TS" architecture intact.
- **(b) New Node sidecar** mirroring `sidecar/claude-agent/` — wrap `@opencode-ai/sdk` in a Node binary that speaks JSON-RPC over stdio to Rust (so it does fit JsonRpcChild). Estimated 5-8 days. Adds a second sidecar binary to ship + a second TS build pipeline. **Faster but introduces ongoing maintenance debt.**
- **(c) WASM-compile the JS SDK and load in Rust** — not realistic; the SDK depends on Node's `fetch`, file I/O for config, etc. Reject.

Lock recommendation: **option (a)**. Investigate the HTTP API surface in Stage 1 before committing. If the API is undocumented or unstable, fall back to (b).

### 2.6 Process lifecycle

Whichever option, the OpenCode adapter must:

1. Locate `opencode` binary (`which opencode` or settings override path).
2. Spawn `opencode serve --hostname=127.0.0.1 --port=<chosen>` with env override.
3. Read stdout until ready-line; capture URL.
4. Construct HTTP client; call `provider.list()` + `app.agents()` for capabilities snapshot.
5. On `agent_chat_start_session`, `session.create({})` to get session id.
6. On `send_turn`, `session.promptAsync(...)` + start event subscription.
7. Translate OpenCode events into Codemux's `ProviderRuntimeEvent` union (matches Codex/Claude shape).
8. On shutdown: `session.abort` + SIGTERM-then-SIGKILL the server, similar to JsonRpcChild's graceful path.

Existing infrastructure to reuse:
- `sanitize_gui_env_tokio` from CLAUDE.md (mandatory before spawning).
- `ProviderRuntimeEvent` shape and `event_stream()` channel.
- Approval / permission event union — shape already supports `RequestOpened { kinds: command|file-read|file-change|other }`.

---

## 3. Per-instance config

### 3.1 Decision

**v1 = single instance per driver. Driver === instanceId.** Defer multi-instance to v2.

### 3.2 Justification

- The reference impl added multi-instance to support distinct credential profiles. For OpenCode specifically, **upstream creds live in OpenCode's own config**, not Codemux's — multi-instance would only differentiate `serverPassword` and `binaryPath`, neither of which a typical user changes.
- Codemux today has zero multi-instance infrastructure: settings shape, sync (Step 10), pane provider field, capability store, picker rail, ChatModelInfo all assume `provider: AgentChatProviderKind`. Lifting that to `instance: ProviderInstanceId` ripples through every chat surface.
- Multi-instance UX without strong demand creates "Why are there two Claudes in the picker?" confusion for new users.
- The data model can be made forward-compatible: introduce `instance_id: string` on session-creation payloads now, default to `instance_id === provider`, and treat the picker as a pre-collapsed view. When v2 lands, the underlying Tauri commands don't change shape.

### 3.3 Forward-compat shim (zero-cost, recommended)

In Stage 1, when defining the OpenCode driver's startup config struct, name the field `instance_id` even though the value is constant. When ChatModelInfo gets `sub_provider`, also add a constant `instance_id: ProviderInstanceId` field on session payloads. Future v2 just changes a singleton to a map.

---

## 4. UI changes — picker redesign

### 4.1 Scope of the rebuild

The reference impl's two-column picker is a substantial rewrite. From `/tmp/<reference>/apps/web/src/components/chat/ModelPickerContent.tsx` (~640 lines):

- Left rail (`ModelPickerSidebar`, 50px): provider icons + favorites star + selected indicator bar + "Soon"/"New" badges + per-instance accent badges.
- Main column: search input (`ComboboxInput`), virtualized model list (`ComboboxList` + `ModelListRow`), empty state.
- Search collapses provider grouping. Favorites get -24 score boost, not a separate section.
- `Ctrl+1`…`Ctrl+5` map to visible-filtered position via global keybinding context.
- Star toggle persists to user settings.

### 4.2 Estimate

- **Lines of new TSX**: ~600-800 (picker content) + ~200-300 (sidebar) + ~150 (search/score helpers) + ~100 (keybinding wiring) ≈ **1,200-1,400 LoC** of new TS, not counting tests.
- **Tests**: ~30-40 new Vitest cases (search ranking, favorites, keyboard slots, sidebar selection, descriptor sanitization on switch).
- **Person-time**: 4-6 working days at this codebase's cadence (compare: `DerivativeBranchPicker` was a multi-day effort and is much smaller).
- **Composer footer rewrite**: ComposerFooter.tsx grows from 5 chips to a generic descriptor renderer (TraitsPicker pattern) so OpenCode's `variant` + `agent` chips don't require bespoke code. **Adds 1-2 days; without it, every new descriptor is a code change.**

### 4.3 Integration with composer

ComposerFooter.tsx wraps the existing pickers as siblings. The new picker is a single dropdown trigger replacing both `ProviderPicker` and `ModelPicker` (matching the reference impl's "one chip" pattern). Accordion: keep existing chips functional during the transition; introduce the unified picker behind a feature flag.

### 4.4 Keyboard shortcuts

`Ctrl+1`…`Ctrl+5` are slot-based on filtered position — users build muscle memory only after favoriting. We already use `Ctrl+1..9` elsewhere (workspace switching, see `KEYBINDINGS.md`); **must namespace differently** (e.g., `Cmd+Shift+1` or in-popover-only). Real keybinding decision; cannot copy the reference impl's choice verbatim.

### 4.5 Favorites

Match the reference impl: `Array<{ provider: AgentChatProviderKind, model: string }>` in user settings (zustand-persisted). Score boost -24 in search ranking. No separate section.

### 4.6 Provider initials badges

Skip in v1 (no multi-instance, no need to disambiguate). Wire the visual shell so v2 can flip it on without rework.

---

## 5. Capability descriptors

### 5.1 Current state

Codemux's chips are **bespoke per descriptor**:

- `src/components/chat/pickers/ReasoningPicker.tsx` — reasoning level.
- `src/components/chat/pickers/PermissionModePicker.tsx` — sandbox mode.
- `src/components/chat/pickers/ModePill.tsx` — Build/Plan toggle.
- `src/components/chat/pickers/ModelPicker.tsx` — model.

Each picker is hand-written, takes a `ChatModelInfo`-shaped or `string[]`-shaped option set, and renders a popover. **There is no generic "descriptor → chip" renderer.**

### 5.2 What OpenCode needs

OpenCode adds two new descriptor families:

- `variant` (e.g., `"low" | "medium" | "high"` — a per-model effort knob) — semantically overlaps Claude's effort but uses different option ids.
- `agent` (e.g., `"build" | "plan" | "review"` — a per-model agent profile) — overlaps Codemux's existing Build/Plan mode but originates from OpenCode's own agent registry.

Two paths:

- **(a) Per-provider chip groups** — render different chip sets based on `provider`. Lower disruption (~1 day), but each new descriptor is a code change.
- **(b) Generic descriptor renderer** (the reference impl's `TraitsPicker`) — `ChatModelInfo` carries `option_descriptors: OptionDescriptor[]`, the composer iterates and renders one chip per descriptor. **3-5 days** to refactor existing chips into this shape, but new descriptors then land for free.

Recommendation: **(b) for Stage 4**. The refactor pays for itself when OpenCode descriptors land and again when MCP-tool-toggle chips arrive (Step 11+).

### 5.3 Sanitization on model switch

Codemux already has the right primitives:
- `resolveEffort` at `src/lib/agent-chat/model-resolution.ts:39`
- `resolveContextWindow` at `src/lib/agent-chat/model-resolution.ts:88`

Pattern matches the reference impl's `resolveDescriptorChoiceValue`: silently fall back to the new model's default when the prior selection isn't supported. **No refactor at this layer.** Generic-ifying just means moving these fns into a `resolveDescriptor(descriptor, raw)` shape if Stage 4 path (b) is chosen.

---

## 6. Stages proposal

Seven stages. Pattern matches Step 9/10 cadence — research locked first, demoable cuts every stage, last stage is polish + docs.

### Stage 1 — Backend OpenCode driver scaffold (no harvest, no UI)

**Frontend / Backend**: Backend.

**Deliverables:**
- New module `src-tauri/src/agent_provider/opencode/` with `mod.rs`, `lifecycle.rs`, `http_client.rs`, `protocol.rs`.
- `OpenCodeProvider` impl of `AgentProvider` trait (capabilities returns hardcoded fallback, all turn methods return `ProviderError::NotImplemented` for now).
- Spawn lifecycle: `opencode serve --hostname=127.0.0.1 --port=<auto>`, stdout-line URL discovery, graceful shutdown, version probe + min-version check, GUI env sanitation.
- Tauri command `opencode_probe` that returns `{ installed, version, server_url }` for diagnostics.
- `ProviderKind::OpenCode` arm wired through `agent_chat.rs` for capability listing only (returns fallback hardcoded models).
- `AgentChatProviderKind = "claude" | "codex" | "opencode"` in TS.

**Verification:** `opencode_probe` returns `{installed:true, version: ">=1.14.19"}` on dev machine. Shutdown cleanly tears down the spawned server (no zombies). 25-30 Rust tests for spawn, version, error mapping (ENOENT, version-too-old, port-in-use). No frontend changes user-visible.

**Dependencies:** None.

### Stage 2 — Live model harvest via HTTP API

**Frontend / Backend**: Backend.

**Deliverables:**
- HTTP client implementation of `provider.list` and `app.agents` calls in Rust (option (a) from §2.5). Investigate OpenCode's HTTP API spec; if undocumented, fall back to option (b) Node sidecar.
- `flatten_opencode_models` analogue: emit `ChatModelInfo[]` with `slug = "${providerId}/${modelId}"`.
- Capability descriptors per model (variants, agents) extracted from the harvest response.
- `list_chat_provider_capabilities("opencode")` returns live data.
- `provider-capabilities-store.ts` gains `opencode` field; `useProviderCapabilitiesInit` refreshes it.

**Verification:** Calling `list_chat_provider_capabilities("opencode")` on a machine with OpenCode + an OpenAI key returns real `gpt-5` / `gpt-5.4` / etc. entries with sub_provider set to "OpenAI". Shape matches `ChatModelInfo`. ~20 Rust tests + ~5 TS tests.

**Dependencies:** Stage 1.

### Stage 3 — `ChatModelInfo.sub_provider` + minimal picker integration

**Frontend / Backend**: Both, frontend-heavy.

**Deliverables:**
- Add `sub_provider: string \| null` to `ChatModelInfo` (TS + Rust). Display below model name in existing `ModelPicker`. Non-breaking — Claude/Codex models leave it null.
- `ProviderPicker` gains OpenCode entry. `ProviderLogo` gains OpenCode icon.
- `ChatPanePlans.ts` arms updated: OpenCode model-switch behavior (no restart; emit `set_model` analog).
- Wire `agent_chat_send_turn` for OpenCode through to `session.promptAsync` with hardcoded `agent: "default"`, `variant: descriptor.default`. End-to-end "send a message, get a response" works.

**Verification:** A user opens a chat, picks Provider=OpenCode, sees the live model list with sub-provider labels, sends "Hello", gets a streamed reply. Token-delta events render. Approval flow works for at least one tool. ~15 TS + ~10 Rust tests. **First demoable end-to-end milestone — feature flag flips ON for testers.**

**Dependencies:** Stages 1-2.

> **Stage 3.5 alt path / "minimal viable feature":** Stop here. Skip Stages 4-7. Ship a functional but rough OpenCode chat in the existing one-dropdown picker. ~1-1.5 weeks total elapsed. Delivers 70-80% of user value. See §9.

### Stage 4 — Generic capability-descriptor renderer (TraitsPicker pattern)

**Frontend / Backend**: Frontend.

**Deliverables:**
- Refactor existing `ReasoningPicker`, `PermissionModePicker`, `ModePill` into a unified `DescriptorPicker` driven by `ChatModelInfo.option_descriptors[]`.
- ComposerFooter.tsx renders a chip per descriptor, conditional on the active model exposing one.
- OpenCode's `variant` + `agent` descriptors render automatically.
- Selection sanitization wraps the existing `resolveEffort` / `resolveContextWindow` helpers into a generic `resolveDescriptor(descriptor, raw)`.

**Verification:** Switching from Claude Opus 4.7 to OpenCode `openai/gpt-5` swaps chip set silently; selections that don't apply reset to descriptor defaults. ~25 TS tests covering the matrix. No regressions on Claude/Codex chip behavior.

**Dependencies:** Stage 3.

### Stage 5 — Two-column picker rebuild

**Frontend / Backend**: Frontend.

**Deliverables:**
- New `ModelPickerContent.tsx` with provider rail + search input + flat ranked list + favorites.
- Search collapses provider grouping; tokenized fuzzy ranking with -24 favorite boost.
- Empty state, loading state, "OpenCode not installed" state.
- Replaces both `ProviderPicker` and `ModelPicker` triggers in ComposerFooter behind the unified picker.
- Per-instance accent badges scaffolded but unused (forward-compat for v2).

**Verification:** Operator UI smoke checklist (mirror Step 10 pattern): all three providers' models appear, search ranks favorites first, switching providers in the rail filters the list, loading + error states render. ~30-40 Vitest cases, ~5 hand-driven smoke checks documented in `docs/archive/step-12-ui-smoke-checklist.md`.

**Dependencies:** Stages 3-4.

### Stage 6 — Favorites + keyboard shortcuts

**Frontend / Backend**: Frontend.

**Deliverables:**
- Favorites persisted in user settings (`Array<{ provider, model }>`). Star toggle on each row.
- `Ctrl+Shift+1`…`Ctrl+Shift+5` (or namespace-different — see §4.4) jump to visible-filtered position when picker is open.
- Keybinding label rendered on each row.
- Favorites optionally synced via Step 10's skills-sync engine (no, defer — favorites are not E2E-encryption-worthy).

**Verification:** Star a model; reload; favorite persists. Shortcut keys jump to expected slots after filtering. ~15 TS tests.

**Dependencies:** Stage 5.

### Stage 7 — Polish + docs + smoke

**Frontend / Backend**: Both.

**Deliverables:**
- Error UX: "OpenCode CLI not installed" message + install instructions. "0 upstream providers connected" guidance toward `opencode auth login`.
- Per-status indicator on the OpenCode rail icon (running / probing / error) reusing Step 9 status-dot patterns.
- New canonical doc describing the integration (shipped as `docs/features/multi-provider-chat.md`, with conversation-sync split into `docs/features/opencode-conversation-sync.md`).
- Update `docs/INDEX.md`, `docs/core/STATUS.md`, `docs/core/PLAN.md`.
- Live cross-environment smoke against three real upstream provider keys (OpenAI, Anthropic, Google) on dev machine. Document checklist in `docs/archive/step-12-ui-smoke-checklist.md`.
- `docs/archive/step-12-opencode-implementation-plan.md` (this doc) closed out with per-stage deltas.

**Verification:** Smoke checklist passes. Doc cross-refs intact. Step 12 row added to STATUS+PLAN.

**Dependencies:** Stages 5-6.

### Stage table summary

| Stage | Layer | New Rust tests | New TS tests | Person-days | Demoable? |
|---|---|---|---|---|---|
| 1 | BE | ~25-30 | 0 | 2-3 | dev-only probe |
| 2 | BE | ~20 | ~5 | 2-3 | live model list in dev tools |
| 3 | Both | ~10 | ~15 | 3-5 | **first end-to-end chat** |
| 4 | FE | 0 | ~25 | 3-5 | descriptor chips render |
| 5 | FE | 0 | ~30-40 | 4-6 | new picker visible |
| 6 | FE | 0 | ~15 | 1-2 | favorites + shortcuts |
| 7 | Both | ~5 | ~5 | 2-3 | shipped |

**Cumulative**: ~60 Rust tests, ~95-105 TS tests, **17-27 person-days** (~3.5-5.5 weeks of focused work).

---

## 7. Risks

### 7.1 OpenCode SDK / API stability

**Risk:** OpenCode is pre-1.0 in spirit even at v1.14.x. The HTTP API is internal to the SDK and may break between minor versions.

**Mitigation:** Pin the minimum version (`>= 1.14.19`, matching the reference impl's pin). Surface "incompatible OpenCode version" gracefully. Watch upstream changelog. If we go option (a) Rust-direct-HTTP, freeze the request shapes against a specific version and test before bumping.

### 7.2 OpenCode CLI not on PATH

**Risk:** Most users don't have `opencode` installed.

**Mitigation:** Probe at startup. Show a stable empty-state in the picker rail with install instructions. Do NOT block the rest of the chat surface on OpenCode being absent.

### 7.3 Upstream API key management UX

**Risk:** Users expect to add their OpenAI/Anthropic key in Codemux's Settings. OpenCode requires editing `~/.config/opencode/` or running `opencode auth login`.

**Mitigation:** Lean in. Document that "OpenCode credentials are managed by OpenCode itself — run `opencode auth login`." Do not duplicate the credential UI. Codemux only owns the relationship to OpenCode-the-server; OpenCode owns the relationship to upstream APIs. This is the same boundary the reference impl drew.

### 7.4 HTTP+async-iterable streaming protocol

**Risk:** Translating OpenCode's event union into Codemux's `ProviderRuntimeEvent` shape may have lossy edges (tool-call ordering, partial-message coalescing, permission-replied races).

**Mitigation:** Use the reference impl's `OpenCodeAdapter.ts:656-949` event handler as the porting source — it's shipped behaviour. Stage 3 includes a fixture-replay test harness for the event union.

### 7.5 MCP integration

**Risk:** Step 9's MCP runtime injects tools into Claude via `Options.mcpServers["codemux"] = { type: "sdk", instance }`. Codex is getting an HTTP-MCP-gateway approach in Step 11. **OpenCode handles MCP servers internally via its own config (`~/.config/opencode/`).** Codemux's MCP runtime doesn't reach into OpenCode's tool surface.

**Mitigation:** Accept the boundary. Document "OpenCode-mediated tool calls don't appear in Codemux's MCP toggles — toggle them via `opencode` itself or its config." Surface a Settings disclaimer on the OpenCode row of the MCP servers section.

**Alternative:** Codemux could write to OpenCode's MCP config the same way Codex Step 9 plan considered writing to `~/.codex/config.toml`. Defer to v2 — same reasons as Codex deferral.

### 7.6 Skill discovery

**Risk:** Step 7 already supports OpenCode skills (`SkillProvider::Opencode` enum at `src-tauri/src/skills/mod.rs:17-24`), but OpenCode's CLI consumes skills differently from Claude/Codex. Whether the skills the scanner discovers actually load when an OpenCode chat runs depends on OpenCode's behavior.

**Mitigation:** Needs investigation in Stage 2 or 3. If OpenCode reads `~/.codemux/skills/` automatically, no work. If it requires a path in OpenCode's config, document the `--config-content` injection pattern. **Marking this as "needs more investigation" — concrete answer requires running real OpenCode chats with skills.**

### 7.7 Permission system integration

**Risk:** Codemux's approval flow expects `RequestOpened { kind: command|file-read|file-change|other }`. OpenCode emits `permission.asked` events. Mapping is straightforward in principle but the reference impl's adapter has 70+ lines of permission handling logic — there's nuance.

**Mitigation:** Port the reference impl's permission handler shape (`OpenCodeAdapter.ts:770-792`) as a starting point. Add unit tests against fixture event streams.

### 7.8 Sidecar architecture drift if option (b) is chosen

**Risk:** If we end up needing a Node sidecar for OpenCode, we now ship two sidecar binaries (`claude-agent` + a hypothetical `opencode-bridge`). Build pipelines, packaging, signing, auto-update — all double.

**Mitigation:** Strongly prefer option (a) Rust-direct-HTTP. Only fall back to (b) if Stage 1 reveals the SDK does something non-trivial that's hard to port (e.g., custom WebSocket framing, OAuth token refresh).

### 7.9 Picker rewrite scope creep

**Risk:** The 2-column picker is a substantial UI rewrite that touches the most-used surface in Codemux's chat UI. Regression risk is high.

**Mitigation:** Feature-flag the new picker. Keep the old single-dropdown ModelPicker behind `useNewModelPicker` until Stage 5 stabilizes. Hand-driven UI smoke checklist before flipping the flag.

---

## 8. Effort estimate

### 8.1 Research already invested

- Reference clone + 3 parallel investigation agents: **~2 hours**.
- Codemux state mapping agent + this scoping doc: **~2 hours**.
- **Total research: ~4 hours.** Locked in `docs/research/step-12-opencode-research.md` and this doc.

### 8.2 Implementation by stage

| Stage | Person-days |
|---|---|
| 1 (driver scaffold) | 2-3 |
| 2 (live harvest) | 2-3 |
| 3 (sub_provider + first chat) | 3-5 |
| 4 (descriptor renderer) | 3-5 |
| 5 (picker rebuild) | 4-6 |
| 6 (favorites + shortcuts) | 1-2 |
| 7 (polish + docs) | 2-3 |
| **Total** | **17-27 person-days** |

At a single-engineer cadence with documentation, tests, and review: **3.5 to 5.5 weeks elapsed**.

### 8.3 Comparison to recent steps

| Step | Stages | Elapsed | Test count delta |
|---|---|---|---|
| Step 9 (MCP runtime) | 6 | (in flight, multi-week) | TBD |
| Step 10 (skills sync) | 6 | ~2 weeks | +197 tests across server/Rust/TS |
| **Step 12 (this)** | **7** | **3.5-5.5 weeks** | **+155-165 new tests** |

Step 12 is **larger than Step 10**. It's comparable to Step 9 in complexity but with additional UI surface area. The novelty risk is concentrated in Stages 1-2 (HTTP transport for OpenCode) and Stage 5 (picker rebuild).

### 8.4 Reduced-scope path

If we ship **Stages 1-3 only** (Stage 3.5 alt path in §6): **7-11 person-days, ~1.5-2.5 weeks**. Delivers OpenCode chat working end-to-end in the existing one-dropdown picker, with sub-provider labels but no favorites, no keyboard shortcuts, no rail. This is the 80%-of-the-value cut. Stages 4-7 then become a separate "Step 12.5 — picker UX" feature that can ship independently.

---

## 9. Strategic question

### 9.1 What does Step 12 unlock?

- **Multi-LLM chat in one ADE.** Today Codemux's GUI chat is Claude-only (Codex chat is wired but recently hidden by `c81119f`). Step 12 makes Codemux a true multi-provider ADE without forcing the user to leave the GUI for Gemini/Qwen/local-Llama.
- **OpenCode upstream's continuous model-list growth** lands in Codemux for free. New provider added to OpenCode → appears in the picker.
- **Marketing posture.** "Codemux supports 30+ models via OpenCode" is meaningful in a market where Cursor and Claude Desktop are single-vendor.

### 9.2 Who is the user?

Honest read:

- **Power users with multiple API keys** (OpenAI + Anthropic + Google) who want one-pane chat that switches between them. Smallish niche; they're already comfortable with terminal `opencode` and don't need a GUI shell.
- **Cost-sensitive users** who want to route easy turns to cheap models (Haiku, Gemini Flash) and hard turns to Opus. Real audience but they want intelligent routing, which Step 12 doesn't add.
- **Local-LLM enthusiasts** running Ollama or LM Studio. OpenCode supports them. This is a real niche but rapidly served by other tools too.

The user explicitly said "you can bring your own providers" and "they always add new free models." That suggests the user is targeting the **breadth-of-models** thesis, not solving a specific incumbent's pain. Strong product instinct, but the pull is on the "I want options" side, not "I'm blocked today."

### 9.3 Smaller version that captures 80% of the value

**Yes — Stage 3.5 alt path** (§6). Ship Stages 1-3 only. ~1.5-2.5 weeks elapsed. Users get OpenCode in the existing picker, see sub_provider labels (`Codex Sonnet 4.6 · Anthropic`), and have a working chat. Skip the picker rewrite, skip favorites, skip shortcuts. This captures the "more models" win without the UI ambition.

If Stage 3.5 ships and the OpenCode picker pull is real, Stages 4-7 become an obvious follow-up. If not, you've spent a week, not a month.

### 9.4 Should this come BEFORE or AFTER Step 10.5 + Step 11?

**After.** Honest ordering:

1. **Step 10.5 — project-scoped skills sync.** ~3-5 days per `docs/core/PLAN.md`. Additive migration, ships value to existing paying users. Tiny risk.
2. **Step 11 — Codex MCP via HTTP gateway.** Per `docs/research/step-9-codex-mcp-spike.md`, ~40-50% of Step 9 Stage 3's complexity. Unblocks the existing Codex-chat surface for MCP, which is a discoverability gap users hit every day they switch to Codex.
3. **Step 12 — OpenCode.** Bigger lift, broadens the funnel rather than unblocking existing flows.

Step 12 is genuinely ambitious and a strategic bet. Step 11 is duty work that closes an open gap. Step 10.5 is a quick value ship. Doing them in 10.5 → 11 → 12 order respects compound-interest math: each step makes the next's user base wider.

### 9.5 Real read

If you GO Step 12 now, do it **as Stage 3.5 alt path first**. Get OpenCode chat working end-to-end in the existing UI in ~2 weeks. Use the next 2 weeks of dogfood feedback to decide whether the picker rewrite is worth 3 more weeks. If the answer is "no, I just wanted to chat with Gemini," you've saved 60% of the budget and shipped 80% of the value.

If you DEFER Step 12, the cost is approximately zero — the reference impl isn't a moat, OpenCode isn't going anywhere, and the architecture work in Stages 1-3 can land any time the market signals it's needed.

**Recommended decision: DEFER, do Step 10.5 → Step 11 first, then revisit Step 12 as Stage-3.5-only ship.**

---

## Appendix — open questions for sign-off

1. Pick option (a) Rust-direct-HTTP vs option (b) Node sidecar for OpenCode's transport — committed in Stage 1 after probing the SDK's HTTP surface.
2. Lock keybinding namespace for picker shortcuts (`Ctrl+Shift+1..5` vs in-popover-only vs other) — Stage 6.
3. Confirm `instance_id === provider` shim in payloads for forward-compat — yes/no — Stage 1.
4. Confirm minimum OpenCode version pin — `>= 1.14.19` (matches the reference impl) unless dogfooding reveals breakage.
5. Skill discovery for OpenCode chats — investigate in Stage 2 or 3. May need a Step 12.5 follow-up.
6. MCP integration with OpenCode — defer to v2 explicitly, or build OpenCode-config-rewriter analog of the Codex Step 11 work? Recommendation: defer.
