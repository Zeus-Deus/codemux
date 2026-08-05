# Research: Claude Opus 5 support in Agent Chat

- Purpose: Everything needed to add the new cloud model `claude-opus-5` to
  the agent chat GUI (picker, effort, speed, defaults) correctly.
- Audience: The session that implements the change.
- Status: Research complete, implementation not started.
- ⚠️ **Read the fast-mode resolution below before the fast-mode passages above
  it.** Fast mode for Claude was resolved *after* most of this note was
  written: `merge_sdk_with_maintained` now hard-clamps `supports_fast_mode`
  to `false` for every Claude model, so the speed picker can never render for
  Claude. Passages framed as "the merge promotes it to `true` when the CLI
  reports `supportsFastMode`" describe the pre-clamp code and are retained
  only as the rationale trail. Line numbers cited throughout are from the
  pre-clamp `capabilities.rs` and are stale (the file grew ~440 lines on the
  same branch) — navigate by symbol name, not by line.
- Read next: `docs/features/agent-chat.md`, `docs/features/multi-provider-chat.md`.

## Model facts (Anthropic API, verified against the claude-api reference)

| Property | Value | Codemux relevance |
|---|---|---|
| Model ID | `claude-opus-5` — fixed id, **no date suffix** (same scheme as `claude-opus-4-8`) | Picker id / persisted config value |
| Position | Successor to Opus 4.8 in the Opus line; drop-in upgrade at the same price | Candidate for the new recommended default |
| Pricing | $5 / $25 per MTok (fast mode $10 / $50) | Display-only; Codemux doesn't bill |
| Context | 1M tokens — **default and maximum**, no beta header | Context-window picker options |
| Max output | 128K | No Codemux surface |
| Effort | Full ladder `low / medium / high / xhigh / max`; API default `high` | Matches the existing flagship effort vocabulary exactly — **no new effort token** |
| Thinking | **On by default** (omitting `thinking` runs adaptive). `disabled` only valid at effort ≤ `high` (400 with `xhigh`/`max`) | CLI/SDK-side; Codemux has `supports_thinking_toggle: false` for Claude, so no UI constraint needed |
| Fast mode | Supported by the model (research preview, Claude API only, beta `fast-mode-2026-02-01`) | **Not surfaced.** `supports_fast_mode` is clamped to `false` for all Claude models, so no Standard/Fast speed picker — see the resolution section |
| Rate limits | Separate bucket from the combined Opus 4.x pool | None (user-facing note at most) |
| Safety | Can return `stop_reason: "refusal"`; server-side fallbacks route cyber refusals to Opus 4.8 | Handled entirely by the Claude CLI/SDK; no Codemux change |
| Prompt cache | 512-token minimum (halved from 4.8's 1024) | None |

## How Codemux resolves Claude models (why the change is small)

Codemux never talks to Anthropic for chat. The chat pane drives the bundled
`claude-agent` sidecar (`@anthropic-ai/claude-agent-sdk`), which passes
`model` and `effort` through to the user's deployed `claude` CLI as opaque
strings (`sidecar/claude-agent/src/session.ts:210–216` — effort is even cast
past the SDK's static union for forward compatibility). There is **no model
whitelist anywhere** in the sidecar, the Rust adapter
(`agent_provider/claude/session.rs` start-session build), the launch-flag
injector (`agent_capability.rs` — charset-safety check only), or the DB
(`agent_chat_sessions` stores free-form strings; no migration needed).

The model *list* the picker shows comes from
`src-tauri/src/agent_provider/claude/capabilities.rs` (the single Claude
model registry), through three paths that merge:

1. **Maintained roster** — `models()`, ordered; **index 0 is the recommended
   default** (drives the frontend `defaultModelId()` and the `caps.models[0]`
   test pin). Note this holds for the *fallback* path only: on the SDK-harvest
   path `build_capabilities_from_sdk` reorders, because `dedupe_default_alias`
   folds the `default` alias row into its concrete twin and moves that twin to
   index 0 with a `Recommended · ` description prefix.
2. **SDK harvest** — the sidecar's `list-models` (`supportedModels()`)
   returns the deployed CLI's roster; `merge_sdk_with_maintained()`
   lets SDK effort vocabulary win, with maintained metadata as fallback, and
   strips `[1m]` context-suffix ids before lookup. It no longer lets
   `supportsFastMode` win — that field is now clamped to `false` (see below).
3. **`/v1/models` harvest** — only when `ANTHROPIC_API_KEY` is set.

Unknown ids fall back to family inference (`ClaudeFamily::from_id`,
l. 573–613): `claude-opus-5` **already** classifies as flagship (substring
`"opus"`) and would infer `[low..max]` + `high` default + ultrathink +
200k/1M. So a live-harvested `claude-opus-5` renders sensibly today; what a
maintained entry adds is the curated blurb, correct list position, alias
description backfill, and a deliberate fast-mode/default decision.

## Required change (1 file)

### `src-tauri/src/agent_provider/claude/capabilities.rs`

**A. New `ChatModelInfo` entry in `models()`** — insert at **index 0**
(making it the recommended default, consistent with "drop-in upgrade at
Opus 4.8's pricing"):

```rust
// Opus 5 — the recommended default. Successor to Opus 4.8 at the
// same pricing; strongest on deep reasoning and long-horizon agentic
// work. Thinking is on by default server-side; effort default high.
ChatModelInfo {
    id: "claude-opus-5".into(),
    label: "Claude Opus 5".into(),
    description: Some("Best for everyday, complex tasks".into()),
    effort_levels: vec![
        "low".into(), "medium".into(), "high".into(),
        "xhigh".into(), "max".into(),
    ],
    default_effort: Some("high".into()),
    prompt_injected_effort_levels: vec!["ultrathink".into()],
    context_window_options: vec![ctx_200k(), ctx_1m_default()],
    supports_adaptive_thinking: true,
    supports_thinking_toggle: false,
    supports_fast_mode: false, // clamped off for all Claude models — see the fast-mode section below
    supports_images: true,
    sub_provider: None,
    is_free: false,
},
```

Notes on the values:

- **Effort**: identical vocabulary to 4.8/4.7/Fable — no
  `claude_effort_label_map()` change, no `ReasoningPicker`
  `DEFAULT_EFFORT_DESCRIPTIONS` change (both already cover `xhigh`/`max`).
- **Fast mode**: leave the maintained flag `false` and expect no picker.
  `merge_sdk_with_maintained` clamps `supports_fast_mode` to `false` for
  every Claude model regardless of what the CLI advertises, so nothing about
  adding Opus 5 can surface a speed picker. (Historically the merge promoted
  the flag on an SDK report; that is the behavior the clamp replaced.)
- **Context options**: keep the `[200k, 1M-default]` pair. 1M is already
  the server-side default for Opus 5, but the CLI's bracket-suffix
  convention (`resolve_claude_api_model_id` appends `[1m]` when the user
  picks 1M; already-pinned ids never double-append) is uniform across
  models and harmless — see Open Questions #2 for the one thing to verify
  on a real CLI.

**B. Alias remap** — `ALIAS_CANONICAL_IDS` (l. 778–784): point `"default"`
and `"opus"` at `claude-opus-5` (the comment there says "a future model
bump is a one-line edit" — this is that edit). Otherwise the CLI's alias
rows keep synthesizing Opus 4.8 descriptions.

**C. Demote the 4.8 comment** — the "recommended default" comment moves to
the new entry; 4.8's description could become "Previous Opus generation"
*only if* the CLI's own roster does the same (see Open Questions #1 —
keeping "Best for everyday, complex tasks" on both is fine short-term).

**D. Tests in the same file** (`mod tests`, l. 947+):

| Test | Change |
|---|---|
| `maintained_list_includes_fable_5` (l. 963–985) | `caps.models[0].id` pin flips `claude-opus-4-8` → `claude-opus-5` |
| `fallback_includes_core_roster` (l. 952–960) | add `claude-opus-5` |
| `maintained_effort_vocab_matches_the_deployed_cli` (l. 1136–1160) | add `claude-opus-5` to the flagship loop |
| `canonical_id_for_alias_maps_every_alias_and_ignores_full_ids` (l. 1776–1786) | alias expectations follow the remap |
| `sdk_merge_surfaces_unknown_id_with_inferred_metadata` (l. 1415–1430) | uses fixture id `"claude-opus-5-0"` — **different id, no collision**, but its intent is "unknown id"; leave the fixture as-is |

## Recommended alongside (same PR)

1. **`src/lib/agent-chat/capability-defaults.ts:19`** —
   `FALLBACK_DEFAULT_MODEL_BY_PROVIDER.claude: "claude-opus-4-8"` →
   `"claude-opus-5"`. This is only the pre-hydration fallback (real default
   comes from `caps.models[0]`), but leaving it stale means one render of
   the wrong pill on cold start.
2. **Dev mock** — `src/dev/tauri-mock.ts` (`CLAUDE_CAPABILITIES`, seeded
   session configs pinned to `"claude-opus-4-8"`, l. 488–520 / 759 / 1694 /
   1717): add an Opus 5 row so screenshots/dev flows exercise the new
   default; `src/dev/mock-fixtures.ts` display strings are cosmetic.
3. **Docs** — `docs/features/multi-provider-chat.md` (harvest/description
   contract examples name Opus 4.8), `docs/features/agent-chat.md` (the
   `Opus 4.8 with 1M context · …` example string), `docs/core/STATUS.md`.

## Explicit non-changes (verified)

- **Sidecar** (`sidecar/claude-agent/`): model + effort are opaque
  passthrough; `list-models` forwards the SDK roster verbatim. Nothing to do.
- **DB / persistence**: `AgentChatSessionConfig` columns are free-form
  strings; per-thread picker config (`model`, `effort`, `context_window`,
  `fast_mode`, `permission_mode`) persists and restores unchanged.
- **Picker components**: `ModelPicker`, `MultiProviderModelPicker`,
  `ReasoningPicker`, `SpeedPicker` are fully capability-driven.
- **Favorites**: keyed `"<driver>::<modelId>"`; a new id just starts
  unfavorited.
- **Settings models** (`ai_commit_message_model`, `ai_resolver_model`):
  free-form, picker-driven — inherit the new row automatically.
- **Thinking-disabled 400 at xhigh/max**: an API-level constraint the CLI
  owns; Codemux exposes no thinking toggle for Claude
  (`supports_thinking_toggle: false`), so no gating UI is needed.

## Open questions (decide before/while implementing)

1. **Does Opus 5 become the recommended default (index 0 + alias remap)?**
   Recommended: yes — same price, drop-in successor, matches the registry's
   own "future model bump" design. If the team wants to stage it, add the
   entry *after* 4.8 first and flip the pin in a follow-up; both orders are
   one-line diffs.
2. **`[1m]` suffix on a 1M-default model** — RESOLVED (observed in-app,
   2026-07-24): the deployed CLI's roster reports the Opus 5 alias
   pre-pinned to 1M (display name "Opus (1M context)", suffix-pinned id),
   consistent with 1M being Opus 5's standard-pricing default. The merge's
   suffix-stripping and `resolve_claude_api_model_id`'s no-double-append
   guard handled it correctly with zero code change.
3. **Blurb wording** — proposal above reuses 4.8's "Best for everyday,
   complex tasks" for the new default and would demote 4.8 to "Previous
   Opus generation" once the CLI does. Alternative Opus 5 blurb if we want
   differentiation: "Strongest for deep reasoning and long-horizon agentic
   work".
4. **Fast mode maintained flag** — RESOLVED (owner decision, 2026-07-24):
   the deployed CLI does report `supportsFastMode` for Opus 5, which is
   exactly what surfaced the problem — ⚠️ **`supportsFastMode` is a
   capability advertisement, NOT an entitlement check.** The outcome was to
   clamp the flag to `false` for all Claude models rather than trust the
   report; see "Fast mode: silent-fallback feedback gap" and the resolution
   that follows it.

## Fast mode: silent-fallback feedback gap (observed 2026-07-24)

Live-instance finding (Opus 5 session on a subscription account with Extra
Usage disabled at the org level, `org_level_disabled`):

- Codemux persisted `fast_mode = 1` and correctly sent `fastMode: true`
  through the Agent SDK.
- Every response nonetheless reported `usage.speed: "standard"` /
  `service_tier: "standard"` — the server **silently fell back to
  Standard**. No error, no Extra Usage charge; turns consumed normal
  subscription limits.
- The composer's speed pill kept showing "Fast" the whole time — the UI
  claimed a speed the account never got.

Conclusion: the SDK's `supportsFastMode` (which gates the picker) means
"this model supports fast mode", not "this account is entitled to it right
now". The only truth signal is per-response `usage.speed`, which arrives
after the fact.

**Resolution (implemented on this branch, owner decision 2026-07-24):
fast mode is removed for Claude entirely.** `merge_sdk_with_maintained`
in `capabilities.rs` now clamps `supports_fast_mode: false` regardless of
the SDK-reported `supportsFastMode` (the two maintained `true` entries —
Opus 4.6 / 4.5 — were flipped to `false`, the SDK-passthrough test
assertion inverted, and the dev mock's Claude row mirrored). The speed
picker therefore never renders for any Claude model; Codex's fast tier
(its own catalog-derived flag) is untouched. Threads with a persisted
`fast_mode = 1` heal back to Standard automatically via the existing
capability-change heal paths (`AgentChatPane` / `chat-pane-plans` /
`DraftChatSurface`).

**Future re-enable path (only if entitlement feedback ships first):**

- Pick-time gate: graduate the sidecar's `accountInfo` RPC (Extra Usage
  state, e.g. `org_level_disabled`) and render the Fast row disabled with
  a reason for non-entitled accounts.
- Post-hoc backstop: read `usage.speed` in `translate.rs`; on a
  Fast-requested turn observed `standard`, emit a prefixed
  `RuntimeWarning` (mirroring `resume-fallback: `), promote it via
  `runtime-notice.ts` to an inline notice, and heal the thread's speed to
  Standard. Notice once per session, not per turn.
- The `SdkModelInfo.supports_fast_mode` field is kept (`#[allow(dead_code)]`)
  so re-enabling is a one-line merge change.

## Verification plan for the implementation PR

1. `cargo test --manifest-path src-tauri/Cargo.toml` — capabilities tests
   updated per the table above.
2. `npm run verify`.
3. Manual smoke (real `claude` CLI + auth): open chat → picker shows
   "Claude Opus 5" at the top with blurb; start a session, confirm
   `--effort xhigh` works and a turn streams; flip context 200k ↔ 1M;
   confirm **no** speed picker appears (Claude fast mode is clamped off);
   restart the app and confirm the persisted model re-seeds the pill
   (post-restart "reset to Opus" regression guard).
4. Fast-mode removal (already implemented on this branch): confirm no
   Claude model shows the Standard/Fast picker, and that a thread with a
   previously persisted `fast_mode = 1` heals to Standard on open.
