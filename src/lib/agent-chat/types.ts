import type {
  ApprovalDecision,
  ContextUsageSnapshot,
  ProviderRuntimeEvent,
  SubagentStatus,
  SubagentTaskKind,
  TasksSnapshot,
  TurnStatus,
} from "@/tauri/events";

export type { SubagentStatus, SubagentTaskKind };

/**
 * View-only subagent status. `interrupted` never appears on the wire —
 * it is derived in the frontend when a run is force-settled (a new turn,
 * a session close/error, or a hydrate that ends mid-run) so a persisted
 * transcript never resurrects a perpetual "running" spinner. A later real
 * `running` snapshot revives an interrupted row (see `mergeSnapshot`).
 */
export type SubagentViewStatus = SubagentStatus | "interrupted";

export type ChatItemId = string;

/**
 * One image attached to a user turn, in the shape the right-aligned
 * bubble renders as a thumbnail (and opens in a lightbox).
 *
 * `src` is one of two forms, distinguished only by how the item was
 * created — the renderer never has to know which:
 *   - A `data:` URL, built optimistically at send time from the staged
 *     image bytes (`resolvedImage.bytes` + `.mime`). At that point the
 *     image lives only in memory, so a self-contained data URL is the
 *     only thing that can render before any round-trip.
 *   - An absolute filesystem path, read back at hydrate from the
 *     persisted `user_message` envelope's `images[].path` (the backend
 *     wrote the bytes to disk when the turn was sent).
 *
 * The renderer routes `src` through `resolveAssetSrc` (src/lib/
 * asset-url.ts): data URLs pass through untouched, absolute paths go
 * through Tauri's asset protocol (`convertFileSrc`). That keeps the
 * bubble oblivious to the fresh-vs-hydrated distinction.
 */
export interface UserMessageImage {
  src: string;
  /** MIME type (`image/png`, …) when known. Purely informational
   *  today (alt text / future download); the `<img>` src carries its
   *  own type for data URLs. */
  mediaType?: string;
}

/**
 * Every ChatViewItem carries a monotonic `seq`. The render layer sorts
 * by `seq` so message order is a property of the data, not of React
 * reconciliation or store-update timing. Mirrors a reference
 * multi-provider client where each activity has a `sequence: number`
 * (apps/web/src/store.ts:838).
 *
 * Assigned at insert time from the thread's `nextSeq` counter. Mutating
 * an existing item (appending delta text, attaching tool result,
 * resolving a permission request) MUST preserve the original `seq`.
 */
export interface UserMessageItem {
  kind: "user_message";
  id: ChatItemId;
  seq: number;
  text: string;
  /** Wall-clock boundary for turn-duration presentation. Optional for
   *  transcripts persisted before timed replay rows were introduced. */
  created_at?: number;
  /** Follow-up queueing: present while this message is parked behind an
   *  active turn (rendered greyed-out with a "Queued" pill + cancel X).
   *  Removed when the turn dispatches (`queued_turn_dispatched`) and the
   *  bubble promotes to a normal user message. */
  queued?: { queuedId: string };
  /** Optimistic-send correlation token. Set when the bubble was
   *  optimistically appended by the composer; lets the `turn_queued`
   *  event reconcile against this exact item instead of appending a
   *  duplicate. Absent for hydrated / backend-reconstructed items. */
  clientNonce?: string;
  /** Images attached to this turn (paste / drop / picker). Optimistic
   *  appends carry `data:` URLs built from the staged bytes at send
   *  time; hydrated items carry absolute filesystem paths mapped from
   *  the persisted envelope's `images[].path`. Absent for text-only
   *  turns and for messages persisted before this field existed. */
  images?: UserMessageImage[];
  /** Durable `agent_chat_messages.id` when this item came from (or was
   * reconciled with) a persisted row. Used only for conversation-search
   * deep links; absent on optimistic bubbles until the backend fans the row
   * back with its id. */
  source_event_id?: number;
}

export interface AssistantMessageItem {
  kind: "assistant_message";
  id: ChatItemId;
  seq: number;
  turn_id: string | null;
  text: string;
  streaming: boolean;
  /** First-observed wall-clock time for this assistant block. */
  created_at?: number;
  /** Durable source row for conversation-search deep links. */
  source_event_id?: number;
}

/**
 * A collapsible "thinking" block. Thinking `content_delta`s accumulate
 * into the trailing reasoning item using the same tail-merge discipline as
 * assistant text: while it is the streaming tail, further thinking deltas
 * append to `text`; it seals (`streaming: false`) when an
 * `assistant_thinking` completion finalises it, or when any non-thinking
 * item lands after it. Renders via the D5 `ReasoningBlock`
 * ("Thinking…" while streaming, "Thought for Ns" once sealed).
 */
export interface ReasoningItem {
  kind: "reasoning";
  id: ChatItemId;
  seq: number;
  turn_id: string | null;
  text: string;
  streaming: boolean;
  /** Wall-clock ms (from the injected clock, default `Date.now`) when the
   *  first thinking delta of this block landed. Absent when the block was
   *  materialised straight from an `assistant_thinking` completion that
   *  carried no deltas, or when a hydrate/replay never observed a first
   *  delta. Drives `duration_ms`. */
  started_at?: number;
  /** Thinking duration in ms (first-delta → seal). Set when the block
   *  seals — via the `assistant_thinking` completion or a trailing
   *  non-thinking item. Absent until sealed, and when `started_at` was
   *  never captured (nothing to measure from). */
  duration_ms?: number;
}

export interface ToolCallItem {
  kind: "tool_call";
  id: ChatItemId;
  seq: number;
  /** Provider turn that owns this call. Older hydrated rows may omit it. */
  turn_id?: string | null;
  tool_use_id: string;
  tool_name: string;
  input: unknown;
  status: "running" | "done" | "error";
  result_content: unknown | null;
  /** When a permission request is tied to this tool call via
   *  `tool_use_id`, the reducer stores the request's id here so the
   *  renderer can look up the pending `PermissionRequestItem` and
   *  show an inline approval footer on the tool-call card. `null` in
   *  bypassPermissions mode or before the approval event lands. */
  approval_request_id: string | null;
  /** Wall-clock ms (from the injected clock, default `Date.now`) when the
   *  `tool_use` landed and the call went `running`. Optional so persisted /
   *  hydrated transcripts written before this field existed still parse.
   *  Feeds the Activity block's rolled-up duration. */
  started_at?: number;
  /** Wall-clock ms when the `tool_result` landed and the call settled
   *  (`done` / `error`). Optional for the same backward-compat reason.
   *  With `started_at`, bounds this call's contribution to run duration. */
  completed_at?: number;
}

export interface PermissionRequestItem {
  kind: "permission_request";
  id: ChatItemId;
  seq: number;
  request_id: string;
  turn_id: string | null;
  request_kind: string;
  payload: unknown;
  /** Provider tool_use_id — when present, the reducer links this
   *  request to the matching `ToolCallItem.approval_request_id`.
   *  `null` for standalone requests (plan, unmatched, Codex
   *  server-initiated). */
  tool_use_id: string | null;
  /** When a subagent raised this approval, its demux key. Lets the
   *  parent flow label it "from subagent X" and the drill-in mirror
   *  it into the sub-transcript. `null` for ordinary parent
   *  requests. */
  subagent_id?: string | null;
  resolution:
    | { state: "pending" }
    | { state: "responding"; decision: ApprovalDecision }
    | { state: "resolved"; decision: ApprovalDecision }
    | {
        state: "failed";
        reason: "stale_provider_callback";
        message: string;
      };
}

export interface TurnEndedItem {
  kind: "turn_ended";
  id: ChatItemId;
  seq: number;
  turn_id: string;
  status: TurnStatus;
  /** Wall-clock completion boundary used by the settled-turn disclosure. */
  completed_at?: number;
  /** True when the provider's main loop merely *yielded* here — it
   *  reported a successful completion while delegated work was still
   *  running, or parent output resumed afterwards without a new prompt
   *  (Claude Code re-invokes the model on background task notifications
   *  inside the same session). An interim boundary is not the turn's end:
   *  the transcript keeps the turn live across it and folds only at the
   *  final, non-interim marker. */
  interim?: boolean;
}

/**
 * One subagent's live state + its own sub-transcript. The reducer merges
 * `subagent_updated` snapshots into these (non-null fields win) and routes
 * `subagent_id`-tagged content/item events into `items` using the same
 * item-construction helpers as the main transcript. Camel-cased view
 * shape — distinct from the snake_case wire `SubagentSnapshot`.
 */
export interface SubagentView {
  /** Demux key (the wire `subagent_id`). Stable React key + lookup id. */
  id: string;
  name?: string;
  agentType?: string;
  /** What the subagent was asked to do, in the spawner's own short words
   *  (`SubagentSnapshot.description`). Titles the pane's spawn waves. */
  description?: string;
  /** `"monitor"` marks a background watch loop rather than delegated agent
   *  work. Such a row still gets its transcript card (the user should be able
   *  to see what is being watched), but it is left out of the docked
   *  "N subagents running" bar — a watch loop is not a subagent doing work,
   *  and counting it there is what used to pin a thread at "Working". */
  taskKind?: SubagentTaskKind;
  model?: string;
  status: SubagentViewStatus;
  /** The spawning tool_use / call id (`SubagentSnapshot.parent_item_id`)
   *  for all three providers. Lets a parent-scoped `tool_result` settle
   *  this row when the adapter's demux lost track and let the raw
   *  spawning `tool_result` through (the issue-#153 stuck-thread shape). */
  parentItemId?: string;
  /** True when `status` was settled by INFERENCE (a parent-scoped
   *  `tool_result` derivation, a forced settle on close/new-turn, or a
   *  hydrate reconciliation) rather than a real terminal snapshot. A real
   *  terminal snapshot clears it; a real `running` snapshot revives an
   *  assumed/interrupted row back to `running` and clears it. */
  statusAssumed?: boolean;
  /** True when the provider reported this row as a *background task*
   *  (e.g. a background shell command) rather than a delegated subagent.
   *  Such a job can outlive the turn indefinitely and never report a
   *  terminal status, so it must not read as live activity once the
   *  thread stops streaming (see `runningSubagentEntries`). */
  backgroundTask?: boolean;
  /** Provider-pushed "currently doing X" line, when supplied. */
  activity?: string;
  /** Final report first surfaced on completion. */
  resultText?: string;
  toolUseCount?: number;
  totalTokens?: number;
  durationMs?: number;
  /** Wall-clock ms (injected clock) when the row was first observed —
   *  drives the elapsed-time fallback when the provider sends no
   *  `durationMs`. */
  startedAt?: number;
  /** The subagent's own transcript, built with the shared reducer
   *  item-builders so every existing renderer works in the drill-in. */
  items: ChatViewItem[];
  /** Deterministic accent index (hash of `id`) for the design's tone
   *  cycle; the visible status colour still derives from `status`. */
  toneIndex: number;
}

/**
 * One orchestration card per contiguous spawn group per turn (locked
 * decision 5). Holds every subagent spawned back-to-back; a new turn or
 * an interrupting parent item starts a fresh card.
 */
export interface SubagentRunItem {
  kind: "subagent_run";
  id: ChatItemId;
  seq: number;
  turn_id: string | null;
  subagents: SubagentView[];
}

/**
 * Lifecycle status of a `Workflow` tool run, as merged into the frontend's
 * per-workflow view state. Mirrors the wire `WorkflowSnapshot.status`
 * string (not a Rust enum on the wire, so this is the view-side
 * narrowing of it).
 */
export type WorkflowRunStatus =
  | "pending_approval"
  | "running"
  | "completed"
  | "failed"
  | "stopped";

/**
 * One planned phase of a workflow run, carrying the subagents the
 * reducer has routed into it (via `SubagentSnapshot.phase`). Status and
 * rollups (tokens, elapsed, counts) are derived on read by
 * `workflows.ts` helpers rather than stored, so there is a single source
 * of truth (the `agents` list) for them.
 */
export interface WorkflowPhaseView {
  title: string;
  detail: string | null;
  agents: SubagentView[];
}

/**
 * One `Workflow` tool run's live state + its phases. The reducer merges
 * `workflow_updated` snapshots into these (non-null fields win, status
 * stays monotonic once terminal) and routes `workflow_id`-tagged
 * `subagent_updated` snapshots into the matching phase's `agents` list
 * instead of the generic `subagent_run` card.
 */
export interface WorkflowRunItem {
  kind: "workflow_run";
  id: ChatItemId;
  seq: number;
  /** Best-effort provider turn attribution for settled-turn folding. */
  turn_id?: string | null;
  /** Demux key — the wire `workflow_id` (the `Workflow` tool call's
   *  `tool_use_id`). */
  workflowId: string;
  status: WorkflowRunStatus;
  name: string | null;
  description: string | null;
  /** Raw script text, for a "view source" affordance. */
  script: string | null;
  /** Phases as originally planned by the script's `meta.phases`, before
   *  any subagents have been attributed to them. `workflowPhaseStats`
   *  callers should prefer `phases` (which carries the live agents);
   *  this is kept so a phase with zero subagents-so-far still renders
   *  its title/detail. */
  plannedPhases: { title: string; detail: string | null }[];
  /** Live phases — one per planned phase (by title), each carrying the
   *  subagents the reducer has routed into it. A subagent whose `phase`
   *  hint doesn't match any planned title lands in a synthesized
   *  trailing "Run" phase. */
  phases: WorkflowPhaseView[];
  resultText: string | null;
  totalTokens: number | null;
  agentCount: number | null;
  /** Wall-clock ms (injected clock) when the launch was first observed. */
  startedAt: number;
  durationMs: number | null;
  /** When a permission request gates this workflow's launch (e.g. an
   *  approval-required workflow), the request's id. `null` once
   *  resolved or when no gate applies. */
  approvalRequestId: string | null;
}

/**
 * A compact inline notice surfaced from a `runtime_warning` the reducer
 * decides is user-facing (a provider rate-limit rejection, an enumerated
 * SDK assistant error). Most `runtime_warning`s stay console-only debug
 * noise; the classifier (`runtime-notice.ts`) decides which ones become
 * one of these rows. Rendered as a muted-amber left-bordered line.
 */
export interface RuntimeNoticeItem {
  kind: "runtime_notice";
  id: ChatItemId;
  seq: number;
  message: string;
  /** Visual temperature. `"error"` (red) marks a terminal failure —
   *  the session died and the message is the cause; the default
   *  (absent / `"warning"`, amber) covers advisories the run may
   *  recover from (rate-limit rejection, resume fallback). */
  severity?: "warning" | "error";
}

export type ChatViewItem =
  | UserMessageItem
  | AssistantMessageItem
  | ReasoningItem
  | ToolCallItem
  | PermissionRequestItem
  | TurnEndedItem
  | SubagentRunItem
  | WorkflowRunItem
  | RuntimeNoticeItem;

/**
 * A live provider event plus the durable row id it was persisted as
 * (`null` for kinds the backend never persists — `content_delta`,
 * lifecycle notices).
 *
 * The id is what makes cursor resume safe: history replayed from a tail
 * read and the live stream overlap by design (the pane buffers live
 * events across a hydrate rather than risk a gap), and an event at or
 * below the thread's `lastPersistedEventId` is one the tail already
 * applied.
 */
export interface LiveChatEvent {
  event: ProviderRuntimeEvent;
  persistedId: number | null;
}

export interface ChatThreadState {
  messages: ChatViewItem[];
  /** Latest complete provider-authored task plan for this thread. */
  tasks: TasksSnapshot | null;
  /** Clock time (ms) the latest task snapshot was applied. Hydration
   *  replays through the same reducer path, so after a reopen this is
   *  "when this session learned about the plan", not the original wall
   *  time — good enough for the panel's "last update" caption. */
  tasksUpdatedAt: number | null;
  streaming: boolean;
  pendingRequestIds: string[];
  /** Next `seq` to assign to a freshly-appended item. Strictly
   *  increasing; never reset across a silent session restart so the
   *  migrated transcript stays ordered relative to new items. */
  nextSeq: number;
  /** Non-null while the stall watchdog has flagged this thread as
   *  silently mid-turn (issue #154). Set by the transient `run_stalled`
   *  event and cleared by the next real activity; never persisted, so a
   *  hydrated thread always starts `null`. Drives the amber "no activity"
   *  transcript notice. */
  stalled: { silentForSecs: number } | null;
  /** True when the thread's last turn never settled — the run died
   *  without a terminal event (child exit, laptop sleep, usage cutoff).
   *  Computed at hydrate (last persisted user turn has no later
   *  `turn_completed`) and set live on a `child_exited` turn error.
   *  Drives the "Run interrupted" divider and the composer's Continue
   *  chip. */
  interrupted: boolean;
  /** Whether the thread's history ends on an unsettled user turn: a
   *  `user_message` with no later `turn_completed`.
   *
   *  Maintained live by the reducer and recomputed by every hydrate, so a
   *  warm cursor-tail merge can seed `lastTurnUnsettled` with the answer
   *  for the prefix it is NOT re-scanning and get the same result as
   *  scanning the whole concatenated history.
   *
   *  Deliberately NOT derived from `streaming`. Those two came apart when
   *  the interim-turn hold landed: a provider that yields to wait on a
   *  delegated agent emits a real `turn_completed` (so the last turn IS
   *  settled) while the thread stays `streaming` because the run is still
   *  alive. Using `streaming` as the proxy made every remount during a
   *  delegated phase report a false "Run interrupted". */
  turnUnsettled: boolean;
  /** Latest context-window occupancy reported by the provider, or
   *  `null` before the first usage report lands (which is also the
   *  signal to hide the composer's meter entirely). Latest snapshot
   *  wins; the lifetime `total_processed_tokens` is merged forward so
   *  it never regresses when a later snapshot omits it. Persisted
   *  backend-side, so hydrate-replay restores it after a restart. */
  contextUsage: ContextUsageSnapshot | null;
}

export function emptyThreadState(): ChatThreadState {
  return {
    messages: [],
    tasks: null,
    tasksUpdatedAt: null,
    streaming: false,
    pendingRequestIds: [],
    nextSeq: 0,
    stalled: null,
    interrupted: false,
    turnUnsettled: false,
    contextUsage: null,
  };
}

export type AnyProviderEvent = ProviderRuntimeEvent;
