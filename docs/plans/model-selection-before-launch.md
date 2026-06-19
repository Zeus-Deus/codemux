# Plan: Model Selection in the Workspace-Creation Dialog

- Purpose: Implementation plan for adding model / reasoning / context
  selection to the CLI launch path (the New Workspace dialog), sourced
  dynamically and extensible to future presets.
- Audience: Whoever implements this feature.
- Authority: Active work plan only, not current truth.
- Update when: Stages, the registry contract, or open questions change.
- Read next: `docs/features/workspace-creation.md`,
  `docs/features/multi-provider-chat.md`, `docs/features/presets.md`

> **Status: SHIPPED in `v0.9.2`.** Model/reasoning/context selection before
> launch is live in the New Workspace dialog and is reused by the structured
> "agent launcher" presets. Current behavior lives in
> `docs/features/workspace-creation.md` and `docs/features/presets.md`; the
> rest of this file is the original implementation plan, kept for history.

## Goal

The New Workspace dialog (`new-workspace-dialog.tsx`) — the default,
non-Beta launch path — lets the user pick an agent **preset** but not a
**model**, **reasoning effort**, or **context window**. To change the
model today the user must launch with the default, stop the agent,
change the model, and resend the prompt. Only the Beta agent-chat
composer exposes model controls.

Add a model selector (a second pill next to the agent pill) to the New
Workspace dialog. It must be:

- **Dynamic** — model lists come from the existing chat capability
  system, not a hardcoded table. OpenCode is live-harvested; Claude /
  Codex use the same maintained bundles the Beta composer already uses.
- **Capability-gated** — the popover shows only the controls a provider
  actually supports (Claude has a context choice; OpenCode does not).
- **Extensible** — a new preset that launches an already-modeled CLI
  picks up the selector automatically, with zero changes to the dialog.

## Status

**Shipped in `v0.9.2`** (PR #104). Model +
reasoning selection ships for all four modeled families in one pass, and
the same `LaunchModelPicker`/`LaunchReasoningPicker` pickers are reused by
the structured "agent launcher" presets (see `docs/features/presets.md`).
The staged plan below is kept for history; the Deferred list at the end of
this section is the remaining future work:

- Backend `agent_capability` module — `ModelSelection`, `detect_family`,
  `apply_model_selection` with strip-then-append override semantics; 19
  unit tests. Threaded through `apply_preset`, `create_worktree_workspace`,
  and both control-socket call sites.
- Frontend — `launch-models.ts` helpers, the `LaunchModelPicker`
  component (adaptive flat/search list, favorites, sub-provider grouping,
  reasoning row), `ui-store` last-pick persistence, wired into
  `new-workspace-dialog.tsx`. 19 frontend unit tests.
- Capability source is the existing `provider-capabilities-store` — no
  fourth registry. Family detection makes new presets zero-touch.
- `npm run verify` green (1724 frontend tests, full Rust suite).
- CLI flag syntax verified against the installed `claude` / `codex` /
  `opencode` / `gemini` binaries.

Also implemented:

- **Context-window control** — Claude-only. The selected model's
  `context_window_options` drive a Context Window row; 1M is encoded as
  the `[1m]` model-id suffix (`apply_model_selection` appends + quotes
  it), matching the Beta chat picker's `resolve_claude_api_model_id`.
- **Fully capability-driven option lists** — reasoning levels and
  context options are read per-model from the live capability bundle
  (`effort_levels`, `context_window_options`), not hardcoded tables.
  `launch-models.ts` keeps only `REASONING_FLAG_FAMILIES` — the
  structural fact of which CLIs have a reasoning flag at all.

SDK-based dynamic harvest (follow-up after `ultracode` wasn't picked up
from the bundled SDK types):

- **Sidecar `list-models` RPC** — new method in
  `sidecar/claude-agent/src/methods/list-models.ts` that opens a
  transient `query()`, awaits `supportedModels()`, and returns the
  array. Works for every Claude Code user regardless of auth, and
  surfaces the deployed CLI's actual effort vocabulary (including
  runtime-only levels like `ultracode` the static SDK type union
  doesn't enumerate).
- **Rust `harvest_via_sidecar`** spawns the sidecar transiently
  (`JsonRpcChild`), sends `list-models`, and merges the live model
  list with the hand-maintained metadata (SDK wins on
  `effort_levels`; maintained fills in context windows + ultrathink +
  the Haiku thinking toggle).
- **Cascade**: `ClaudeCapabilityCache::get_or_harvest` tries sidecar
  → `/v1/models` → maintained. The dispatcher is back to a single
  `match` arm; the env-key check moved into the cache.

Hybrid live harvest (initial follow-up after Opus 4.8 didn't surface):

- **Claude `/v1/models` harvest** — Anthropic's SDK / OAuth path has no
  model-list endpoint, so subscription users still see the
  hand-maintained bundle. With `ANTHROPIC_API_KEY` set the dispatcher
  harvests live and merges the live id list against per-id maintained
  metadata; unknown ids get family-pattern-inferred defaults so a
  brand-new Opus surfaces without a code change. Cached in a
  `ClaudeCapabilityCache` Tauri state, mirroring the Codex pattern.
- **Gemini hybrid** — Gemini isn't a chat provider, so the launch
  picker routes through a new `list_launch_gemini_models` Tauri command
  that does the same hybrid (`GEMINI_API_KEY` → Google's
  `generativelanguage.googleapis.com/v1beta/models`, else maintained).
  Frontend stores it in a tiny `gemini-models-store` with lazy init on
  dialog mount.
- **Maintained Claude list bumped** to include Opus 4.8 as the current
  flagship; Opus 4.7 demoted from flagship description.

Post-review hardening (three fresh-context review agents, all "fixes only"):

- **Data-loss fix** — a remembered reasoning/context pick was dropped and
  re-persisted as `null` if the user submitted before the capability
  harvest resolved. Effective values now pass the stored value through
  while caps are loading; persistence stores the user's literal pick.
- **Codex preset fixed** — `codex --full-auto` → `codex
  --dangerously-bypass-approvals-and-sandbox` (the stale flag now only
  exists on `codex exec`).
- Minor: `strip_codex_reasoning` loop step-back, reset effect keyed on
  primitives (no clobber on a mid-dialog `presets` refresh), control
  socket logs a malformed `model_selection` instead of silently
  dropping it, plus a dialog-level wiring test.

Deferred (see Open Questions / Current Constraints):

- **`project-onboarding.tsx`** — not wired; New Workspace dialog only.
- **MCP `model_selection`** — `worktree_create` / `preset_apply` MCP
  tools don't forward it (control-socket path already accepts it).
- **Capability-provider trait/registry** — detection is currently a
  `match` in `detect_family` rather than a trait object. It already
  satisfies the zero-touch goal; promoting it to a trait is optional
  cleanup, not a blocker.

## Background — Current State

- The agent pill in `new-workspace-dialog.tsx` lists pinned `cli`
  presets. On submit, `handleSubmit` calls `applyPreset` or
  `createWorktreeWorkspace` with `selectedAgentId`.
- The final terminal command is assembled by
  `prepare_agent_command(preset_id, base_command, initial_prompt)` in
  `src-tauri/src/branch_name.rs` — a hardcoded `match preset_id`. This
  is the single chokepoint where model flags must splice in.
- The Beta composer (`ComposerFooter.tsx`) already has
  `MultiProviderModelPicker` + `ReasoningPicker` + `PermissionModePicker`.
- Capability data: `list_chat_provider_capabilities` →
  `provider-capabilities-store`. OpenCode is **live-harvested**;
  Claude / Codex use hand-maintained fallback bundles. `ChatModelInfo`
  carries `sub_provider`; the bundle carries effort levels and
  context-window options.
- OpenFlow has a **separate, mostly-hardcoded** model registry
  (`list_models_for_tool` in `src-tauri/src/commands/openflow.rs`).
  **Do not reuse it** — it is the staleness this plan avoids. It is
  retired/converged in Stage 3.

## Design

### Source of truth

Reuse `list_chat_provider_capabilities` / `provider-capabilities-store`.
One source of model truth for both the Beta composer and the launch
dialog. OpenCode stays genuinely live; Claude / Codex stay maintained in
exactly one place. No fourth registry.

### Capability Provider registry

A Rust trait + registry decouples "what models/flags an agent supports"
from the dialog. The dialog never names a specific agent.

```
trait AgentCapabilityProvider {
    fn id(&self) -> &str;                       // "claude", "codex", "opencode", ...
    fn binaries(&self) -> &[&str];              // CLI names it owns, for auto-detection
    fn capabilities(&self) -> ProviderCapabilities;  // models + reasoning + context;
                                                     // declares which controls exist
    fn build_flags(&self, sel: &ModelSelection) -> Vec<String>;  // selection -> argv
}
```

**Resolution:** a preset is bound to a provider by **auto-detecting the
first binary token of its command** against each provider's
`binaries()`. An optional explicit `capability_provider` field on
`TerminalPreset` overrides detection (escape hatch for wrapper scripts,
`npx`-launched tools, env-prefixed commands).

Three outcomes when a preset is added — the dialog changes in none:

| New preset launches… | Model pill? | Work needed |
|---|---|---|
| `claude` / `codex` / `opencode` / `gemini` (any variant) | yes, automatic | none |
| A brand-new CLI agent | no, until its provider exists | one `AgentCapabilityProvider` impl |
| A shell script / unmodeled command | no (correct) | none |

The pill is purely additive: a preset that opts into nothing costs
nothing and still launches normally.

### Capability gating

The popover renders only the controls a provider declares:

- **Claude** — `{ models, reasoning, context }`
- **Codex** — `{ models, reasoning }`
- **OpenCode** — `{ models, reasoning }`; context shown as a read-only
  per-model label (most OpenCode models have a fixed context window —
  there is no `--context` choice to make)
- **Gemini** — `{ models }`

### Flag injection

`build_flags` returns argv fragments spliced into the base command at
the `prepare_agent_command` chokepoint, before the prompt argument.

**Double-injection guard:** if the base command already contains a
`--model` flag (e.g. a user-made "Claude Opus" preset), parse the value
out, **pre-select it in the pill**, and do not add a second flag.

### UX — adaptive popover

A second pill next to the agent pill. Defaults to "Default" → emits no
flag → behavior unchanged for default-model users.

Short list (Claude / Codex, ≤ ~10 models) — flat list, no search:

```
 [✳ Claude Code ⌄]  [◇ Default ⌄]
                          │
        ╭─ Model ─────────┴───────────╮
        │ ◇ Default               ✓  │
        │ ───────────────────────────  │
        │ ✳ Sonnet 4.6               │
        │ ✳ Opus 4.1                 │
        │ ✳ Haiku 4.5                │
        │ ───────────────────────────  │
        │ Reasoning   ◐ Think    ⌄   │   (Codex: effort; Claude: see Q)
        │ Context     200K       ⌄   │   (Claude only)
        ╰─────────────────────────────╯
```

Long list (OpenCode, > threshold) — search + favorites + sub-provider:

```
 [▣ OpenCode ⌄]  [◇ Default ⌄]
                      │
   ╭─ Model ──────────┴──────────────────╮
   │ 🔍 Search 4,312 models...            │
   │ ──────────────────────────────────── │
   │ ★ FAVORITES                          │
   │   claude-sonnet-4-6    · Anthropic   │
   │   gpt-5                 · OpenAI      │
   │ ──────────────────────────────────── │
   │ ANTHROPIC                            │
   │   claude-sonnet-4-6                ☆ │
   │   claude-opus-4-1                  ☆ │
   │ OPENAI                               │
   │   gpt-5                            ☆ │
   │   gpt-5-mini                       ☆ │
   │ ──────────────────────────────────── │
   │ Reasoning   ◐ High      ⌄           │
   ╰──────────────────────────────────────╯
```

- Search bar + star buttons appear only above a threshold (~10 models).
- Favorites reuse `picker-favorites-store` (already keyed
  `${provider}::${model_id}`, already persisted).
- Last-used model is persisted per provider in `ui-store`, mirroring
  the existing `lastSelectedAgentId`.

## Staged Implementation

### Stage 1 — Flat pill for Claude / Codex

- Add optional `capability_provider` field to `TerminalPreset`
  (`presets.rs`) + binary-token auto-detection.
- `AgentCapabilityProvider` trait + registry; Claude and Codex
  providers backed by the existing maintained capability bundles.
- Thread a `ModelSelection { model, reasoning, context }` (all
  optional) through `applyPreset` and `createWorktreeWorkspace`.
- `build_flags` injection in `prepare_agent_command`, with the
  double-injection guard.
- Second pill in `new-workspace-dialog.tsx` with a flat list; reasoning
  sub-row for Codex; context sub-row for Claude.
- Persist last-used model per provider in `ui-store`.

### Stage 2 — OpenCode search list

- OpenCode provider backed by the live harvest.
- Extract a shared `<ModelList>` component from
  `MultiProviderModelPicker` (search + `sub_provider` grouping +
  favorites). The Beta composer keeps wrapping it with the provider
  rail; the launch-dialog pill wraps it with nothing.
- Adaptive threshold: search/favorites appear only for long lists.

### Stage 3 — Registry hardening + new providers

- Gemini provider (maintained list).
- Harden auto-detection edge cases (wrapper scripts, `npx`, env
  prefixes); surface the explicit `capability_provider` override in the
  preset editor.
- Converge `prepare_agent_command`'s hardcoded `match` and OpenFlow's
  `list_models_for_tool` onto the registry where practical.
- Document the one-line contract for the future-presets agent in
  `docs/features/presets.md` and `docs/features/workspace-creation.md`.

## Open Questions

- **Model ID reconcile.** Do chat-capability model IDs map cleanly onto
  each CLI's `--model` argument? OpenCode slugs (`provider/model`) and
  Claude aliases (`sonnet`/`opus`) are known-good; Codex and Gemini need
  verification.
- **Claude reasoning.** Claude Code CLI has no `--reasoning` flag — the
  chat path uses ultrathink prompt-injection. Lean: implement Claude
  reasoning as a think-keyword injected into the prompt (the dialog
  already builds the prompt), or hide the control for Claude. Decide in
  Stage 1.
- **Claude context.** Confirm how the CLI selects the 1M context window
  (model-id variant, beta header, or env var) before wiring the context
  control.
- **Onboarding flow.** Should the pill also appear in
  `project-onboarding.tsx` (first-project flow)? Likely yes — same
  component.
- **OpenCode harvest cost.** The harvest spins an `opencode serve`
  child. Acceptable to trigger lazily when the dialog opens with an
  OpenCode preset selected, or pre-warm?

## Likely Touch Points

### Backend

- `src-tauri/src/presets.rs` — `TerminalPreset` gains
  `capability_provider`.
- `src-tauri/src/branch_name.rs` — `prepare_agent_command` gains flag
  injection via the registry.
- `src-tauri/src/commands/presets.rs` — `apply_preset` threads
  `ModelSelection`.
- `src-tauri/src/commands/workspace.rs` — `create_worktree_workspace`
  threads `ModelSelection`.
- New: `src-tauri/src/agent_capability/` — the trait, registry, and
  per-provider impls.
- `src-tauri/src/commands/agent_chat.rs` — `list_chat_provider_capabilities`
  reused as the capability source.

### Frontend

- `src/components/overlays/new-workspace-dialog.tsx` — the second pill.
- `src/components/overlays/project-onboarding.tsx` — pill (pending Q).
- `src/components/chat/pickers/MultiProviderModelPicker.tsx` — extract
  the shared `<ModelList>`.
- `src/stores/picker-favorites-store.ts` — reused as-is.
- `src/stores/provider-capabilities-store.ts` — reused as the source.
- `src/stores/ui-store.ts` — last-used model per provider.
- `src/tauri/commands.ts`, `src/tauri/types.ts` — `ModelSelection` shape.

### Docs

- `docs/features/workspace-creation.md` — update the creation flow.
- `docs/features/presets.md` — document `capability_provider` + the
  extension contract.
- `docs/core/STATUS.md` — record the feature when it lands.

## Notes

- Do not add a fourth model registry. Reuse the chat capability system.
  OpenFlow's `list_models_for_tool` stays untouched until Stage 3
  convergence.
- The pill is purely additive: "Default" = today's behavior, zero
  regression for default-model users.
- Gemini gets a maintained list in Stage 3; Pi has no capability source
  and correctly shows no pill.
