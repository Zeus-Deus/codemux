# Step 12 Research Deliverable — OpenCode as a Third Chat Provider

> **RESEARCH NOTE.** Pre-implementation research or a spike. Some conclusions
> here were later revised or reversed by what actually shipped — read it as
> reasoning history, never as current behavior. Current truth lives in
> `docs/features/*`.

- Purpose: Locked research notes on how a reference multi-provider client implements its chat surface, with concrete file:line evidence, captured before scoping the Codemux build.
- Audience: Anyone reviewing or implementing Step 12.
- Authority: Read-only research checkpoint. Implementation decisions live in `docs/archive/step-12-opencode-implementation-plan.md`.
- Update when: A factual claim here is wrong; otherwise leave alone.
- Read next: `docs/archive/step-12-opencode-implementation-plan.md`.

Research was performed against a shallow clone of a reference multi-provider client at `/tmp/<reference>` (cloned 2026-04-30). Three parallel agent investigations covered model registry + provider integration, picker UI, and capability propagation. Findings reconciled below.

---

## 1. Architecture summary — three concept layers

The reference impl separates the chat-provider problem into three concepts. Codemux's `AgentChatProviderKind` collapses two of them.

| Reference layer | Codemux today |
|---|---|
| **`ProviderDriverKind`** — slug selecting backend code path: `"codex"`, `"claudeAgent"`, `"opencode"`, `"cursor"`. Open / extensible. | `AgentChatProviderKind` enum, hardcoded to `"claude" \| "codex"`. |
| **`ProviderInstanceId`** — user-named instance routing turns. One driver can have many instances with different creds/accent colors (`codex_work`, `claude_personal`). | Not modeled. One config per driver. |
| **`ServerProviderModel`** — `{ slug, name, shortName, subProvider, isCustom, capabilities }` returned by each driver after probing its CLI/SDK at startup. | `ChatModelInfo` is similar shape but lacks `subProvider`. |

Definitions:

- `ProviderDriverKind` and `DEFAULT_MODEL_BY_PROVIDER` at `/tmp/<reference>/packages/contracts/src/model.ts:128-201`.
- `ProviderInstanceConfig` envelope (driver, displayName, accentColor, environment, enabled, opaque driver-specific config) at `/tmp/<reference>/packages/contracts/src/providerInstance.ts:112-149`.
- `ServerProviderModel` at `/tmp/<reference>/packages/contracts/src/server.ts:54-62`.
- `ProviderInstanceRegistry` (Effect Service) routes `(instanceId) → ProviderInstance.adapter` and owns the lifecycle of each spawned driver. `/tmp/<reference>/apps/server/src/provider/Services/ProviderInstanceRegistry.ts:1-85`.

Each `ProviderInstance` carries `{ instanceId, driverKind, adapter, snapshot, textGeneration }`. The adapter is the per-driver RPC client.

---

## 2. Model lists are runtime-discovered, not hardcoded

Each driver probes its CLI/SDK at startup and emits `ServerProviderModel[]`. The web UI reads them straight out of the registry snapshot — there is no hardcoded model table.

For Claude (`/tmp/<reference>/apps/server/src/provider/Layers/ClaudeProvider.ts`), the per-model capability blob is hand-curated server-side and includes per-version reasoning levels (low/medium/high/xhigh/max/ultrathink) plus context window options. Claude Opus 4.7 supports the full `xhigh`/`ultrathink` ladder and 1M context, Sonnet 4.6 doesn't.

For OpenCode, the model list is fetched live via the official SDK — see §3 below. New OpenCode upstream models appear in the picker without code changes.

Capability descriptors are typed per-model:

```ts
capabilities: {
  optionDescriptors: [
    { id: "effort", type: "select", options: [...], isDefault: "high" },
    { id: "contextWindow", type: "select", options: [{ value: "200k", isDefault: true }, { value: "1m" }] },
    { id: "fastMode", type: "boolean" },
    { id: "variant", type: "select", options: [...] },  // OpenCode-specific
    { id: "agent", type: "select", options: [...] },    // OpenCode-specific
  ]
}
```

Definitions:

- `ModelCapabilities` at `/tmp/<reference>/packages/contracts/src/model.ts:123-126`.
- Claude per-model definitions at `/tmp/<reference>/apps/server/src/provider/Layers/ClaudeProvider.ts:29-161` (Opus 4.7 / 4.6 / Sonnet 4.6 examples).
- Web-side capability lookup `getProviderModelCapabilities(models, model, provider)` at `/tmp/<reference>/apps/web/src/providerModels.ts:80-87`.
- Web-side validation/clamp `resolveDescriptorChoiceValue(descriptor, raw)` at `/tmp/<reference>/packages/shared/src/model.ts:80-101` — silently resets to descriptor default if the prior selection isn't in the new model's option set.

This validation primitive maps cleanly onto Codemux's existing `resolveEffort` (`src/lib/agent-chat/model-resolution.ts:39`) and `resolveContextWindow` (line 88) — same shape, same semantics.

---

## 3. OpenCode driver specifics — HTTP SDK, not JSON-RPC stdio

This is the biggest architectural divergence from Claude/Codex/Cursor and the most important fact for Codemux scoping.

### SDK and version

- `@opencode-ai/sdk: ^1.3.15`, v2 API surface, dependency in `/tmp/<reference>/apps/server/package.json:32`.
- Client construction: `createOpencodeClient({ baseUrl, directory, headers?, throwOnError: true })` at `/tmp/<reference>/apps/server/src/provider/opencodeRuntime.ts:491-503`.

### Local server lifecycle

The reference impl spawns the `opencode` binary as a **local HTTP server**, not as a JSON-RPC subprocess.

```ts
// /tmp/<reference>/apps/server/src/provider/opencodeRuntime.ts:331
const args = ["serve", `--hostname=${hostname}`, `--port=${port}`];

// :333-341
const child = yield* spawner.spawn(
  ChildProcess.make(input.binaryPath, args, {
    detached: process.platform !== "win32",
    env: {
      ...(input.environment ?? process.env),
      OPENCODE_CONFIG_CONTENT: JSON.stringify({}),
    },
  }),
)
```

Hostname `127.0.0.1`, port auto-discovered from a free-port helper. After spawn the reference impl reads stdout and waits for the line `"opencode server listening on http://..."`, regex-extracts the URL (lines 378-386), then drives the SDK against that URL.

Lifetime is bound to an Effect `Scope`; closing the scope sends SIGTERM, sleeps 1s, then SIGKILL (lines 355-372).

### Auth

- Server-level: HTTP Basic header `Authorization: Basic base64("opencode:${serverPassword}")` (line 498). Setting stored in `OpenCodeSettings.serverPassword`.
- Upstream provider keys (OpenAI, Anthropic, OpenRouter, etc.): **not stored by the reference impl**. OpenCode reads them from its own `~/.config/opencode/` config and env vars. The reference impl never sees them.

### Model harvest

```ts
// /tmp/<reference>/apps/server/src/provider/opencodeRuntime.ts:505-524
const loadProviders = (client: OpencodeClient) =>
  runOpenCodeSdk("provider.list", () => client.provider.list())…

const loadAgents = (client: OpencodeClient) =>
  runOpenCodeSdk("app.agents", () => client.app.agents())…

// :527
Effect.all([loadProviders(client), loadAgents(client)], { concurrency: "unbounded" })
```

Single one-shot `await`. Result is filtered to providers in `connected: Set<string>` and flattened in `flattenOpenCodeModels` at `/tmp/<reference>/apps/server/src/provider/Layers/OpenCodeProvider.ts:218-249` to `slug = "${providerId}/${modelId}"` (e.g., `openai/gpt-5`, `anthropic/claude-sonnet-4-6`). `subProvider` is set to the upstream provider name.

Per-model capability descriptors come from `model.variants` (effort levels) plus the agents list, transformed by `openCodeCapabilitiesForModel` at lines 169-216 (`titleCaseSlug`, default "medium" for OpenAI/OpenCode, "high" for Anthropic/Google).

### Turn dispatch and event stream

```ts
// /tmp/<reference>/apps/server/src/provider/Layers/OpenCodeAdapter.ts:1207-1214
yield* runOpenCodeSdk("session.promptAsync", () =>
  context.client.session.promptAsync({
    sessionID: context.openCodeSessionId,
    model: parsedModel,
    ...(context.activeAgent ? { agent: context.activeAgent } : {}),
    ...(context.activeVariant ? { variant: context.activeVariant } : {}),
    parts: [...(text ? [{ type: "text" as const, text }] : []), ...fileParts],
  }),
)
```

`promptAsync` is fire-and-forget — it returns an ACK immediately. The streaming response arrives on a separate **async iterable subscription**:

```ts
// :952-1014
const subscription = yield* runOpenCodeSdk("event.subscribe", () =>
  context.client.event.subscribe(undefined, { signal: eventsAbortController.signal }),
)
yield* Stream.fromAsyncIterable(subscription.stream, ...)
  .pipe(Stream.runForEach((event) => handleSubscribedEvent(context, event)))
```

Event types include `message.updated`, `message.removed`, `message.part.delta`, `message.part.updated`, `permission.asked`, `permission.replied`, `question.asked`, `question.replied`, `question.rejected`, `session.status` (busy/idle/retry), `session.error` — all dispatched by `handleSubscribedEvent` at lines 656-949.

### Error mapping (UX matters)

`OpenCodeProvider.ts:64-135` formats common failures: `ENOENT` → "OpenCode CLI is not installed or not on PATH"; 401/403 → "OpenCode server rejected authentication"; `ECONNREFUSED`/timeout → "Couldn't reach the configured OpenCode server"; macOS quarantine → suggests `xattr -d com.apple.quarantine`. Minimum version `1.14.19` enforced (lines 375-402).

Probe state surfaces to the UI as `{ installed, version, status: "ready"|"warning", auth, message: "X upstream providers connected" }` at lines 450-470.

### Compatibility with a JSON-RPC-stdio child pattern

**Not compatible.** OpenCode is an HTTP server, the SDK is the canonical client, and events arrive on an async iterable (HTTP long-poll/WebSocket internally). A JsonRpcChild abstraction that pipes line-delimited JSON over stdin/stdout cannot wrap this without:

- Custom port-discovery from stdout
- A thin HTTP+SDK wrapper replacing the JSON-RPC pipe layer
- Re-shaping the async iterable into JSON-RPC server-sent notifications
- Loss of the SDK's type safety

Cleaner option: model OpenCode as a sibling driver to the existing JsonRpcChild adapters, with its own lifecycle. See implementation plan §1 and §2.

---

## 4. Picker UI — two-column rail with search-collapses-grouping

### Layout

`ModelPickerContent.tsx` (`/tmp/<reference>/apps/web/src/components/chat/ModelPickerContent.tsx:518-643`) uses a flexbox: optional 50px left rail (`ModelPickerSidebar`) + main column (search input + list + empty state). When a single instance is locked, the layout collapses to a single column.

Active provider in the rail is highlighted via a right-edge vertical bar (`ModelPickerSidebar.tsx:35`):

```tsx
const SELECTED_INDICATOR_CLASS =
  "pointer-events-none absolute -right-1 top-1/2 z-10 h-5 w-0.5 -translate-y-1/2 rounded-l-full bg-primary";
```

### Search

Fuzzy + tokenized. Tokens split on whitespace, scored independently per field (model name, shortName, subProvider, driverKind, instance display name); minimum across fields combined (`modelPickerSearch.ts:55-83`). Searchable text built by `buildModelPickerSearchText`. **Once the user types, the sidebar disappears and results are flat-ranked across providers** — provider grouping does NOT survive search.

Favorites get a **score boost of -24** (lower wins) so they land first in unsorted views (`modelPickerSearch.ts:18,82`). Not a separate section.

### Keyboard shortcuts

`Ctrl+1`…`Ctrl+5` are **slot-based on visible filtered position**, not bound to specific models. After filtering, each visible model at index 0…4 maps to a jump command (`ModelPickerContent.tsx:385-398`). If the user searches "haiku", `Ctrl+1` jumps to the first matching Haiku result. Implementation hooks into a global keybinding context via `resolveShortcutCommand()`.

### Favorites

Stored in user settings (Zustand-backed) as `Array<{ provider: ProviderInstanceId, model: string }>` with synchronous toggle (`ModelPickerContent.tsx:352-364`). Likely server-synced.

### Composer footer chips

The composer footer is a horizontal-scroll container (`ChatComposer.tsx:1999-2062`) with separate chips:

- **Model picker** — `ProviderModelPicker` triggers the full sidebar+search popup.
- **Reasoning** — `TraitsPicker` with the model's first select descriptor (effort levels). Conditional on the model exposing one. Label like "High", "Max", "Extra High · 1M".
- **Build/Plan toggle** — text switch via `ProviderInteractionMode = "default" | "plan"` (`/tmp/<reference>/packages/contracts/src/orchestration.ts`). Persisted per-thread, not per-model.
- **Permissions** — `Select` over `RuntimeMode = "approval-required" | "auto-accept-edits" | "full-access"` (same orchestration.ts). Per-thread.

When the model changes, `getComposerProviderState()` (`composerProviderState.tsx:47-73`) rederives `modelOptionsForDispatch`; the chips re-render with the new descriptor's options. Selections that don't match the new option set silently reset to the descriptor default — `resolveDescriptorChoiceValue` in `packages/shared/src/model.ts:80-101`.

### Provider rail icons & badges

`PROVIDER_ICON_BY_PROVIDER` in `/tmp/<reference>/apps/web/src/components/chat/providerIconUtils.ts:5-10` maps driver kind to an `Icon` component. Per-instance accent badges (initials like "CP" for "Codex Personal") render in the rail when multiple instances share a driver (`ProviderInstanceIcon.tsx:57-70`). "New" sparkle and "Soon" clock badges decorate the rail for built-ins and coming-soon providers (`ModelPickerSidebar.tsx:38-39, 165-169, 213-214`).

---

## 5. Backend integration

Every existing driver except OpenCode wraps a child CLI with **ACP (Agent Client Protocol)** over JSON-RPC 2.0 stdio. ACP types live in `/tmp/<reference>/packages/effect-acp/src/protocol.ts`. Drivers spawn `claude-agent`, `codex app-server`, or the Cursor CLI and speak JSON-RPC.

OpenCode breaks the pattern as documented in §3 — HTTP server + SDK.

The unifying interface is `ProviderAdapterShape<E>` on each `ProviderInstance.adapter`. Turns route by `instanceId` to `adapter.startTextGenerationTurn(...)`, which streams events back as `RuntimeEventV2` regardless of the underlying transport.

This is the key abstraction lesson: the adapter shape is broad enough to hide the JSON-RPC vs HTTP/SDK divergence. Codemux's `AgentProvider` trait at `src-tauri/src/agent_provider/provider.rs:29-93` is similarly broad — it talks at the level of `start_session`, `send_turn`, `event_stream()` — so OpenCode can be a third `AgentProvider` impl without forcing JsonRpcChild downward.

---

## 6. Files to reference for Codemux implementation

Most useful single files in `/tmp/<reference>/`:

- `packages/contracts/src/providerInstance.ts` — instance/driver separation contract.
- `packages/contracts/src/model.ts` — capabilities + driver kind + default model table.
- `packages/contracts/src/server.ts` — `ServerProviderModel` shape.
- `apps/server/src/provider/Services/ProviderInstanceRegistry.ts` — registry lifecycle.
- `apps/server/src/provider/opencodeRuntime.ts` — OpenCode server spawn + SDK client + model harvest.
- `apps/server/src/provider/Layers/OpenCodeProvider.ts` — per-model capability mapping, error formatting, probe state.
- `apps/server/src/provider/Layers/OpenCodeAdapter.ts` — turn dispatch, event subscription, scope-bound shutdown.
- `apps/web/src/components/chat/ModelPickerContent.tsx` — full picker UI with rail + search + shortcuts.
- `apps/web/src/components/chat/TraitsPicker.tsx` — generic capability-descriptor chip renderer.
- `apps/web/src/components/chat/composerProviderState.tsx` — per-pane state derivation.
- `packages/shared/src/model.ts` — capability resolution / clamping primitives.

---

## 7. Notable mismatches with current Codemux assumptions

- Codemux assumes JsonRpcChild stdio is the universal sidecar shape. **OpenCode breaks that assumption.** Plan must include a non-JsonRpcChild lifecycle path.
- Codemux's capability resolution (`src/lib/agent-chat/model-resolution.ts`) is already shaped like the reference impl's (effort levels, prompt-injected efforts, context window options, default fallback) — no refactor required at that layer.
- Codemux's chips are not yet rendered by a generic `TraitsPicker`-style descriptor renderer — they are bespoke (`ReasoningPicker.tsx`, `PermissionModePicker.tsx`, `ModePill.tsx`, `ModelPicker.tsx`). Generic-ifying is optional for a 3rd provider but useful when descriptors diverge (OpenCode adds `variant` and `agent` selects that don't exist on Claude/Codex).
- The reference impl's "skill" concept is not the same as Codemux's. The reference clone has `providerSkillSearch.ts` referring to a different feature (system-prompt skills tagged to providers); the Codemux skills system (Step 7/10) is closer to Anthropic's `~/.claude/skills/`.
