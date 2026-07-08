# Subagent View (cross-provider)

- Purpose: Locked implementation plan for the cross-provider subagent view in
  Agent Chat — see when subagents launch / work / finish, and enter one to
  watch its own transcript.
- Audience: Implementation agents and anyone changing agent-chat internals.
- Authority: Landed work plan (all stages shipped via the `subagent-view`
  branch, merged to `main` in PR #125 — see "Already Landed" below).
  Canonical current truth now lives in
  `docs/features/agent-chat.md` § "Subagent view (cross-provider)"; this doc
  is retained for the locked decisions and the implementation record.
  Protocol facts below were research-verified (July 2026) against SDK type
  declarations, generated schemas, and a live `opencode serve` capture.
- Update when: Stages land or a locked decision changes.
- Read next: `docs/features/agent-chat.md`,
  `docs/features/multi-provider-chat.md`, `docs/reference/DESIGN-SYSTEM.md`,
  design mock at `docs/plans/assets/Subagents.dc.html`.

## Goal

When a provider session delegates work to subagents, the chat pane must show
a **Subagents orchestration card** in the transcript (one row per subagent:
status, name, model, live activity line, elapsed + tool count, expandable
peek, Enter button) and support **entering** a subagent — a read-only,
in-pane drill-in showing that subagent's own stream with a breadcrumb back to
the orchestrator. One canonical model, three providers (Claude, Codex,
OpenCode). The visual spec is `docs/plans/assets/Subagents.dc.html`
(implement with design-system tokens, never its hardcoded oklch values).

## Current state (pre-implementation baseline, research-verified July 2026)

> **Historical.** This section captures the pre-implementation baseline that
> the stages below have since resolved. It is kept as the research record;
> for what is true today read `docs/features/agent-chat.md` § "Subagent view
> (cross-provider)".

**No layer of Codemux currently understands subagents.**

- `ProviderRuntimeEvent` (`src-tauri/src/agent_provider/events.rs`) has no
  parent/subagent concept. Only `SessionConfigured`, `ContentDelta`,
  `ItemCompleted`, `TurnCompleted`, `RequestOpened`, `RequestResolved`,
  `SessionStateChanged`, `RuntimeWarning`, `ResumeCursorUpdated`.
- **Claude**: sidecar forwards all SDK messages raw (good — no sidecar change
  needed for demux); `claude/translate.rs` never reads `parent_tool_use_id`,
  so subagent inner messages flatten indistinguishably into the parent
  transcript; `task_started/updated/progress/notification` system subtypes
  are explicitly routed to `RuntimeWarning` and dropped.
- **Codex**: `codex/session.rs::spawn_notifications_task` ignores the wire
  `threadId` — child-thread events would blend into the parent or drop.
  `collabAgentToolCall`, `subAgentActivity`, `enteredReviewMode`,
  `exitedReviewMode` item types fall to the warning arm.
- **OpenCode**: `opencode/sse.rs::event_matches_session` hard-drops every
  event whose `sessionID` differs from the active session — all child-session
  activity is discarded. `ToolStateValue` doesn't decode `state.metadata`
  (which carries the child session id), `Pending`/`Running` tool states emit
  nothing, `SessionInfo` doesn't decode `parentID`, and `subtask`/`agent`
  part types are swallowed by `PartPayload::Other`.
- Frontend: `ChatViewItem` has no subagent kind; the reducer appends tool
  items flat; `tool-visuals.ts` has no `Task` entry (falls to Wrench);
  `TaskSummaryCard` is **TodoWrite-only — do not repurpose it**.

## Locked decisions

1. **Canonical model, defined once in `events.rs`.** Two additions:
   - A new event: `SubagentUpdated { thread_id: ThreadId, subagent: SubagentSnapshot }`
     where `SubagentSnapshot` is a **merge-able state snapshot**:
     ```rust
     pub struct SubagentSnapshot {
         pub subagent_id: String,          // stable key (see per-provider keying)
         pub parent_item_id: Option<String>, // tool_use/call id that spawned it
         pub name: Option<String>,          // "Explore", nickname, agent name
         pub agent_type: Option<String>,    // subagent_type / role / agent
         pub model: Option<String>,
         pub status: SubagentStatus,        // Pending | Running | Completed | Failed | Stopped
         pub activity: Option<String>,      // live "currently doing X" line
         pub result_text: Option<String>,   // final report (on completion)
         pub tool_use_count: Option<u64>,
         pub total_tokens: Option<u64>,
         pub duration_ms: Option<u64>,
         pub provider_ref: Option<String>,  // provider-native id (codex threadId, opencode sessionID, claude agentId)
     }
     ```
     Every field except id/status is `Option` — providers dribble identity
     out across events (Synara's "identity directory" lesson); the frontend
     merges non-None fields into its per-subagent state. Serde: additive,
     `#[serde(default)]` everywhere, so old persisted rows keep
     deserializing.
   - A new field on the existing item event: `ItemCompleted { thread_id,
     item, subagent_id: Option<String> }` (`#[serde(default)]`). Child
     transcript items reuse `CompletedItem` unchanged, so the drill-in view
     reuses every existing renderer. Same optional field on `ContentDelta`
     for live child streaming where available (Claude stream events carry
     `parent_tool_use_id`; Codex deltas carry the child threadId).
2. **Subagent transcripts ride the parent thread.** No synthetic child
   threads (Synara's string-encoded `subagent:` ids leaked everywhere and
   needed SQL LIKE recovery hacks). Events persist to `agent_chat_messages`
   under the parent `thread_id` as today; the `subagent_id` inside the
   payload is the demux key. DB hydrate replays through the same reducer, so
   cards + child transcripts survive restart with **zero schema migration**.
   `should_persist_event` additionally persists `SubagentUpdated`.
3. **Drill-in is in-pane view state, not a new pane/session.** `AgentChatPane`
   gets `viewMode: { kind: "orchestrator" } | { kind: "subagent", subagentId }`
   local state. Entering swaps the transcript body for the subagent's
   sub-transcript and swaps the pane sub-header for the breadcrumb header.
   The composer stays wired to the parent thread (design: "steering goes to
   the orchestrator"), with placeholder switched to "Steering goes to the
   orchestrator…". Read-only banner on top of the drill-in stream.
4. **Child permission requests must bubble.** A subagent's approval request
   (all three providers can raise one) surfaces in the parent view exactly
   like today's `RequestOpened` — otherwise the turn stalls invisibly
   (observed live with OpenCode). Tag `RequestOpened` with
   `subagent_id: Option<String>` (`#[serde(default)]`) so the UI can label it
   "from subagent X" and also show it inside the drill-in.
5. **One orchestration card per contiguous spawn group.** The reducer emits a
   `SubagentRunItem` view item when the first `SubagentUpdated` of a turn
   arrives; subsequent subagents in the same turn join the same card (design
   shows one card with N rows + aggregate header "N tasks · running in
   parallel", "X done · Y active"). New turn ⇒ new card.
6. **Activity line precedence** (per row): provider-pushed summary (Claude
   `task_progress.summary` / Codex `agentsStates[..].message`) → latest child
   tool call rendered as `verb target` (fallback derived from the child's own
   sub-transcript — the fallback Synara never built) → status text. Done rows
   show `result_text` first line or "Done".
7. **Design tokens only.** Running = `status-working` (amber), completed =
   `status-open` (green), failed = `status-attention` (red), stopped/idle =
   muted. Row accent tones may cycle the design's tone set via a
   deterministic hash of `subagent_id`. JetBrains Mono for activity/meta
   lines, DM Sans for names. No hardcoded colors — consume
   `DESIGN-SYSTEM.md` tokens.

## Per-provider mapping (protocol ground truth)

### Claude (Agent SDK via sidecar) — `claude/translate.rs`

Keying: `subagent_id = parent_tool_use_id` of the spawning tool_use block.

- **Launch**: assistant `tool_use` block named `"Agent"` **or** `"Task"`
  (renamed in CLI v2.1.63; `system:init.tools` still says `Task` — match
  both). Input: `{ description, prompt, subagent_type?, model?,
  run_in_background?, name?, mode? }`. Emit `SubagentUpdated` (Pending→
  Running) with name = `name ?? subagent_type ?? description`. Suppress the
  generic `ToolUse` item for this block (the card replaces it).
- **Inner transcript**: every SDK `assistant` / `user` / `stream_event`
  message carries `parent_tool_use_id: string | null` at the top level of
  the wire message (NOT inside `message`). Non-null ⇒ route the translated
  items with `subagent_id = parent_tool_use_id` instead of flat append.
  Nested subagents (depth ≤ 5): a grandchild's `parent_tool_use_id` is the
  *child's* Agent tool_use id — keep a set of known Agent tool_use ids seen
  inside sub-transcripts; v1 renders grandchildren flattened into their
  parent subagent's stream (no recursive drill-in yet).
- **Live progress**: `system` subtypes currently dropped as warnings become
  signal: `task_started { task_id, tool_use_id?, description }`,
  `task_progress { task_id, tool_use_id?, usage: { total_tokens, tool_uses,
  duration_ms }, last_tool_name?, summary? }`, `task_updated { task_id,
  patch: { status?, ... } }`, `task_notification { task_id, tool_use_id?,
  status: completed|failed|stopped, summary, usage? }`. Map by
  `tool_use_id` → `subagent_id` (keep a `task_id → tool_use_id` map).
  `summary` needs `Options.agentProgressSummaries: true` — add it to the
  sidecar options construction (one-line sidecar change; everything else is
  Rust-side).
- **Completion**: parent-level `user` message with `tool_result` for the
  spawning id; the wire message's `tool_use_result` field is a structured
  `AgentOutput { status: "completed", agentId, totalToolUseCount,
  totalDurationMs, totalTokens, usage, toolStats? }` — use it for final
  meta. **Background variant (default since CLI 2.1.198!):** the tool_result
  arrives immediately with `status: "async_launched"` — do NOT mark done;
  completion arrives later via `task_notification`. Synthetic user messages
  with `origin: { kind: "task-notification" }` / `isSynthetic: true` must
  not render as user bubbles (suppress; the card already shows the state).
- **Usage hygiene**: drop non-null-`parent_tool_use_id` `message_delta`
  usage from the parent's token accounting (t3code's trick).
- Available agent types (for future use): sidecar `initialization-result`
  already returns `agents: AgentInfo[]`.

### Codex (`codex app-server`) — `codex/{protocol,translate,session}.rs`

Keying: `subagent_id = child threadId`. Multi-agent is **stable and enabled
by default** (`multi_agent` feature). Reference schema: `codex app-server
generate-json-schema` (local CLI 0.142.5).

- **Demux first**: `spawn_notifications_task` must route by wire
  `params.threadId`. Maintain `child_thread_id → parent` from
  `collabAgentToolCall.receiverThreadIds` and from child `thread/started`
  notifications whose `thread.parentThreadId` = our thread. Child-thread
  `item/*` events translate through the existing item translator, then wrap
  with `subagent_id = child threadId`. Child `thread/started`,
  `turn/started`, `turn/completed` update subagent status, never the parent
  turn state.
- **Launch/lifecycle items on the parent thread**:
  `collabAgentToolCall { tool: spawnAgent|sendInput|resumeAgent|wait|closeAgent,
  status: inProgress|completed|failed, receiverThreadIds, prompt?, model?,
  agentsStates: { <threadId>: { status: pendingInit|running|interrupted|
  completed|errored|shutdown|notFound, message? } } }` — spawn completed ⇒
  Running with model/prompt; `agentsStates[..].message` feeds the activity
  line; wait/close results update per-child status.
  `subAgentActivity { agentThreadId, kind: started|interacted|interrupted }`
  ⇒ cheap status ticks. Suppress raw rendering of both item types (the card
  replaces them).
- **Identity**: child `thread/started` carries
  `thread.{ id, parentThreadId, agentNickname, agentRole }`.
- **Wire-drift caution**: the current v2 schema says `thread/started` =
  `{ thread: {...} }` (nested), `turn/started` / `turn/completed` =
  `{ threadId, turn: { id, status, ... } }` — our existing flat
  `ThreadStartedParams { threadId }` / `TurnStartedParams { threadId,
  turnId }` bindings may fail on live servers. Decode **both shapes**
  (untagged enum or manual fallback) and add a unit test per shape. Do not
  regress the existing session flow.
- Optional richness (follow-up, not v1): `thread/read` for cold child
  transcript fetch, `thread/status/changed`, review-mode child threads
  (`threadSource: subAgentReview`).

### OpenCode (`opencode serve` HTTP + SSE) — `opencode/{protocol,translate,sse,client}.rs`

Keying: `subagent_id = child sessionID`. Verified live against opencode
1.15.13.

- **Stop dropping child events**: maintain a watched-session set = parent ∪
  descendants. On `session.created` with `properties.info.parentID` in the
  set, add the child id. `event_matches_session` passes events for any
  watched id; translation tags non-parent-session events with their
  `subagent_id`.
- **Launch**: parent task tool part (`type: "tool"`, `tool: "task"`) via
  `message.part.updated`. `state.status: pending → running → completed |
  error`. **`pending` has NO metadata** — don't emit the row until the first
  `running` update, which carries `state.metadata.{ parentSessionId,
  sessionId, model }` (camelCase, lowercase `d` — differs from `sessionID`
  elsewhere!) plus `title` and `input.{ description, prompt, subagent_type }`.
  Requires decoding `state.metadata` in `ToolStateValue` and emitting on
  Pending/Running (both currently dropped). Suppress the generic tool card
  for `tool == "task"`.
- **Working**: child-session `message.updated` / `message.part.updated` /
  `message.part.delta` events (child `sessionID`) → `SubagentItem` routing;
  `session.status` `{ type: busy|idle|retry }` → status ticks. Child
  `permission.asked` / `question.asked` arrive with the child sessionID and
  **stall the whole turn if unanswered** — translate to `RequestOpened` with
  `subagent_id` set; replies go to `POST /session/{childID}/permissions/{permID}`.
- **Done**: parent task part flips `status: "completed"` with `output`
  wrapped in `<task id="ses_…" state="completed"><task_result>…</task_result></task>`
  (strip the envelope for `result_text`; `<task_error>` + `state:"error"` ⇒
  Failed) and `time.{start,end}` for duration. Note `session.idle` for the
  child fires *before* the parent part flips.
- **Cold hydrate for drill-in**: `GET /session/{childID}/message?limit=N`
  returns `[{ info, parts }]` in the shapes we already decode;
  `GET /session/{id}/children` lists children. Use as backfill only if the
  live stream missed the start (mid-session attach); primary path is SSE.
- **Also decode** `SessionInfo.parentID` and the `subtask`/`agent` part
  types (render `agent` mentions as inert chips; `subtask` needs no special
  UI since the runtime always materializes a `task` tool part — verified in
  source).

## Frontend

- `src/tauri/events.ts` — mirror `SubagentUpdated`, `SubagentStatus`,
  `subagent_id` fields.
- `src/lib/agent-chat/types.ts` — new `SubagentRunItem` view item:
  `{ kind: "subagent_run", id, subagents: SubagentView[] }` where
  `SubagentView = { id, name, agentType?, model?, status, activity?,
  resultText?, toolUseCount?, totalTokens?, durationMs?, startedAt?,
  items: ChatViewItem[], toneIndex }`. Child `items` are built with the
  **same reducer item-builders** (extract the item-construction helpers so
  both paths share them).
- `src/lib/agent-chat/reducer.ts` — `subagent_updated` merges snapshots
  (non-null fields win) into the turn's open `SubagentRunItem`; events with
  `subagent_id` route into that subagent's `items` instead of flat append;
  elapsed/tool-count fall back to counting child items when the provider
  sends no usage. Cap per-subagent retained items (e.g. 500) the same way
  the main transcript caps at 5,000.
- `src/components/chat/transcript-slots.ts` — `subagent_run` is a standalone
  slot (excluded from tool folding, like diffs).
- `src/components/chat/SubagentsCard.tsx` — the orchestration card per the
  design: header (aggregate spinner/check, "Subagents", "N tasks · running in
  parallel", right-aligned "X done · Y active" mono), rows (status
  spinner/check/x, name + model column ~150px, shimmer activity line while
  running / muted result line when done, mono meta "2m 41s · 28 tools",
  Enter button, chevron), inline peek on row click ("Recent activity" — last
  3 child tool rows as `verb · target · meta` + "Enter subagent" button),
  footer note ("Subagents report back to this thread when finished. Steering
  messages go to the orchestrator."). Reuse `StreamingMarker`'s shimmer
  primitive and DiffView's card surface recipe.
- `src/components/chat/SubagentView.tsx` — drill-in: tone-tinted read-only
  banner (lock icon, "Read-only view of the **X** subagent. To change
  direction, message the orchestrator."), then the sub-transcript rendered
  through the existing renderers (`ToolCallCard`, `ReasoningBlock`,
  Streamdown text), live tail block (spinner + shimmer line) while Running.
- `AgentChatPane.tsx` / `AgentChatPaneHeader.tsx` — `viewMode` state;
  orchestrator mode shows an amber blinking "N subagents running" pill in
  the pane sub-header while any subagent is Running; subagent mode replaces
  the sub-header with the breadcrumb (`← Orchestrator › ⟨glyph⟩ Name`
  `model` chip, right-aligned status). Esc / back returns. Composer stays
  parent-bound; placeholder swaps.
- `tool-visuals.ts` — add `Task`/`Agent` mapping (Bot icon, ember tint) for
  any stray un-suppressed occurrences.
- `src/dev/tauri-mock.ts` — seed one subagent turn in the demo transcript
  (two subagents: one completed, one running, matching the design fixture)
  and extend `streamMockChatReply` / `window.__codemuxChatMock` with a
  `streamSubagents()` scenario for visual dev.

## Implementation stages (Opus xhigh subagents)

### Already Landed (all stages, `subagent-view` branch)

All five stages have landed on this branch. Canonical truth now lives in
`docs/features/agent-chat.md` § "Subagent view (cross-provider)"; the
per-stage notes below are retained as the implementation record.

- **Stage 1 — canonical core + Claude (blocking)** ✅ LANDED: `events.rs`
  (`SubagentUpdated` + `SubagentSnapshot` + `SubagentStatus`, `subagent_id`
  tag on `ContentDelta`/`ItemCompleted`/`RequestOpened`, all
  `#[serde(default)]`), `claude/translate.rs` (`parent_tool_use_id` keying,
  `task_*` system-event progress, `task_id → tool_use_id` map,
  `async_launched` deferral), the `agentProgressSummaries: true` sidecar
  option, and `agent_chat.rs::should_persist_event`. Unit + adapter tests.
- **Stage 2 (parallel after Stage 1)**:
  - 2a Codex adapter ✅ LANDED: per-session `threadId` demux, `collabAgentToolCall` /
    `subAgentActivity` mapping, flat+nested wire-shape tolerance, parent
    turn/session isolation. +25 lib tests, +1 adapter scenario.
  - 2b OpenCode adapter ✅ LANDED: `SseRouter` watched-session set (transitive
    via `SessionInfo.parentID`), `state.metadata`/`time` task-tool decode,
    `<task_result>`/`<task_error>` envelope stripping, child-session
    permission reply routing, cold-hydrate helpers. +32 tests.
    (`question.asked` left as a flagged decode-as-`Other` gap.)
  - 2c Frontend ✅ LANDED: `src/tauri/events.ts` mirror, `SubagentView` /
    `SubagentRunItem` types, reducer snapshot-merge + `subagent_id` routing,
    `subagents.ts` helpers, `SubagentsCard` / `SubagentView` /
    `SubagentBreadcrumb`, pane `viewMode`, header pill, `.cm-blink`,
    dev-mock `streamSubagents()`. Vitest/jsdom tests.
- **Stage 3 — integration** ✅ LANDED: `npm run verify` + `cargo test` green
  (one pre-existing flaky interrupt timing test, `claude_adapter.rs`,
  unrelated to subagents — passes in isolation and normal runs); browser-pane
  visual QA of the card, inline peek, and drill-in against
  `Subagents.dc.html` plus a live `streamSubagents()` run; docs updated
  (`docs/features/agent-chat.md`, `docs/core/STATUS.md`, this plan).

## Open Questions

- Codex live wire shapes: confirm whether flat legacy or nested v2 params
  arrive from `codex app-server` without v2 negotiation (decode both).
- Claude cold-resume of a subagent transcript (`getSubagentMessages`) — not
  in v1; persistence via our own DB covers restart for sessions run in
  Codemux.
- Recursive drill-in for nested subagents — v1 flattens grandchildren into
  the child's stream.

## Reference material (left on disk by research agents)

- `/tmp/sdkpack/package/sdk.d.ts` — Claude Agent SDK 0.2.114 types.
- `/tmp/codex-schema/` — generated Codex app-server v2 JSON schema (CLI 0.142.5).
- `/tmp/opencode-openapi.json`, `/tmp/opencode-src/`, `/tmp/sse-capture.jsonl`
  — OpenCode OpenAPI, source, and a live captured subagent lifecycle.
- `/tmp/t3code/`, `/tmp/synara/` — reference project sources (patterns only;
  never copy code).
- `docs/plans/assets/Subagents.dc.html` — the visual spec.

## Notes

- `TaskSummaryCard` renders TodoWrite checklists — unrelated; do not touch.
- The bounded event channel means the UI must treat live events as
  live-only; hydrate covers history (unchanged invariant).
