# Multi-Provider Chat

- Purpose: Describe how Codemux's chat picker supports Claude, Codex, and OpenCode side-by-side after Step 12.
- Audience: Anyone working on the chat composer, the model picker, or adding a new chat provider.
- Authority: Canonical feature-level reality doc for the picker + capability harvest stack.
- Update when: A provider is added/removed, the picker layout changes, the capabilities harvest pipeline changes, or the federation slug shape changes.
- Read next: `docs/features/agent-chat.md`, `docs/research/step-12-opencode-research.md`, `docs/archive/step-12-opencode-implementation-plan.md`, `docs/archive/step-12-ui-smoke-checklist.md`.

## What This Feature Is

The chat composer's model picker drives session-creation across three provider drivers: **Claude** (native via the Claude Agent SDK), **Codex** (native via the `codex app-server` JSON-RPC binary), and **OpenCode** (federated — a single CLI that fans out to OpenAI / Anthropic / OpenRouter / Google / 100+ upstream providers behind one HTTP API). All three render in one popover with a 2-column layout: a 48-pixel provider rail on the left and a searchable model list on the right.

OpenCode is the federated arm: a single rail entry whose flat model list shows entries like `OpenCode · OpenAI` / `OpenCode · Anthropic` for the upstream providers the user has configured credentials for. Disconnected upstreams are filtered out at the data-layer (Stage 3) so the picker stays usable on a fully-configured machine (~5 connected upstreams typically, vs ~110 visible if we showed every provider OpenCode knows about).

## Current Model

### Provider drivers

| Driver | Transport | Adapter | Capabilities |
|---|---|---|---|
| Claude | JSON-RPC stdio (`claude-agent` sidecar bin) | `src-tauri/src/agent_provider/claude/mod.rs` | **Live harvest** via the sidecar's `list-models` RPC (SDK `supportedModels()`), merged with hand-maintained per-id metadata in `capabilities.rs`; `/v1/models` (API-key) and `claude_fallback_capabilities()` as fallbacks. Handles alias ids (`default`, `sonnet`, `haiku`) and pinned-window ids (`claude-fable-5[1m]`) via family inference. **Alias folding + label promotion:** `dedupe_default_alias` drops the `default` row whenever a concrete twin exists — the twin absorbs it, moves to index 0, and takes a `"Recommended · …"` description prefix — and alias/nickname rows render promoted concrete names ("Claude Opus 4.8") parsed from the live version-bearing description, falling back to the maintained catalog. So index 0 is the recommended default on the fallback path, and the *folded twin* leads on the SDK-harvest path. For persistence compat, `selectModel` (`provider-capabilities-store.ts`) deliberately breaks strict-id matching for the literal id `"default"` and resolves it to `models[0]`, so drafts and threads persisted before the fold still resolve a real model (and so still get the right trigger label, tooltip, active-row highlight, and reasoning/speed picker availability); maintained entries of families the curated roster omits (currently Opus) are appended so they stay selectable via `--model`. Ultrathink prompt-injection effort level, 200k/1M context-window picker on bare flagship/Sonnet ids. |
| Codex | JSON-RPC stdio (`codex app-server`) | `src-tauri/src/agent_provider/codex/mod.rs` | **Live harvest** via a short-lived `codex app-server` child (`initialize` → `account/read` → `model/list`); unauthenticated users get a clean error state, no static fallback. Auth classification follows the app-server contract: `requiresOpenaiAuth` describes the active model provider, so login is missing only when that field is true **and** `account` is null. Sandbox-policy permission modes, per-turn effort. |
| OpenCode | HTTP (Rust-direct `reqwest` against a managed `opencode serve` child) | `src-tauri/src/agent_provider/opencode/{server,manager,client,capabilities}.rs` | **Live harvest** at `harvest_opencode_capabilities` — calls `GET /provider`, flattens to per-model `ChatModelInfo` entries with `sub_provider` populated and connected-only filter applied. |

OpenCode's server lifecycle is owned by `OpenCodeServerManager` (singleton, `kill_on_drop` on the underlying `tokio::process::Child`, idempotent `ensure_running()`). The server is spawned lazily on the first capabilities refresh, lives for the duration of the Codemux session, and is killed when the manager state is dropped. Generated 32-char `OPENCODE_SERVER_PASSWORD` is exported into the child's env at spawn, then attached as HTTP Basic auth (`opencode:<password>`) on every request.

### Capabilities surface

`ChatModelInfo` is the canonical chat-side per-model record (`src-tauri/src/agent_provider/types.rs`). Step 12 added one field: `sub_provider: Option<String>`. For Claude/Codex it's `None` (the driver IS the provider); for OpenCode it carries the upstream provider id (`"openai"`, `"anthropic"`, `"openrouter"`, …). Slug shape for OpenCode is `${provider_id}/${model_id}` (e.g. `openai/gpt-5`, `anthropic/claude-sonnet-4-6`) so a model selection round-trips deterministically.

The `list_chat_provider_capabilities` Tauri command dispatches by `ProviderKind`: Claude uses its live-sidecar/API cascade with a maintained fallback, Codex performs a cached live app-server harvest, and OpenCode runs its live harvest through `OpenCodeServerManager`. Failures fall through to the matching frontend store error slot — Codex errors retain typed prefixes so the picker can distinguish missing CLI, missing login, and harvest failures.

### Codex authentication classification

`account/read` returns two independent facts. `account` is the current login (or null), while `requiresOpenaiAuth` says whether the active model provider needs OpenAI credentials. A normal logged-in ChatGPT account therefore returns an account object **and** `requiresOpenaiAuth: true`; the latter is not a login-status boolean. Both capability harvest and session startup use the same truth table:

| `account` | `requiresOpenaiAuth` | Result |
|---|---:|---|
| present | `true` or `false` | Ready; a usable login/credential is present |
| null | `true` | Not authenticated; show the `codex login` hint |
| null | `false` | Ready; the local/custom provider does not require OpenAI login |

The separate one-shot CLI probe uses the current `codex login status` command. Regression coverage drives the fake app-server with the real logged-in shape (`account` present plus `requiresOpenaiAuth: true`) through both capability harvesting and session startup.

The frontend `provider-capabilities-store` (Zustand) has one slot per driver (`claude`, `codex`, `opencode`) plus matching `*Error` slots. `refreshAll` fires the three refreshes in parallel via `Promise.all`; each `refresh(provider)` swallows its own error so a single failure doesn't block the other slots. `selectCapabilities(state, kind)` is an exhaustive switch — adding a fourth provider is a TypeScript error on day one.

### Picker UI

`MultiProviderModelPicker` is the Stage 4 surface (`src/components/chat/pickers/MultiProviderModelPicker.tsx`). One Popover, two columns:

- **Provider rail (48px)**: icon-only buttons, one per driver. Active provider gets a 2px-wide vertical bar at the right edge as the selected indicator. Tooltip on hover.
- **Search + model list**: cmdk `Command` with `shouldFilter={false}` (we own the filter via `matchesQuery`); typing in the search input collapses provider grouping and returns a flat result list across all three drivers. Clearing search snaps back to "show only the rail-selected provider's models." Empty states cover (a) no-match-for-query, (b) OpenCode-not-installed, (c) OpenCode-installed-but-no-connected-upstreams.

Pane creation threads `provider: AgentChatProviderKind | null` through `agentChatCreatePane`. Both draft and live-pane pickers emit `(provider, model)` as one atomic selection. Drafts store the selected provider, model, and provider-specific defaults before launch. A live pane stops its previous adapter, starts the selected provider on the same Codemux thread (preserving the visible transcript), clears the provider-native resume cursor, and persists the pane's provider/thread binding together. If the new adapter cannot start, Codemux best-effort restores the previous provider session.

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
- `DraftChatSurface` uses the same unified provider/model picker as a live pane, so the first session launches directly on the selected adapter.

## Current Constraints

- **Single instance per provider.** A user with multiple Codex accounts or multiple OpenCode connections sees them collapsed under one rail entry. The `ProviderInstanceId` shim is in place (`src-tauri/src/agent_provider/instance.rs`) for forward-compat — multi-instance lifts the singleton without changing the wire format.
- **Row-jump shortcuts inside the picker.** While the popover is open, `Ctrl+1..9` (`Cmd+1..9` on macOS) activates the Nth row of the *filtered* list. A window capture-phase `keydown` listener is registered only while the popover is open and calls `preventDefault` + `stopPropagation`, so there is no collision with the global tab/workspace bindings that own the same chord — the earlier "deferred until we find a non-colliding namespace" constraint was resolved by scoping rather than renaming. Each row renders its own kbd chip (`JUMP_MOD_LABEL`).
- **No "favorites only" filter** in the rail — favorites bubble up via sort, not via a dedicated filter mode.
- **No favorites sync across devices.** Codemux doesn't sync UI prefs; favorites live in `localStorage` only.
- **OpenCode credential management lives in OpenCode.** Codemux never reads or writes upstream API keys (OpenAI / Anthropic / etc.). `opencode auth login` is the one entry point; settings panel only shows the connected/disconnected state.

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
- `src/components/chat/pickers/ModelPicker.tsx` — legacy single-provider picker. No longer used by `DraftChatSurface` (which passes `showProviderPicker={true}`); retained for any surface that opts out of the unified picker.
- `src/components/chat/ComposerFooter.tsx` — switches between `MultiProviderModelPicker` (when `showProviderPicker={true}` — the path both the pane and the draft surface take today) and the legacy `ModelPicker`.
- `src/components/chat/AgentChatPane.tsx:103` — `ENABLE_PROVIDER_PICKER = true` flag.

### Docs

- `docs/research/step-12-opencode-research.md` — locked research notes from a reference multi-provider client.
- `docs/archive/step-12-opencode-implementation-plan.md` — original scoping doc; final-state summary added in Stage 7.
- `docs/archive/step-12-ui-smoke-checklist.md` — operator-driven manual smoke.

## Notes

- **The legacy single-provider `ProviderPicker.tsx` was deleted in Stage 7 cleanup.** Stage 4 replaced it with `MultiProviderModelPicker` and Stage 4's wiring left zero callers; the file was kept around through Stage 6 as a paranoia hedge and removed once dead-code audit confirmed no remaining imports.
- **Multi-instance per provider is planned for v2.** `ProviderInstanceId` already exists; the wire format already serializes as a bare provider slug (`"claude"` / `"codex"` / `"opencode"`). The lift is mostly settings UI + capability-harvest fan-out.
- **Picker keyboard shortcuts shipped** as in-popover-only row jumps (see above); the deferred cross-app namespace question is moot.
