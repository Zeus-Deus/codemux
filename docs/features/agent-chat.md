# Agent Chat

- Purpose: Describe the current capability and constraints of the agent-chat
  subsystem.
- Audience: Anyone working in or around the chat panel feature area.
- Authority: Canonical feature-level reality doc.
- Update when: Behavior, constraints, expectations, or major touch points
  change.
- Read next: `docs/features/multi-provider-chat.md`,
  `docs/features/skills-sync.md`, `docs/features/mcp-server.md`,
  `docs/features/workflow-orchestration.md`,
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
- **`src/components/chat/`** — chat pane UI (redesigned on the shadcn
  chat primitives, June 2026): `AgentChatPane`, `Composer` (with `+`
  popup, `@` mention popup, slash command popup, image paste/drop),
  `ChatTranscript`, `MessageList` (shadcn MessageScroller shell),
  `transcript-slots.ts` (pure turn-grouping/tool-folding slot builder),
  `AssistantAvatar`, `ReasoningBlock` (collapsible thinking),
  `ToolCallCard` + per-tool bodies, `ActivityBlock` (one folded run of
  reasoning + tool calls, working/settled — see "Activity block" below;
  `activity-steps.ts` holds its pure step/summary/duration derivations),
  `DiffView` (red/green edit surface), `TaskSummaryCard` (TodoWrite
  checklist), `StreamingMarker` (shimmer tail status), `tool-visuals.ts`
  (icon/tint mapping), `PlanProposalBlock`, `ComposerPendingInputPanel`
  for AskUserQuestion, `PermissionRequestBlock`, `ModePill`,
  `SessionSelector`, `DraftChatSurface`, `ChatHomeLanding`,
  `DebugCleanupBanner`, `DebugExitDialog`, and the picker family under
  `src/components/chat/pickers/`. Shared primitives live in
  `src/components/ui/` (message-scroller, message, bubble, attachment,
  marker, avatar, spinner) and `src/components/ai-elements/` (vendored
  AI Elements — reasoning, tool, task, prompt-input, shimmer,
  message/MessageResponse, code-block — with all Vercel AI SDK type
  imports replaced by local unions; the AI SDK is NOT a dependency).
  Assistant markdown renders through **Streamdown**
  (`parseIncompleteMarkdown` keeps mid-stream fences/emphasis clean).

## What Works Today

- **Three end-to-end providers** behind one unified picker:
  - **Claude** — Claude Agent SDK via the Bun-compiled `claude-agent`
    sidecar (JSON-RPC over stdio).
  - **Codex** — `codex app-server` subprocess, JSON-RPC over stdio.
  - **OpenCode** — Rust-direct HTTP against a managed `opencode serve`
    child (`kill_on_drop`, generated `OPENCODE_SERVER_PASSWORD`).
  - All three render in a single 2-column picker (provider rail + searchable
    model list); favorites persist via zustand + `localStorage`.
  - **Model rows show the resolved version + blurb** (like the terminal
    `/model` picker): when `ChatModelInfo.description` is present the row
    subtitle renders `<driver> · <description>` (truncated, full text in a
    `title` tooltip), e.g. `Claude · Opus 4.8 with 1M context · Best for
    everyday, complex tasks`; the trigger pill mirrors label + description
    into its tooltip. Backend guarantees every Claude row carries a
    description: SDK-provided string verbatim when the CLI supplies one →
    maintained blurb for known full ids (blurb-only; the label already
    carries the version) → alias backfill (`ALIAS_CANONICAL_IDS` in
    `src-tauri/src/agent_provider/claude/capabilities.rs` maps
    `default`/`opus`/`fable`/`sonnet`/`haiku` to canonical maintained ids and
    synthesizes `<Version>[ with 1M context] · <blurb>`). Model *selection*
    itself passes the picked id through unchanged; alias ids are resolved to
    the latest concrete model by the Claude CLI/SDK, not by Codemux.
- **Streaming chat UX**: Streamdown-rendered messages, tool approvals
  (per-tool body rendering), collapsible reasoning blocks (thinking
  deltas reduce into a `reasoning` ChatViewItem that seals on any
  non-thinking boundary and finalizes with a "Thought for Ns"
  duration), **Activity blocks** (one folded run of reasoning + tool
  calls — see "Activity block" below), red/green diff cards for edits,
  TodoWrite task checklists, plan proposals (`ExitPlanMode`),
  AskUserQuestion panels, shimmer streaming marker, debug-mode banner
  + exit dialog.
- **Mode pills**: Ask / Allow always / Plan / Debug, with Shift+Tab cycling
  and silent-restart on pill removal.
- **Attachments** via `+` and `@`: files, folders, GitHub issues + PRs,
  images via paste / drop / picker. Inline chips, send-time injection,
  expand, caps, gif guard, chip tooltips. See
  `docs/plans/step-8-attachments.md`.
  - **Ctrl+V image paste has a Linux clipboard fallback.** WebKit2GTK
    strips image payloads from the JS `paste` event, so `Composer`'s
    handler tries `e.clipboardData.items` first (works in the browser
    dev mock / Chrome) and, when that yields no image, falls back to the
    `paste_clipboard_image` Rust command (reads the OS clipboard
    server-side, re-encodes PNG via the shared `encode_rgba_to_png`,
    returns `{ bytes, mime }`). Composer wraps the bytes in a `File` and
    hands them to the same `handleAttachImage` (`AgentChatPane`) the `+`
    picker uses, so validation/staging are identical. This mirrors the
    new-workspace dialog's `paste_clipboard_image_to_file`, but returns
    bytes (not a temp-file path) because the chat stages bytes in
    memory. A text-only clipboard rejects the command → default paste
    runs untouched.
- **Slash command popup** with cross-provider parsing.
- **Cross-provider skill system**: watcher, conflicts, disable, refined
  compat. Server-side sync (see `docs/features/skills-sync.md`).
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
- **Restart auto-resume + persisted picker config**: a chat pane reopened
  after an app quit resumes transparently. The provider session map is
  empty on every launch (no startup rehydration), so the backend
  `ensure_live_session` choke point in `commands/agent_chat.rs` rebuilds a
  dead session from its `agent_chat_sessions` row on the next
  `agent_chat_send_turn` / `agent_chat_respond_to_request` — reusing the
  SAME `thread_id` (keeping the attached Channel, pane snapshot, and store
  slice valid) and passing `resume_cursor: {"resume": sdk_session_id}`
  when the row carries one (falling back to a fresh session, whose
  transcript still hydrates from the DB, when it does not or when the
  resume-start fails). Each thread's picker config (`model`, `effort`,
  `context_window`, `permission_mode`) is persisted on the session row —
  written at `agent_chat_start_session`, updated fire-and-forget by the
  picker handlers via `agent_chat_update_session_config`, always persisted
  by `agent_chat_set_model` / `agent_chat_set_permission_mode` (which now
  swallow a `SessionNotFound` from the dead-session apply into `Ok`, so
  the value takes effect on the next auto-resume), and carried across
  silent restarts by `migrate_agent_chat_session`. On pane mount the
  frontend fetches `agent_chat_get_session(threadId)` and re-seeds the
  pickers + resume cursor from the row, falling back to the provider
  default model only when no row (or no persisted model) exists —
  fixing the post-restart "reset to Opus" regression.
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

## Transcript scroller (issue #77 contract, shadcn MessageScroller)

The transcript body (`MessageList.tsx`) renders on the shadcn
**MessageScroller** (June 2026; styled wrapper over the `@shadcn/react`
headless engine). It replaced `react-virtuoso`: rows are contained with
`content-visibility:auto` + `contain-intrinsic-size` instead of
windowed mounting, so all rows exist in the DOM but off-screen rows
skip layout/paint — measured ~60 fps through the ~1,200-item mock
transcript; the reducer's 5,000-message cap bounds the worst case.

Layout derivation lives in the pure `buildTranscriptSlots`
(`transcript-slots.ts`): turn grouping (avatar once per assistant
turn), Activity-run folding (reasoning + tool calls → one `activity`
slot — see "Activity block" below), and per-slot identity.

Contract preserved from the pre-redesign renderer:

- **Stable keys + memo rows.** Per-slot keys are still `slot.item.id`
  / `run:<first-id>` (as `MessageScrollerItem messageId`), and rows
  render through memoized leaves (`ItemRowMemo` / `ActivityRowMemo`)
  — a streaming token mutates exactly one row. (Activity rows rebuild
  their `items` array each build, as the old tool-group rows did; the
  stable `run:<first-id>` key keeps the scroller row from remounting as
  the run grows and transitions working → settled.)
- **Stick-to-bottom.** Pinned-ness is tracked from real scroll events
  (≤ 80 px from the bottom); after every transcript change (or
  streaming-marker toggle), if pinned, it snaps to the tail. Content
  growth alone never unpins, so auto-scroll never fights a user
  reading history. This is implemented directly in `MessageList.tsx`
  (mount tail-snap with a bounded settle loop + pinned-follow effect)
  because `content-visibility` rows expose only estimated heights
  until rendered — the engine's own mount positioning and smooth
  `scrollToEnd` land short on large hydrated transcripts.
- **Turn anchoring is deliberately OFF** (`scrollAnchor={false}` on
  every row): with hundreds of pre-hydrated rows the engine's anchor
  handling scrolls the viewport to a stale early anchor when new items
  register mid-stream, breaking the pin. Do not re-enable it without
  re-testing against the `agent-chat-demo` mock workspace.
- **Variable heights.** Activity blocks expand in place as a single
  scroller row (both the step list and per-step inline detail); the
  browser re-derives sizes as rows materialize.
- **Streaming marker** (shimmer status) renders as the last row inside
  the scroller content (not a footer) so the tail snap keeps it
  visible while streaming; the jump-to-latest pill is the restyled
  `MessageScrollerButton` with a direct-DOM snap fallback. **A working
  Activity block already shows the single live line, so the marker is
  suppressed when one is the transcript tail** — `MessageList` passes
  the thread `streaming` flag into `buildTranscriptSlots`, and the
  marker only renders when the tail slot is not a working Activity block
  (it still fills the gap right after send, before any step arrives).

### Activity block (Activity Stream)

`ActivityBlock.tsx` replaces the old per-tool "Thought / Thought /
Ran…" spam with **one card per contiguous run of mechanical steps**
(reasoning items + tool calls). `buildTranscriptSlots` folds a run of
`reasoning` and groupable `tool_call` items into a single `activity`
slot; `activity-steps.ts` holds the pure step/summary/counter/duration
derivations (unit-tested in `activity-steps.test.ts`).

- **Working vs settled is derived.** A run is *working* when the thread
  is `streaming` and the run is the tail run of the active turn;
  everything else *settles*. Working shows a collapsed row — amber
  spinner (`status-working`), bold "Working", a shimmering mono live
  action, a `N done · M running` counter, a rotating chevron. Settled
  rolls up to a green circled check + a derived summary sentence
  (`deriveActivitySummary`: read-heavy → "Explored the codebase",
  command-heavy → "Ran commands", edit-heavy → "Edited files", mixed →
  sensible) + a mono `N steps · 1m 12s` meta (duration from step
  `started_at`/`completed_at`, omitted when < 1s / not derivable, e.g.
  hydrated transcripts) + a Details/Hide toggle. On settle the block
  re-renders collapsed (any mid-stream expansion resets).
- **Shared step rows.** Both states expand to the same mono rows: a dim
  short verb (`read`/`grep`/`edit`/`run`/`think`), a truncating summary
  (from `describeToolCall` / the thought's first line), and a dim
  right-aligned meta (`2 hits`, `+9 −1`, `ok`, `running`, `failed`).
  Working dims completed steps (~0.6) and keeps the running one full
  opacity; settled shows all green checks. Clicking a step row expands
  its full detail inline (`ToolCallBody` / `DiffView` for tools, the
  thought text for reasoning) — there is no side panel for tool output.
- **What stays standalone (breaks the run).** Approval-gated tool calls
  (`approval_request_id` set — the inline approval footer must render),
  TodoWrite / task-summary tools (`TaskSummaryCard` stays a visible
  checklist), `permission_request`/plan/AskUserQuestion rows, assistant
  and user prose. Errored tool calls **do** fold in but stay
  discoverable: a red ✕ step row, and the settled header appends a
  subtle red `· N failed` to the meta. A lone reasoning run with no
  tools keeps rendering as `ReasoningBlock`s (its live streaming text is
  better served there); a lone settled tool call keeps rendering as a
  single `ToolCallCard` (GROUP_MIN is 2 for settled runs, relaxed to 1
  only for a single live working step). `ToolGroupCard` is retired.
- **Timestamps.** `ToolCallItem` gained optional `started_at` /
  `completed_at`, stamped by the pure reducer from its injectable
  `Clock` (backward-compatible optionals so persisted transcripts still
  parse); these plus `ReasoningItem`'s `started_at`/`duration_ms` feed
  the settled duration.

`ChatTranscript` is a thin shell that derives `showThinking`, forwards
the optional `sessionStartedAt` (top session-start divider), and sizes
the list. jsdom tests assert on the slot builder and rendered DOM
(see `MessageList.test.tsx` / `MessageList.virtualization.test.tsx` /
`transcript-slots.test.ts`).

### Navigation trail (turn rail)

`MessageTrail.tsx` renders a slim **navigation trail** in the
transcript's left gutter — one tick mark per user turn — so long threads
can be scanned and jumped without scrubbing the scrollbar. It lives
inside `MessageList`'s `<MessageScroller>` (inside the provider, a
sibling of the viewport), absolutely positioned over the dead left
margin of the `mx-auto max-w-[760px] px-7` transcript column, so it
never overlaps text. It needs **no new setting** — it is part of the
already Beta-gated pane and simply hides on short threads.

- **Data source is `visibleMessageIds`, not `currentAnchorId`.** The
  pure helpers in `message-trail.ts` (`buildTrailEntries` /
  `deriveActiveTrailIndex`) derive one entry per user-turn slot and the
  active turn from the scroller's `useMessageScrollerVisibility()`
  document-order visible list. `currentAnchorId` only reports rows with
  `data-scroll-anchor="true"`, and this transcript keeps
  `scrollAnchor={false}` on **every** row (see below), so it is always
  `null` here — active tracking routes the first visible row's id
  through a `messageId → slotIndex` map because the first visible row is
  usually an assistant / tool-group row, not the user turn itself.
- **Threshold.** Hidden entirely below `TRAIL_MIN_TURNS` (3). The
  subscribing rail only mounts past the threshold, so short threads pay
  nothing for visibility tracking (the engine's tracking is pay-for-use).
- **Bounded density.** A very long thread downsamples evenly to a
  height-derived tick cap (≤ `TRAIL_MAX_TICKS`, 60) so the gutter never
  overflows; the in-view turn's tick is always re-injected so the active
  turn stays represented.
- **Jump behavior.** Clicking a tick calls
  `scrollToMessage(messageId, { align: "start", behavior: "auto",
  scrollMargin })` for a rough jump, then runs a bounded rAF **correction**
  loop: `content-visibility:auto` rows expose only estimated heights, so a
  cold jump across thousands of estimated pixels lands the target well off
  (and re-calling `scrollToMessage` reproduces the same wrong offset), so
  the loop instead nudges `scrollTop` by the target row's *measured* offset
  error each frame until it rests near the top (same spirit as
  `MessageList`'s own mount settle loop). Jumping up fires real scroll
  events, so the `#77` `pinnedRef` unpins itself — no extra coordination.
  The rail overlays the viewport's left gutter but is its **sibling**, so
  its `<nav>` forwards `onWheel` to the viewport's `scrollTop` — otherwise
  wheeling over that 28px strip would be a dead zone. Ticks are real `<button>`s (`Enter`/`Space`,
  focus-visible, `aria-label` "Jump to turn N: …"); the rail is a
  `<nav aria-label="Conversation turns">`. Hovering a tick shows one
  shared preview card (prompt + reply start) after a short delay.
- **Anchoring stays off.** The trail is the reason turn anchoring can
  stay disabled: it gives long threads their jump affordance without
  re-enabling the engine anchor handling that breaks the stick-to-bottom
  pin on hydrated transcripts. Unit tests cover the pure helpers
  (`message-trail.test.ts`) and the component (`MessageTrail.test.tsx`).

## Subagent view (cross-provider)

When a provider session delegates to subagents, the chat pane shows a
**Subagents orchestration card** in the transcript and lets the user
**enter** a subagent for a read-only, in-pane drill-in of its own stream.
One canonical model drives all three providers. Design spec:
`docs/plans/assets/Subagents.dc.html`; locked decisions:
`docs/plans/subagent-view.md`.

### Canonical event model (`agent_provider/events.rs`)

Two additive, backward-compatible changes to `ProviderRuntimeEvent`
(every new field is `#[serde(default)]`, so payloads persisted before
subagents existed still deserialise, and the frontend merges non-null
fields into per-subagent view state):

- A new event `SubagentUpdated { thread_id, subagent }` carrying a
  merge-able `SubagentSnapshot`. Serialised (snake_case) fields:
  `subagent_id` (stable demux key, required), `parent_item_id?`,
  `name?`, `agent_type?`, `model?`, `status`
  (`pending|running|completed|failed|stopped`), `activity?`,
  `result_text?`, `tool_use_count?`, `total_tokens?`, `duration_ms?`,
  `provider_ref?`. All but `subagent_id` / `status` are optional because
  providers dribble a subagent's identity out across many events.
- A `subagent_id: Option<String>` tag on `ContentDelta`,
  `ItemCompleted`, and `RequestOpened`. Non-null routes the (otherwise
  unchanged) item / delta / approval into that subagent's sub-transcript
  instead of the parent flow — so the drill-in reuses every existing
  renderer, and a child's permission request still bubbles into the
  parent view (labelled "from subagent X") rather than stalling the turn
  invisibly.

Subagent events are emitted thread-scoped under the **parent**
`thread_id`; the `subagent_id` inside the payload is the demux key. They
persist to `agent_chat_messages` under the parent thread as usual
(`should_persist_event` also persists `SubagentUpdated`), so DB hydrate
replays cards + child transcripts through the same reducer with **zero
schema migration**. The TypeScript mirror lives in `src/tauri/events.ts`.
The backend also consumes these `SubagentUpdated` statuses to keep the
left-sidebar working spinner alive while subagents outlive the parent
turn — see [Sidebar status indicators](#sidebar-status-indicators).

### Per-provider keying + mapping

- **Claude** (`claude/translate.rs`, keying = spawning
  `parent_tool_use_id`): an assistant `tool_use` block named `Agent`
  **or** `Task` spawns the row; every SDK message's top-level
  `parent_tool_use_id` routes inner items into the sub-transcript; the
  previously-dropped `task_started` / `task_progress` / `task_updated` /
  `task_notification` system events become status + activity + usage
  (mapped via a `task_id → tool_use_id` table), with the background
  `async_launched` tool_result deferred to a later `task_notification`
  for completion. The sidecar sets `agentProgressSummaries: true` (the
  only sidecar change) so `task_progress.summary` feeds the activity line.
- **Codex** (`codex/{protocol,translate,session}.rs`, keying = child
  `threadId`): a persistent per-session demux routes wire events by
  `params.threadId`; `collabAgentToolCall` / `subAgentActivity` map to
  spawn/lifecycle + `agentsStates[..].message` activity (raw rendering
  suppressed); child `thread/started` / `turn/*` update subagent status
  and never the parent turn. `thread/started`, `turn/started`, and
  `turn/completed` decode **both** the legacy flat and the v2 nested wire
  shapes (custom `Deserialize`), and the success arm accepts both
  `succeeded` and `completed`.
- **OpenCode** (`opencode/{protocol,translate,sse,client}.rs`, keying =
  child `sessionID`): an `SseRouter` grows a watched-session set (parent
  ∪ transitive descendants via `SessionInfo.parentID`) instead of
  dropping child-session events; the `task` tool part decodes
  `state.metadata.{parentSessionId,sessionId,model}` (note the camelCase
  lowercase-`d` keys) and `time.{start,end}`, emitting the row on the
  first `running` update (`pending` carries no metadata) and stripping
  the `<task_result>` / `<task_error>` envelope for `result_text`. Child
  `permission.asked` bubbles as a `RequestOpened` and its reply is POSTed
  to the **child** session's permissions endpoint. `question.asked`
  remains a known gap (a different reply endpoint the current
  `ApprovalDecision` model can't express — decoded as `Other`, no
  regression).

### Frontend (`src/lib/agent-chat/`, `src/components/chat/`)

- `types.ts` adds `SubagentView` and a `SubagentRunItem`
  (`kind: "subagent_run"`) `ChatViewItem`; `reducer.ts` merges snapshots
  (non-null wins, status is monotonic and never regresses to pending) and
  routes `subagent_id`-tagged events into the subagent's own `items` via
  shared item-builders (one card per contiguous spawn group; a new turn
  ⇒ a new card; per-subagent cap 500). Pure helpers live in
  `subagents.ts`; `subagent_run` is a standalone unfoldable transcript
  slot.
- `SubagentsCard.tsx` — the orchestration card: aggregate header
  ("N tasks · running in parallel" / "X done · Y active"), one row per
  subagent (status spinner/check/x, name + mono model, shimmering
  activity line while running / muted result when done, mono
  `elapsed · N tools` meta, Enter button, chevron), an inline "Recent
  activity" peek (last 3 child tool rows + "Enter subagent"), and the
  footer note.
- `SubagentView.tsx` + `SubagentBreadcrumb.tsx` — the read-only drill-in:
  a `← Orchestrator › ⟨ordinal⟩ Name` breadcrumb with model chip and
  right-aligned blinking status, a tone-tinted read-only banner, the
  sub-transcript folded through `buildTranscriptSlots` + the existing
  renderers, and a live shimmer tail while running.
- `AgentChatPane.tsx` holds `viewMode` (orchestrator ↔ subagent) local
  state; entering swaps the transcript body and the sub-header for the
  breadcrumb, Esc / back returns, and the composer stays parent-bound
  with the placeholder "Steering goes to the orchestrator…".
  `AgentChatPaneHeader.tsx` shows an amber blinking "N subagents running"
  pill while any subagent runs. All tones consume design-system tokens
  (running = `status-working`, completed = `status-open`, failed =
  `status-attention`); `.cm-blink` in `globals.css` honors
  reduced-motion.

The dev mock seeds one subagent turn in the demo transcript and exposes
`window.__codemuxChatMock.streamSubagents()` for a live two-subagent
lifecycle over the real per-thread Channel.

**Workflow runs are a Claude-only relative of this system.** When a
Claude session runs a top-level `Workflow` tool_use (a script that
coordinates many subagents across named phases), the same translate
layer emits a separate `workflow_updated` event instead of treating it
as an ordinary subagent, and top-level subagents spawned while that
workflow is active get stamped with a `workflow_id`/`phase` so they
route into the workflow's phases rather than the generic Subagents card.
The chat pane renders this as a `WorkflowRunCard` (approval / running /
summary) and a conditional **Orchestration** right-panel tab instead of
the inline Subagents card + drill-in described above. See
`docs/features/workflow-orchestration.md` for the full model; this file
does not duplicate it.

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

The three `fake_*` helper `[[bin]]` targets are gated behind the
`test-fixtures` cargo feature (`required-features` in
`src-tauri/Cargo.toml`). Tauri's CLI bundles every ungated `[[bin]]` of
the app package, so without the gate the fixtures shipped in the
deb/rpm/AppImage at `usr/bin` (through v0.9.1). A self-referential
dev-dependency turns the feature on for `cargo test`, so plain
`cargo test` still builds the fixtures and `env!("CARGO_BIN_EXE_fake_*")`
resolves — no `--features` flag needed anywhere.

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

**Current state.** Full SDK host. The sidecar depends on
`@anthropic-ai/claude-agent-sdk` and exposes 13 JSON-RPC methods
(session lifecycle, turn streaming, approvals, probes — see "Exposed
RPCs" under "Claude Agent SDK integration" below). The original
`ping` scaffold method remains as the liveness probe.

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
registering handlers in `buildMethods` in
`sidecar/claude-agent/src/methods/index.ts` (dispatched from `main.ts`).

**Workspace env injection.** `agent_chat_start_session` overlays the
chat pane's workspace env onto `StartSessionInput.env` before the
provider consumes it (`workspace_env_overlay` in
`src-tauri/src/commands/agent_chat.rs`): `CODEMUX=1`,
`CODEMUX_WORKSPACE_ID`, `CODEMUX_PANE_ID` (the chat pane),
`CODEMUX_BROWSER_CMD`, `BROWSER`, plus the workspace-level vars from
the terminal path's `workspace_pty_env` helper (reused `pub(crate)` so
the two surfaces stay in lockstep). The Claude and Codex adapters pass
`input.env` through to their per-session child spawns, so the agent's
Bash subprocesses carry `CODEMUX_WORKSPACE_ID` and `codemux browser
open` routes to the agent's own workspace instead of falling into the
control layer's legacy active-workspace path (the pre-fix bug: with
the beta on, browser panes landed in whatever workspace the user was
viewing). Caller-provided env entries always win (insert-if-absent);
an orphaned pane (no owning workspace) injects nothing. OpenCode's
shared long-lived server cannot take per-session env — it relies on
the control layer's cwd fallback (`resolve_workspace_id_by_cwd` in
`src-tauri/src/control.rs`; see `docs/features/browser.md`).

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
| `update-mcp-tools` | Push an updated MCP tool list into the live session (`query.setMcpServers`); lets servers that come up after `start-session` surface without a chat restart. Empty list removes the codemux MCP. Idempotent. |
| `respond-to-request` | Resolve a pending `canUseTool` approval. |
| `respond-to-user-input` | Answer an `AskUserQuestion` prompt. |
| `initialization-result` | Read the SDK's cached init payload. |
| `stop-session` | Close a session; idempotent. |
| `probe-installed` | Shell out to `<binary> --version`. |
| `probe-authenticated` | Shell out to `<binary> auth status`. |
| `ping` | Liveness probe from the scaffold. |

Deliberately NOT exposed (per the integration research): the ~15
other `Query` methods (`setMaxThinkingTokens`, `applyFlagSettings`,
`supportedCommands`, `supportedModels`, `supportedAgents`,
`mcpServerStatus`, `getContextUsage`, `reloadPlugins`, `accountInfo`,
`rewindFiles`, `seedReadState`, `reconnectMcpServer`,
`toggleMcpServer`, `streamInput`, `stopTask`). These ship as
follow-ups only when UI calls for them. (`setMcpServers` graduated
from this list — `update-mcp-tools` wraps it.)

### Options construction

Exactly 16 SDK `Options` fields are populated (`cwd`, `model`,
`pathToClaudeCodeExecutable`, `settingSources: ["user","project","local"]`,
`effort` (cast through `unknown` for forward-compat), `permissionMode`,
`allowDangerouslySkipPermissions` (only with `bypassPermissions`),
`settings` (when non-empty), `resume`, `sessionId`,
`includePartialMessages: true`, `agentProgressSummaries: true` (so
`task_progress` events carry a human-readable `summary` for the subagent
card's activity line), `canUseTool`, `env: process.env`,
`additionalDirectories` (when non-empty), `extraArgs` (when non-empty)).
The other 30+ fields in the SDK's Options surface are intentionally
left unset — they become features when the UI surfaces them.

### Testing

The sidecar ships with 70 Bun tests across six files: ping/liveness,
session unit tests using a `FakeQuery` injected via
`setQueryFactoryForTests`, permissions tests exercising the canUseTool
bridge, MCP-bridge tests, respond-to-request tests, and Stage 3
real-tools coverage. The Rust
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

**GUI chrome suppression.** When the Beta flag is on and the chat pane is
the **sole root** of its surface (not a split), `AgentChatPaneHeader` does
NOT render — the title bar absorbs the tab, its session-history dropdown,
close, "Restore checkpoint", and the "N subagents running" pill (`PaneNode`
gates on `isSurfaceRoot`). In split layouts the per-pane header still
renders, so split/close/drag keep working. The session-switch orchestration
(stop → hydrate → resume), the checkpoint-restore state, and the grouped
session list were extracted into shared pieces so the legacy per-pane header
and the titlebar tab drive one implementation, not a fork:
`src/hooks/use-agent-chat-session-actions.ts`,
`src/hooks/use-agent-chat-checkpoint-restore.ts` +
`src/components/chat/restore-checkpoint-dialog.tsx`, and
`src/components/chat/session-history-menu.tsx` (`SessionHistoryList` +
`useSessionHistory`, now consumed by both `SessionSelector` and the
title-bar chat tab). See `docs/features/gui-chrome.md`.

## Tauri command surface

Session/lifecycle commands are gated on the `enable_agent_chat` feature
flag and return a `feature_disabled: ...` string when the flag is off;
session commands also return a `provider_not_configured: ...` string
when the target provider is missing from the registry. The read-only /
DB-only history commands (`agent_chat_list_sessions`,
`agent_chat_get_session`, `agent_chat_update_session_config`,
`agent_chat_get_checkpoint`, rename/delete) are intentionally NOT
flag-gated, so a pane re-seeds or falls back to defaults rather than
surfacing a `feature_disabled` string.

| Command | Purpose |
|---|---|
| `agent_chat_create_pane` | Insert a new chat pane in a workspace, returns the new pane id. |
| `agent_chat_close_pane` | Close a chat pane. Idempotent — double-close is a no-op. |
| `dev_agent_chat_spawn_test_pane` | Debug-only. Spawns a chat pane in the active workspace for manual QA from the browser devtools. |
| `agent_chat_start_session` | Start a provider session, writing the returned thread id back onto the pane. |
| `agent_chat_send_turn` | Queue a user turn on a thread. Auto-resumes a dead session (rebuilt from its persisted row) before the turn, so a pane reopened after an app restart never sees `session_not_found`. |
| `agent_chat_interrupt_turn` | Halt the currently-running turn. A `SessionNotFound` (nothing to interrupt on a dead session) is swallowed into `Ok`; does NOT auto-resume. |
| `agent_chat_respond_to_request` | Resolve a pending approval / input request. Auto-resumes a dead session first (same choke point as `send_turn`). |
| `agent_chat_set_model` | Swap a thread's model. Persist-always, apply-if-live: writes the model to the session row unconditionally, then applies to the live session if one exists; a `SessionNotFound` from the apply is swallowed into `Ok` (the value takes effect on the next auto-resume). |
| `agent_chat_set_permission_mode` | Swap a thread's permission mode. Same persist-always, apply-if-live contract as `agent_chat_set_model`. |
| `agent_chat_stop_session` | Gracefully close a session. Idempotent. |
| `agent_chat_get_session` | Return a single persisted session row by thread id, INCLUDING rows whose `sdk_session_id` is still null (unlike `agent_chat_list_sessions`), with the per-thread config columns. Not flag-gated. Used by the pane's mount-seed to restore pickers + resume cursor after a restart. |
| `agent_chat_update_session_config` | Persist a per-thread config patch (`model` / `effort` / `context_window` / `permission_mode`) to the session row. DB-only, no live-session requirement; only the provided fields overwrite (`COALESCE`). Not flag-gated. Fired fire-and-forget by the picker handlers. |
| `agent_chat_get_checkpoint` | Return the run-start rollback checkpoint recorded for a thread (or null). Not flag-gated — renders as "no checkpoint" instead of an error. |
| `agent_chat_restore_checkpoint` | Roll the workspace back to the thread's run-start checkpoint. Mutates the working tree; the UI confirms first. |

Provider errors are serialized as `SerializableProviderError` JSON so
the UI can inspect the error subtype (e.g.
`{"kind":"not_authenticated", ...}`) instead of parsing a free-form
string.

## Run checkpoints (issue #80)

Opt-in rollback point taken when a chat session starts. Off by
default; toggled via Settings → Agent → "Checkpoint before agent
runs" (`UserSettings.agent_chat.checkpoints_enabled`, synced
settings).

- **Background, never on the first-token path.** `agent_chat_start_session`
  spawns `tauri::async_runtime::spawn` + `spawn_blocking` AFTER the
  provider session is up; even the settings-cache read happens inside
  the spawned task.
- **Non-destructive snapshot.** `git_checkpoint_create`
  (`src-tauri/src/git.rs`) stages the worktree (tracked + untracked,
  `.gitignore` respected) into a temporary `GIT_INDEX_FILE`, writes a
  tree, and `commit-tree`s it onto HEAD — the user's index, worktree,
  and stash list are untouched and no hooks run. The commit is
  anchored at `refs/codemux/checkpoints/<sanitized-thread-id>` so gc
  can't reap it. Skipped silently for non-repos and unborn-HEAD repos.
- **Recorded per thread.** `agent_chat_checkpoints` table (FK →
  `agent_chat_sessions`, ON DELETE CASCADE) stores snapshot commit,
  HEAD commit, branch, repo path, ref name. On success the backend
  emits `agent_chat_checkpoint` (`{thread_id, checkpoint}`); the
  frontend hook `useAgentChatCheckpoint` (event + on-mount fetch)
  feeds the pane header's restore button (history icon, hover-reveal,
  hidden when no checkpoint, disabled mid-turn).
- **Restore** (`git_checkpoint_restore`): refuses on branch mismatch
  or pruned commits; takes a safety snapshot of the current state to
  `refs/codemux/pre-restore/<id>` first; then `read-tree --reset -u
  <snapshot>` + `clean -fd` + `reset --mixed <pre-run HEAD>`. Result:
  run commits undone, run-created files deleted (ignored files
  spared), pre-run changes back as unstaged edits / untracked files.
  Known loss: the pre-run staged/unstaged split flattens to unstaged.
- **Pruning.** Each create prunes both `refs/codemux/` namespaces to
  the 20 newest refs and drops the matching bookkeeping rows.

Design note: `docs/plans/agent-run-checkpoint.md`.

## Event bridge

Every registered provider's canonical event stream is forwarded to
the frontend per thread over a `tauri::ipc::Channel` (issue #75).
Payloads (`AgentChatEventPayload`) carry the originating `thread_id`
alongside the raw `ProviderRuntimeEvent`.

Transport split:

- **Thread-scoped events** — including the high-frequency streaming
  `content_delta` tokens — are routed to the `Channel` the owning
  pane registered via `attach_agent_chat_output(thread_id, channel)`
  (`AgentChatChannelRegistry` in `commands/agent_chat.rs`, mirroring
  the PTY `attach_pty_output` pattern). A pane only ever receives its
  own thread's events; nothing is broadcast app-wide. Tauri's event
  system is explicitly not designed for high-throughput streaming;
  Channels are the documented mechanism for it.
- **Thread-less events** (global `RuntimeWarning`s) keep the
  low-frequency `agent_chat_event` global event bus, with an empty
  `ThreadId`.

Replay split: the channel carries **live events only**. Transcript
events are persisted to SQLite by `forward_event` whether or not a
channel is attached, and a late-attaching / resumed pane rebuilds
history through the DB hydrate (`agent_chat_list_messages` →
`hydrateThread`). Non-persisted lifecycle notices that fire while no
pane is attached are dropped — the same outcome the event bus had
for unmounted panes.

Attach/detach lifecycle: `attach_agent_chat_output` returns a
generation token; `detach_agent_chat_output(thread_id, generation)`
only removes a matching generation, so a stale unmount can never tear
down the channel a newer pane just installed. The frontend hook
`useAgentChatEvents(threadId, handler)` in
`src/hooks/use-agent-chat-events.ts` owns this lifecycle: it attaches
a `Channel` per mounted thread and serializes its detach behind the
attach promise.

The bridge is a thin loop: one background Tokio task per provider,
each consuming the provider's `event_stream()` and routing each
event through `forward_event`. `broadcast::error::RecvError::Lagged`
is already swallowed by each provider's event-stream helper, so slow
subscribers never crash the loop — they just drop old events.

## Sidebar status indicators

Chat sessions publish into the same `pane_statuses` snapshot the left
sidebar reads, so a chat workspace shows the working spinner / red
needs-input pulse / green ready-for-review dot exactly like a terminal
agent (previously chat panes showed nothing). `forward_event` in
`commands/agent_chat.rs` maps each `ProviderRuntimeEvent` to a
`PaneStatus` (`map_event_to_pane_status`) and writes it through
`AppStateStore::set_pane_status_by_thread`, which resolves the
`thread_id` to its `AgentChat` pane (`find_agent_chat_pane_id`) — the
chat-side analogue of the terminal hooks path in `hooks.rs`. Mapping:
streaming deltas / committed items / a running session / a resolved
request → `Working`; an opened approval / plan / AskUserQuestion (or a
session parked on approval) → `Permission`; `TurnCompleted` → `Review`
(downgraded to `Idle` when the pane's workspace is already active,
mirroring `handle_lifecycle_event`); session `Closed`/`Error` → cleared.
A change-guard on `set_pane_status_by_thread` means the `content_delta`
token stream collapses to a single write per turn rather than one per
token. Because the write lands in `pane_statuses`, the sidebar rows,
collapsed-rail aggregate dot, hover flyout, and `PaneNode` borders all
update for free.

**Subagents keep the spinner alive past `TurnCompleted`.** The primary
chat turn can finish while subagents it spawned are still running, so the
mapping is stateful: `map_event_to_pane_status` takes the event *plus* a
per-thread `ThreadSubagentState` (running `subagent_id`s + a
`review_pending` flag), held in the Tauri-managed `SubagentTracker`
(`Mutex<HashMap<thread_id, _>>`, one lock per decision, registered in
`lib.rs` alongside `AgentChatChannelRegistry`). `SubagentUpdated` with a
`running`/`pending` status records the subagent; a terminal status
(`completed`/`failed`/`stopped`) drops it. `TurnCompleted` publishes
`Review` only when no subagents are tracked — otherwise it *holds
`Working`* and marks `review_pending`, and the deferred `Review` fires
when the last subagent goes terminal. The working indicator therefore
persists until the turn **and** all tracked subagents finish, matching the
still-running `SubagentsCard` in the drill-in.

**A new user turn resets the thread's tracker** (both `running` and
`review_pending`). The authoritative, provider-agnostic anchor is the
send-message command path: `agent_chat_send_turn` calls
`SubagentTracker::clear_thread` before dispatching the turn. This matters
because a provider can leave a subagent non-terminal — Claude's background
`async_launched` task stays `Running` until a later `task_notification`
that may never arrive — and without the reset that stale `running` id
would hold `Working` and suppress `Review` for *every* later turn until
session close. `SessionStateChanged::Running` also resets the tracker
(reliably per-turn for Codex's `turn/started` and OpenCode's
`session.status = busy`; Claude does not fire it per user turn, hence the
command-path clear). A genuinely-still-alive background subagent
re-inserts itself on its next `Running` snapshot — Claude re-emits these
via `task_progress` ticks — so clearing at turn start is safe. Crucially,
parent-scoped `content_delta`/`item_completed` (`subagent_id == None`) do
**not** clear `review_pending`: such output can trickle in after a deferred
`TurnCompleted`, and dropping the owed `Review` there would strand the
pane at `Working`. `review_pending` is cleared only by publishing the
owed/normal `Review`, a new-turn reset, session `Closed`/`Error`, or
`agent_chat_close_pane`.

Only *live* provider events reach the tracker: transcript hydration/resume
replays persisted events through the frontend reducer
(`agent_chat_list_messages` + `hydrateThread`), never through
`forward_event`, so there is no backend replay to guard against. Session
`Closed`/`Error` and explicit `agent_chat_close_pane` also clear the
thread's tracking so a stuck subagent can never pin the spinner after the
session is gone.

## Follow-up queueing

Sending a chat message while the agent is mid-turn **queues** it instead
of rejecting it. Previously the frontend optimistically appended the user
bubble and then `agent_chat_send_turn` failed with
`ProviderError::ValidationError("session has an active turn …")`, leaving
an orphaned bubble + a toast + a message that never got answered. Now the
send is queued and auto-dispatched as the next turn.

**Backend-authoritative queue.** The queue lives in Rust session state
(`queued_turns: VecDeque<QueuedTurn>` on both `claude/session.rs` and
`codex/session.rs`), so it survives pane unmount/remount. The busy branch
of `send_turn` enqueues rather than erroring (`enqueue_or_send`); the
busy check re-runs under the state lock, so there is no TOCTOU with a
turn that finished between the frontend send and the backend enqueue.
`TurnStartResult` gained an optional `queued_id` — set when the send was
queued (its `turn_id` is then an empty placeholder). OpenCode has no busy
guard and no queue; `cancel_queued_turn` is a trait-default no-op there.

**Draining.** On the turn-completion transition (Claude: the SDK `result`
message → Ready; Codex: `turn/completed` → Ready), and after
interrupt/abort, the session pops the next queued turn (FIFO) and
dispatches it through the same internal send path
(`drain_queue`). Dispatch happens **after** the completion handler
releases the session lock, so `do_send` can re-acquire it without
deadlocking. Draining only fires when the session is truly idle (Ready,
no active turn, no pending approval). On session error/close the whole
queue is cancelled (`cancel_all_queued`). A failed dispatch cancels that
item and continues — the queue never wedges.

**Events** (`ProviderRuntimeEvent`, forwarded through `forward_event`):

- `TurnQueued { thread_id, queued_id, client_nonce, text }` — a send was
  parked. `client_nonce` echoes the optimistic-send correlation token so
  the reducer greys the already-appended bubble instead of duplicating
  it; `text` lets a remounted pane reconstruct the bubble.
- `QueuedTurnDispatched { thread_id, queued_id, turn_id, text }` — the
  queued turn is now active. `forward_event` persists the user-message
  envelope **here** (not at enqueue time) so chat history reflects real
  turn order. The reducer promotes the greyed bubble to a normal user
  message and re-seqs it to the dispatch-time tail.
- `QueuedTurnCancelled { thread_id, queued_id }` — the queued turn was
  cancelled (by the user or by session close/error). The reducer removes
  the greyed bubble.

**Command.** `agent_chat_cancel_queued_turn(provider, thread_id,
queued_id)` cancels a queued turn (idempotent). Wrapper:
`agentChatCancelQueuedTurn` in `src/tauri/commands.ts`.

**Frontend.** `UserMessageItem` gained `queued?: { queuedId }` (greyed
render + "Queued" pill + hover-X) and `clientNonce?` (reconciliation +
error rollback). Queued items sort to the very bottom via a `QUEUED_SEQ_BASE`
offset so they stay below the streaming turn. The composer's `handleSubmit`
attaches a `client_nonce`, treats a queued result as success (the
`turn_queued` event greys the bubble), and on a genuine RPC failure rolls
the optimistic bubble back and restores the draft (fixes the orphan bug).
The Composer allows Enter-to-queue while streaming (blocked only during
the in-flight send RPC) and shows a subtle "Enter to queue" hint;
cancelling a queued item restores its text into the composer draft.

**Persistence/hydrate note.** The queue is in-memory session state; there
is no "get session status" command that carries a queued-turns snapshot,
so a pane that remounts mid-queue does NOT re-render the still-pending
queued items until they dispatch (they still drain correctly
backend-side). Adding the snapshot to a status command is a possible
follow-up.

**Out of scope (TODO):**

- **Steering / inject-into-live-turn** (Cmd+Enter "send now" that injects
  into the running turn rather than queuing behind it).
- **Editing a queued message in place** (cancel-restores-to-composer
  covers the common case).
- **Persisting the queue across app restart.**
- **OpenCode queue parity.**

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

2. **Config file:** edit the per-build snapshot at
   `~/.local/share/codemux/observability.json` (release) or
   `~/.local/share/codemux-dev/observability.json` (debug build), set
   `feature_flags.enable_agent_chat: true`, restart.

3. **Fresh install:** the default store is persisted lazily, so a
   brand-new Codemux install has no file yet. Start the app once
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
work. It also seeds one **subagent turn** (an "Implement" completed +
"Verify" running orchestration card) and exposes
`window.__codemuxChatMock.streamSubagents()` for a live two-subagent
lifecycle over the real Channel — the harness for the subagent view.

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
