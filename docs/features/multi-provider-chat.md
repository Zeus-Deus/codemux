# Multi-Provider Chat

- Purpose: Describe how Codemux's chat picker supports Claude, Codex, and OpenCode side-by-side after Step 12.
- Audience: Anyone working on the chat composer, the model picker, or adding a new chat provider.
- Authority: Canonical feature-level reality doc for the picker + capability harvest stack.
- Update when: A provider is added/removed, the picker layout changes, the capabilities harvest pipeline changes, or the federation slug shape changes.
- Read next: `docs/features/agent-chat.md`, `docs/plans/step-12-opencode-research.md`, `docs/plans/step-12-opencode-implementation-plan.md`, `docs/plans/step-12-ui-smoke-checklist.md`.

## What This Feature Is

The chat composer's model picker drives session-creation across three provider drivers: **Claude** (native via the Claude Agent SDK), **Codex** (native via the `codex app-server` JSON-RPC binary), and **OpenCode** (federated — a single CLI that fans out to OpenAI / Anthropic / OpenRouter / Google / 100+ upstream providers behind one HTTP API). All three render in one popover with a 2-column layout: a 48-pixel provider rail on the left and a searchable model list on the right.

OpenCode is the federated arm: a single rail entry whose flat model list shows entries like `OpenCode · OpenAI` / `OpenCode · Anthropic` for the upstream providers the user has configured credentials for. Disconnected upstreams are filtered out at the data-layer (Stage 3) so the picker stays usable on a fully-configured machine (~5 connected upstreams typically, vs ~110 visible if we showed every provider OpenCode knows about).

## Current Model

### Provider drivers

| Driver | Transport | Adapter | Capabilities |
|---|---|---|---|
| Claude | JSON-RPC stdio (`claude-agent` sidecar bin) | `src-tauri/src/agent_provider/claude/mod.rs` | **Live harvest** via the sidecar's `list-models` RPC (SDK `supportedModels()`), merged with hand-maintained per-id metadata in `capabilities.rs`; `/v1/models` (API-key) and `claude_fallback_capabilities()` as fallbacks. Handles alias ids (`default`, `sonnet`, `haiku`) and pinned-window ids (`claude-fable-5[1m]`) via family inference; ultrathink prompt-injection effort level, 200k/1M context-window picker on bare flagship/Sonnet ids. |
| Codex | JSON-RPC stdio (`codex app-server`) | `src-tauri/src/agent_provider/codex/mod.rs` | Hand-maintained `codex_fallback_capabilities()` with 4 models, sandbox-policy permission modes, per-turn effort. |
| OpenCode | HTTP (Rust-direct `reqwest` against a managed `opencode serve` child) | `src-tauri/src/agent_provider/opencode/{server,manager,client,capabilities}.rs` | **Live harvest** at `harvest_opencode_capabilities` — calls `GET /provider`, flattens to per-model `ChatModelInfo` entries with `sub_provider` populated and connected-only filter applied. |

OpenCode's server lifecycle is owned by `OpenCodeServerManager` (singleton, `kill_on_drop` on the underlying `tokio::process::Child`, idempotent `ensure_running()`). The server is spawned lazily on the first capabilities refresh, lives for the duration of the Codemux session, and is killed when the manager state is dropped. Generated 32-char `OPENCODE_SERVER_PASSWORD` is exported into the child's env at spawn, then attached as HTTP Basic auth (`opencode:<password>`) on every request.

### Capabilities surface

`ChatModelInfo` is the canonical chat-side per-model record (`src-tauri/src/agent_provider/types.rs`). Step 12 added one field: `sub_provider: Option<String>`. For Claude/Codex it's `None` (the driver IS the provider); for OpenCode it carries the upstream provider id (`"openai"`, `"anthropic"`, `"openrouter"`, …). Slug shape for OpenCode is `${provider_id}/${model_id}` (e.g. `openai/gpt-5`, `anthropic/claude-sonnet-4-6`) so a model selection round-trips deterministically.

The `list_chat_provider_capabilities` Tauri command dispatches by `ProviderKind`: Claude/Codex return their hand-maintained fallback bundles synchronously; OpenCode runs the live harvest through `OpenCodeServerManager`. Failures fall through to error strings on the frontend store's `opencodeError` slot — the Stage-1 placeholder bundle is no longer used.

The frontend `provider-capabilities-store` (Zustand) has one slot per driver (`claude`, `codex`, `opencode`) plus matching `*Error` slots. `refreshAll` fires the three refreshes in parallel via `Promise.all`; each `refresh(provider)` swallows its own error so a single failure doesn't block the other slots. `selectCapabilities(state, kind)` is an exhaustive switch — adding a fourth provider is a TypeScript error on day one.

### Picker UI

`MultiProviderModelPicker` is the Stage 4 surface (`src/components/chat/pickers/MultiProviderModelPicker.tsx`). One Popover, two columns:

- **Provider rail (48px)**: icon-only buttons, one per driver. Active provider gets a 2px-wide vertical bar at the right edge as the selected indicator. Tooltip on hover.
- **Search + model list**: cmdk `Command` with `shouldFilter={false}` (we own the filter via `matchesQuery`); typing in the search input collapses provider grouping and returns a flat result list across all three drivers. Clearing search snaps back to "show only the rail-selected provider's models." Empty states cover (a) no-match-for-query, (b) OpenCode-not-installed, (c) OpenCode-installed-but-no-connected-upstreams.

Pane creation threads `provider: AgentChatProviderKind | null` through `agentChatCreatePane`. The picker swaps a pane's provider mid-conversation by emitting `(provider, model)` to the composer, which routes through `agentChatStartSession` for the new selection.

### Favorites

Stage 6 added a star button on every model row (`src/components/chat/pickers/MultiProviderModelPicker.tsx` + `src/stores/picker-favorites-store.ts`). Storage is a flat `string[]` of `${provider}::${model_id}` keys, persisted by zustand's `persist` middleware under `codemux:picker-favorites:v1` in `localStorage`. Click isolation: the star's `onClick`/`onPointerDown`/`onMouseDown` all `stopPropagation()` so cmdk's mousedown→select pipeline doesn't fire.

Favorites bubble to the top of the visible list — both in the rail-only view AND in cross-provider search results — with insertion order preserved within each group. Cross-provider keys are independent: favoriting `claude::claude-sonnet-4-6` doesn't affect `opencode::anthropic/claude-sonnet-4-6` (different transport, different auth, different feature surface).

Stale favorites for a now-disconnected provider stay in storage and are silently ignored when the model isn't in the current capabilities. Reconnecting the upstream resurfaces the favorite automatically.

## What Works Today

- Three providers visible in one picker; Codex finally exposed (was hidden behind `ENABLE_PROVIDER_PICKER = false` until Stage 4).
- OpenCode model harvest: ~116 providers / ~4,354 models live-fetched on a fully-configured machine; connected-only filter trims to typically 5 providers / ~150 models.
- Search across all three providers with `label` / `id` / `sub_provider` matching.
- Favorites persist across reloads and bubble to the top of search results.
- OpenCode-not-installed empty state (with install hint).
- OpenCode-installed-but-no-connected-providers empty state (with `opencode auth login` hint).
- `kill_on_drop` reaps the OpenCode server on Codemux shutdown — no zombie `opencode serve` processes.
- `DraftChatSurface` (the empty-state composer before a session exists) keeps the legacy single-provider `ModelPicker` so a user without an active pane still picks a Claude model the simple way.

## Current Constraints

- **Single instance per provider.** A user with multiple Codex accounts or multiple OpenCode connections sees them collapsed under one rail entry. The `ProviderInstanceId` shim is in place (`src-tauri/src/agent_provider/instance.rs`) for forward-compat — multi-instance lifts the singleton without changing the wire format.
- **No keyboard shortcuts on the picker.** `Ctrl+1..9` collide with workspace switching. Slot-based jumps are deferred until we decide on a non-colliding namespace (likely `Cmd+Shift+1..N` or in-popover-only).
- **Picker only on active panes.** The empty-state `DraftChatSurface` retains the legacy single-provider `ModelPicker`. Switching providers from a draft requires materialising the chat first.
- **No "favorites only" filter** in the rail — favorites bubble up via sort, not via a dedicated filter mode.
- **No favorites sync across devices.** Codemux doesn't sync UI prefs; favorites live in `localStorage` only.
- **OpenCode credential management lives in OpenCode.** Codemux never reads or writes upstream API keys (OpenAI / Anthropic / etc.). `opencode auth login` is the one entry point; settings panel only shows the connected/disconnected state.
- **OpenFlow's CLI-launcher dropdown carries its own hardcoded model registry** (`src-tauri/src/commands/openflow.rs::claude_default_models` / `codex_default_models`). Pre-existing, separate code path from the chat picker; convergence is tracked as future cleanup.

## Important Touch Points

### Backend

- `src-tauri/src/agent_provider/types.rs` — `ChatModelInfo`, `ProviderKind` (Claude / Codex / OpenCode).
- `src-tauri/src/agent_provider/instance.rs` — `ProviderInstanceId` shim for forward-compat with multi-instance v2.
- `src-tauri/src/agent_provider/claude/capabilities.rs` — Claude fallback bundle.
- `src-tauri/src/agent_provider/codex/capabilities.rs` — Codex fallback bundle.
- `src-tauri/src/agent_provider/opencode/server.rs` — `OpenCodeServer` spawn + ready-banner detection (`"opencode server listening on http://..."`) + `kill_on_drop`.
- `src-tauri/src/agent_provider/opencode/manager.rs` — singleton lifecycle, `ensure_running` idempotency.
- `src-tauri/src/agent_provider/opencode/client.rs` — `reqwest`-based HTTP client, `list_models()` against `GET /provider`, response flattener.
- `src-tauri/src/agent_provider/opencode/capabilities.rs` — `harvest_opencode_capabilities` + `flatten_into_chat_models` (connected-only filter).
- `src-tauri/src/commands/agent_chat.rs` — `list_chat_provider_capabilities` dispatcher; `ProviderRegistry` with three slots.
- `src-tauri/src/commands/opencode.rs` — `opencode_check_availability`, `opencode_ping`, `opencode_list_models`.
- `src-tauri/src/lib.rs` — `OpenCodeServerManager` registered as managed Tauri state.
- `src-tauri/examples/opencode_smoke.rs` — live integration smoke binary.

### Frontend

- `src/tauri/types.ts` — `AgentChatProviderKind`, `ChatModelInfo` (with `sub_provider`), `OpenCodeAvailability`, `OpenCodeProviderEntry`, `OpenCodeModel`.
- `src/tauri/commands.ts` — `opencodeCheckAvailability`, `opencodePing`, `opencodeListModels` wrappers.
- `src/stores/provider-capabilities-store.ts` — three-slot store, exhaustive `selectCapabilities` / `selectError`.
- `src/stores/picker-favorites-store.ts` — `usePickerFavorites` (zustand + persist), `pickerFavoriteKey()` helper.
- `src/components/chat/pickers/MultiProviderModelPicker.tsx` — the picker.
- `src/components/chat/pickers/ModelPicker.tsx` — legacy single-provider picker (kept for `DraftChatSurface`).
- `src/components/chat/ComposerFooter.tsx` — switches between `MultiProviderModelPicker` (when `showProviderPicker={true}`) and the legacy `ModelPicker` (drafts).
- `src/components/chat/AgentChatPane.tsx:103` — `ENABLE_PROVIDER_PICKER = true` flag.

### Docs

- `docs/plans/step-12-opencode-research.md` — locked research notes from a reference multi-provider client.
- `docs/plans/step-12-opencode-implementation-plan.md` — original scoping doc; final-state summary added in Stage 7.
- `docs/plans/step-12-ui-smoke-checklist.md` — operator-driven manual smoke.

## Notes

- **The legacy single-provider `ProviderPicker.tsx` was deleted in Stage 7 cleanup.** Stage 4 replaced it with `MultiProviderModelPicker` and Stage 4's wiring left zero callers; the file was kept around through Stage 6 as a paranoia hedge and removed once dead-code audit confirmed no remaining imports.
- **OpenFlow capabilities convergence is future cleanup, not a regression.** OpenFlow's `list_models_for_tool` Tauri command has its own model registry that pre-dates the agent-chat capabilities harvest. The Codex fallback file already cross-references this duplication (`src-tauri/src/agent_provider/codex/capabilities.rs:33`). Converging would let Codemux drop two parity-by-comment dependencies, but it's a meaningful refactor that touches OpenFlow's CLI-launcher invariants.
- **Multi-instance per provider is planned for v2.** `ProviderInstanceId` already exists; the wire format already serializes as a bare provider slug (`"claude"` / `"codex"` / `"opencode"`). The lift is mostly settings UI + capability-harvest fan-out.
- **Picker keyboard shortcuts deferred.** `Ctrl+1..9` is owned by workspace switching; we'll revisit with a non-colliding namespace.
