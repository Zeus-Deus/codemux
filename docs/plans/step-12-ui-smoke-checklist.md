# Step 12 — Multi-Provider Chat UI Smoke Checklist

- Purpose: Operator-driven manual smoke for the multi-provider chat picker. Run after a fresh `npm run tauri:dev` build, or against a published release once Step 12 ships.
- Audience: Whoever is verifying Step 12 end-to-end before declaring it daily-drivable.
- Authority: The unit test suite covers component-level behaviour; this checklist is the missing live-app validation pass that unit tests can't capture (provider startup, real Tauri IPC, real session creation, real OpenCode harvest).
- Update when: A picker behaviour changes that the listed steps would no longer exercise correctly.
- Read next: `docs/features/multi-provider-chat.md` for the canonical feature description.

This checklist exists because the picker is a single React surface that touches: the Tauri IPC layer (`list_chat_provider_capabilities`, `agent_chat_create_pane`, `agent_chat_start_session`), the live OpenCode server lifecycle (spawn / ready-banner detection / `kill_on_drop`), the favorites localStorage round-trip, and three independent driver adapters. Unit tests pin each piece in isolation; this is the cross-cutting validation.

## Setup

- [ ] Sign in to Codemux (existing account or fresh signup)
- [ ] Open or create a chat pane in any workspace
- [ ] Confirm `enable_agent_chat` feature flag is on (Settings → Observability → Feature Flags, or the chat pane should render at all)

## Picker visibility

- [ ] Click the model picker trigger pill in the composer footer (left side, next to the reasoning + permission chips)
- [ ] Picker opens as a popover with a 2-column layout
  - Left: 48px-wide provider rail with three icon buttons
  - Right: search input on top, scrollable model list below
- [ ] All three providers visible in the rail: Claude, Codex, OpenCode
- [ ] Active provider has a visible indicator (vertical bar at the right edge of its icon)
- [ ] Hovering a rail icon shows a tooltip with the provider name + model count

## Per-provider browsing

- [ ] Click the **Claude** rail icon → model list shows Claude's hand-maintained models (typically Opus 4.7, Opus 4.6, Opus 4.5, Sonnet 4.6, Haiku 4.5)
- [ ] Click the **Codex** rail icon → model list shows the four Codex models: `gpt-5.4`, `gpt-5.4-mini`, `gpt-5.3-codex`, `codex-mini-latest`
  - This is the smoke step that proves Step 12 made Codex selectable for the first time. Pre-Step-12 it was hidden behind `ENABLE_PROVIDER_PICKER = false`.
- [ ] Click the **OpenCode** rail icon → model list shows a flattened list with subtitles formatted `OpenCode · {sub_provider}` (e.g. `OpenCode · openai`, `OpenCode · anthropic`)
- [ ] Each model row's secondary line shows the provider icon + provider label

## Search

- [ ] With Claude rail selected, type `haiku` in the search input → list narrows to Haiku models from any provider that matches
- [ ] Type `gpt` → list shows results from Codex AND OpenCode (cross-provider). Provider grouping collapses, results are flat
- [ ] Type `xyz123` (gibberish) → "No models match \"xyz123\"" empty state renders
- [ ] Clear the search → rail-selected provider's list takes over again (no flat-search view)

## Favorites

- [ ] Hover any model row → star button fades in on the right side
- [ ] Click the star → it fills with amber color, model row reflows immediately so the favorited row floats to the top
- [ ] Close picker (Escape, or click outside)
- [ ] Reopen picker → star is still filled
- [ ] Search any query that matches multiple models → favorited model appears above non-favorites in the result list
- [ ] Star a Codex model AND an OpenCode model in addition to the Claude favorite
- [ ] Type a generic query that matches all three favorites → all three favorited rows surface above any non-favorite that also matches
- [ ] Click a filled star → returns to empty (unfavorited), row drops back into rail-order

## Selection & session creation

- [ ] Pick a Claude model → composer trigger reflects the new label, popover closes
- [ ] Send a test message → response streams back via the Claude adapter (existing flow)
- [ ] Open picker again, pick a Codex model → composer trigger reflects it
- [ ] Send a test message → response streams back via the Codex adapter
  - This proves the multi-provider routing works end-to-end and Codex panes are functional, not just selectable.
- [ ] Open picker, pick an OpenCode federated model (e.g. `openai/gpt-5` if you have OpenAI credentials in OpenCode) → composer trigger reads `openai · GPT-5` (or similar)
- [ ] Send a test message → response streams back through OpenCode → upstream provider

## OpenCode-specific empty states

- [ ] If `opencode` is **not** on PATH: clicking the OpenCode rail surfaces "OpenCode not detected on your system" with the install hint and an `opencode.ai` link
- [ ] If `opencode` is installed but no upstream credentials are configured: clicking the OpenCode rail surfaces "No connected providers" with the `opencode auth login` hint
- [ ] If a slow OpenCode startup is in progress on first picker open: skeleton placeholders render in the model list area while the harvest finishes

## Persistence + reload

- [ ] Quit Codemux entirely (close all windows, ensure the Tauri process exits)
- [ ] Reopen Codemux → previously-favorited models still favorited
- [ ] Picker still selects the right provider per-pane (pane snapshot serde round-trips the provider variant)
- [ ] Sending a message on a Codex pane after restart routes to the Codex adapter (no reversion to Claude)

## Multi-pane + provider switching

- [ ] Create two chat panes side-by-side (split or new tab)
- [ ] Set pane A's picker to Claude, pane B's picker to Codex
- [ ] Send messages on both → each pane streams from its own adapter; messages don't cross-contaminate
- [ ] Switch pane A from Claude to OpenCode mid-conversation → existing transcript stays intact, new turns route to OpenCode

## Negative / regression

- [ ] Existing Claude flow unchanged when Claude is selected (no behaviour drift from Stages 4 / 6)
- [ ] `DraftChatSurface` (the empty-state composer when no chat exists) still uses the legacy single-provider ModelPicker (no rail visible there)
- [ ] OpenCode server is reaped on Codemux quit: after closing the app, `pgrep -af 'opencode serve'` returns no processes (Linux/macOS); Task Manager shows no `opencode.exe` (Windows)

## Notes

- This checklist deliberately does NOT cover keyboard shortcuts. `Ctrl+1..9` collides with workspace switching and was deferred per the locked Step 12 scope; v2 picks a non-colliding namespace.
- Multi-instance per provider is also deferred. If you have multiple Codex accounts or multiple OpenCode connections, they collapse under one rail entry today.
- If a step fails, capture: which step, what you saw, the OpenCode version (`opencode --version`), and the relevant errors from `Settings → Diagnostics`. File the report against `docs/plans/step-12-opencode-implementation-plan.md` follow-ups.
