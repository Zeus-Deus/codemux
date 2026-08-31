import { emit, listen, type UnlistenFn } from "@tauri-apps/api/event";

export type { UnlistenFn };
import type {
  AgentChatProviderKind,
  AppStateSnapshot,
  AuthStatePayload,
  PresetStoreSnapshot,
  RevisionedDelta,
  RevisionHeartbeat,
  TerminalStatusPayload,
  ThemeColors,
  UserSettings,
} from "./types";
import type { AutomationRunView } from "./commands";

export type EventCallback<T> = (payload: T) => void;

export const onAppStateChanged = (cb: EventCallback<AppStateSnapshot>): Promise<UnlistenFn> =>
  listen<AppStateSnapshot>("app-state-changed", (e) => cb(e.payload));

/** One domain-scoped change to the app state. Ordered against
 *  `app-state-changed` by a shared revision counter — see `RevisionedDelta`. */
export const onAppStateDelta = (cb: EventCallback<RevisionedDelta>): Promise<UnlistenFn> =>
  listen<RevisionedDelta>("app-state-delta", (e) => cb(e.payload));

/** Periodic "the backend is at revision N" ping with no payload to parse.
 *  Lets a renderer that missed an emit notice and resync. */
export const onAppStateRevision = (cb: EventCallback<RevisionHeartbeat>): Promise<UnlistenFn> =>
  listen<RevisionHeartbeat>("app-state-revision", (e) => cb(e.payload));

export interface ActiveWorkspacePersistFailure {
  generation: number;
  error: string;
}

/** The selection changed in memory but its ordered SQLite commit failed.
 *  This is intentionally a separate signal from app state: retry remains
 *  automatic on a later emit, while the current renderer can make the loss
 *  of restart durability visible instead of silently discarding it. */
export const onActiveWorkspacePersistFailed = (
  cb: EventCallback<ActiveWorkspacePersistFailure>,
): Promise<UnlistenFn> =>
  listen<ActiveWorkspacePersistFailure>(
    "active-workspace-persist-failed",
    (e) => cb(e.payload),
  );

export const onPresetsChanged = (cb: EventCallback<PresetStoreSnapshot>): Promise<UnlistenFn> =>
  listen<PresetStoreSnapshot>("presets-changed", (e) => cb(e.payload));

export const onThemeChanged = (cb: EventCallback<ThemeColors>): Promise<UnlistenFn> =>
  listen<ThemeColors>("theme-changed", (e) => cb(e.payload));

export const onTerminalStatus = (cb: EventCallback<TerminalStatusPayload>): Promise<UnlistenFn> =>
  listen<TerminalStatusPayload>("terminal-status", (e) => cb(e.payload));

export const onSerializeTerminalBuffers = (cb: EventCallback<null>): Promise<UnlistenFn> =>
  listen<null>("serialize-terminal-buffers", () => cb(null));

/** Fired by the automation scheduler each time a due automation
 *  produces a run. The payload is the freshly-created run row. */
export const onAutomationFire = (cb: EventCallback<AutomationRunView>): Promise<UnlistenFn> =>
  listen<AutomationRunView>("automations://fire", (e) => cb(e.payload));

export const emitScrollbackSerializationComplete = () =>
  emit("scrollback-serialization-complete");

export interface WorkspaceSetupProgress {
  workspace_id: string;
  command: string;
  index: number;
  total: number;
}

export const onWorkspaceSetupProgress = (
  cb: EventCallback<WorkspaceSetupProgress>,
): Promise<UnlistenFn> =>
  listen<WorkspaceSetupProgress>("workspace-setup-progress", (e) => cb(e.payload));

export interface WorkspaceSetupComplete {
  workspace_id: string;
}

export const onWorkspaceSetupComplete = (
  cb: EventCallback<WorkspaceSetupComplete>,
): Promise<UnlistenFn> =>
  listen<WorkspaceSetupComplete>("workspace-setup-complete", (e) => cb(e.payload));

export interface WorkspaceSetupFailed {
  workspace_id: string;
  command: string;
  stdout: string;
  stderr: string;
  exit_code: number | null;
}

export const onWorkspaceSetupFailed = (
  cb: EventCallback<WorkspaceSetupFailed>,
): Promise<UnlistenFn> =>
  listen<WorkspaceSetupFailed>("workspace-setup-failed", (e) => cb(e.payload));

export interface WorktreeIncludesApplied {
  workspace_id: string;
  source: "file" | "setting" | "defaults";
  copied: string[];
}

export const onWorktreeIncludesApplied = (
  cb: EventCallback<WorktreeIncludesApplied>,
): Promise<UnlistenFn> =>
  listen<WorktreeIncludesApplied>("worktree-includes-applied", (e) => cb(e.payload));

// ── git clone progress ──
//
// Mirror of src-tauri/src/commands/git.rs:GitCloneProgress. Emitted on
// each parsed `git clone --progress` update (throttled to phase/percent
// changes). Filter on `targetDir` to match your own in-flight clone.
export interface GitCloneProgress {
  targetDir: string;
  phase: string;
  percent: number | null;
  detail: string;
}

export const GIT_CLONE_PROGRESS_EVENT = "git-clone-progress";

export const onGitCloneProgress = (
  cb: EventCallback<GitCloneProgress>,
): Promise<UnlistenFn> =>
  listen<GitCloneProgress>(GIT_CLONE_PROGRESS_EVENT, (e) => cb(e.payload));

export const onSettingsSynced = (
  cb: EventCallback<UserSettings>,
): Promise<UnlistenFn> =>
  listen<UserSettings>("settings-synced", (e) => cb(e.payload));

export const onAuthStateChanged = (
  cb: EventCallback<AuthStatePayload>,
): Promise<UnlistenFn> =>
  listen<AuthStatePayload>("auth-state-changed", (e) => cb(e.payload));

// Stage 2 sync state changes: emitted by the backend after signin,
// signout, sync setup, and sync repair.
export interface SyncStateChangedPayload {
  syncAvailable: boolean;
  authMethod: "email" | "github" | null;
}

export const onSyncStateChanged = (
  cb: EventCallback<SyncStateChangedPayload>,
): Promise<UnlistenFn> =>
  listen<SyncStateChangedPayload>("sync-state-changed", (e) => cb(e.payload));

// ── Agent chat events ──
//
// Mirror of src-tauri/src/commands/agent_chat.rs:AgentChatEventPayload
// and src-tauri/src/agent_provider/events.rs:ProviderRuntimeEvent.
// Thread-scoped events stream over a per-thread `Channel` registered
// via `attachAgentChatOutput` (see `useAgentChatEvents`), NOT the
// global event bus — the bus only carries the rare thread-less
// `runtime_warning` on the `agent_chat_event` name. The payload shape
// is identical on both transports.

export type ApprovalDecision =
  | {
      decision: "allow";
      updated_input?: unknown;
      /** Opaque SDK-shaped `PermissionUpdate[]` — populated when the
       *  user picks an "always allow" scope. Stage 1 never sends it,
       *  but the field is on the wire now so Stage 5 can fill it in
       *  without another backend change. */
      updated_permissions?: unknown[];
    }
  | { decision: "allow_for_session" }
  | { decision: "deny"; message: string }
  | { decision: "cancel" };

export type SessionStatus =
  | { status: "starting" }
  | { status: "ready" }
  | { status: "running"; active_turn: string }
  | { status: "waiting_approval"; request_id: string }
  | { status: "error"; message: string }
  | { status: "closed" };

export type ContentDelta =
  | { kind: "text"; text: string }
  | { kind: "thinking"; text: string }
  | { kind: "tool_input"; tool_name: string; partial_json: string };

export type CompletedItem =
  | { kind: "assistant_text"; text: string }
  | { kind: "assistant_thinking"; text: string }
  | { kind: "tool_use"; tool_name: string; input: unknown; tool_use_id: string }
  | {
      kind: "tool_result";
      tool_use_id: string;
      content: unknown;
      is_error: boolean;
    };

// ── Subagents (cross-provider) ──
//
// Mirror of src-tauri/src/agent_provider/events.rs:SubagentSnapshot /
// SubagentStatus (Stage 1). Field names are the SERIALIZED snake_case
// wire names. Every field except `subagent_id` / `status` is optional
// and `#[serde(default)]` on the Rust side, so old persisted rows (and
// providers that only dribble identity out over several events) decode
// cleanly. The frontend reducer merges non-null fields into its
// per-subagent view state.

export type SubagentStatus =
  | "pending"
  | "running"
  | "completed"
  | "failed"
  | "stopped";

/** Mirror of `events.rs::SubagentTaskKind`. `"monitor"` is a background watch
 *  loop (a CI poll, a tailed process) rather than delegated agent work — it is
 *  what lets a thread settle to the calm `monitoring` pane status, and it is
 *  excluded from the "N subagents running" roster. */
export type SubagentTaskKind = "agent" | "monitor";

export interface SubagentSnapshot {
  /** Stable demux key (Claude parent_tool_use_id, Codex child threadId,
   *  OpenCode child sessionID). Required. */
  subagent_id: string;
  /** tool_use / call id of the block that spawned this subagent. */
  parent_item_id?: string | null;
  /** Display name — "Explore", nickname, or agent name. */
  name?: string | null;
  /** subagent_type / role / agent. */
  agent_type?: string | null;
  /** Short task label from the spawning call (`Agent.description`,
   *  `task_started.description`): what the subagent was asked to do. */
  description?: string | null;
  /** Whether this is real delegated work or a background watch loop, when
   *  the provider reports it. `null`/absent means "not reported", which
   *  every consumer treats as `"agent"`. */
  task_kind?: SubagentTaskKind | null;
  model?: string | null;
  status: SubagentStatus;
  /** Live "currently doing X" line pushed by the provider. */
  activity?: string | null;
  /** Final report text, on completion. */
  result_text?: string | null;
  tool_use_count?: number | null;
  total_tokens?: number | null;
  duration_ms?: number | null;
  /** Provider-native id (codex threadId, opencode sessionID, claude agentId). */
  provider_ref?: string | null;
  /** When this subagent was spawned while a `Workflow` tool run was
   *  active, that workflow's `workflow_id`. Lets the reducer route the
   *  snapshot into the workflow's phase view instead of the generic
   *  subagent-run card. `null` outside a workflow run. */
  workflow_id?: string | null;
  /** Best-effort phase label parsed from a `phase:X` hint in the
   *  provider's task label. `null` when no hint was found — the reducer
   *  falls back to the workflow's last planned phase title. */
  phase?: string | null;
  /** True when this row is a provider *background task* (e.g. a
   *  background shell command) rather than a real delegated subagent. It
   *  can legitimately outlive the turn and never emit a terminal update,
   *  so it never counts as live activity once the thread stops streaming.
   *  Absent / false on every provider that has no such concept. */
  background_task?: boolean | null;
}

// ── Workflows (Claude dynamic-workflow orchestration) ──
//
// Mirror of src-tauri/src/agent_provider/events.rs:WorkflowSnapshot /
// WorkflowPhaseSnapshot. Same additive-serde discipline as
// SubagentSnapshot: every field but `workflow_id` is optional and
// `#[serde(default)]` on the Rust side.

export type WorkflowRunWireStatus =
  | "pending_approval"
  | "running"
  | "completed"
  | "failed"
  | "stopped";

export interface WorkflowPhaseSnapshot {
  title: string;
  detail?: string | null;
}

export interface WorkflowSnapshot {
  /** Stable key — the `tool_use_id` of the `Workflow` tool call. */
  workflow_id: string;
  status: string;
  /** Workflow name — `meta.name`, falling back to a saved workflow's
   *  `input.name`. */
  name?: string | null;
  description?: string | null;
  /** Raw script text (`input.script`), for a "view source" affordance. */
  script?: string | null;
  /** Planned phases parsed from `meta.phases`, in script order. */
  phases?: WorkflowPhaseSnapshot[] | null;
  /** Final result text on completion/failure, truncated server-side. */
  result_text?: string | null;
  total_tokens?: number | null;
  agent_count?: number | null;
  duration_ms?: number | null;
}

// Provider-neutral agent task plan. Each provider adapter emits complete
// replacement snapshots so replay and live updates follow the same path.
export type TaskStatus = "pending" | "in_progress" | "completed";

export interface TaskSnapshotItem {
  task_id: string;
  title: string;
  status: TaskStatus;
  detail?: string | null;
  blocked_by: string[];
}

export interface TasksSnapshot {
  explanation?: string | null;
  tasks: TaskSnapshotItem[];
}

export type TurnStatus =
  | { kind: "success" }
  | { kind: "error"; subtype: string; message: string }
  | { kind: "max_turns" }
  | { kind: "max_budget" };

export interface TurnUsage {
  total_cost_usd: number | null;
  duration_ms: number;
  num_turns: number;
}

/** A point-in-time reading of a thread's context-window occupancy.
 *  Mirrors the Rust `ContextUsageSnapshot`.
 *
 *  `used_tokens` is the live occupancy of the model's context window
 *  (bounded — the number the meter visualizes); `total_processed_tokens`
 *  is the monotonic lifetime sum for the thread including everything
 *  compaction discarded (unbounded, text-only). After a compaction the
 *  former drops while the latter keeps climbing. Optional fields are
 *  `#[serde(default)]` on the Rust side — absent means unknown, and the
 *  UI degrades (no bar/percent without `max_tokens`) rather than
 *  guessing a denominator. */
export interface ContextUsageSnapshot {
  used_tokens: number;
  /** Omitted unless strictly greater than `used_tokens`. */
  total_processed_tokens?: number | null;
  max_tokens?: number | null;
  /** Pre-compaction high-water mark, set on compaction-boundary
   *  snapshots. */
  last_used_tokens?: number | null;
  compacts_automatically?: boolean | null;
}

export type ProviderRuntimeEvent =
  | {
      type: "session_configured";
      thread_id: string;
      provider_session_id: string;
    }
  | {
      type: "content_delta";
      thread_id: string;
      turn_id: string;
      delta: ContentDelta;
      /** When set, this delta belongs to the sub-transcript of the
       *  identified subagent (Claude `parent_tool_use_id`, Codex child
       *  threadId). `#[serde(default)]` on the Rust side, so absent /
       *  `null` for ordinary parent-thread streaming. */
      subagent_id?: string | null;
    }
  | {
      type: "item_completed";
      thread_id: string;
      turn_id: string;
      item: CompletedItem;
      /** When set, this completed item belongs to the identified
       *  subagent's sub-transcript rather than the parent flow. */
      subagent_id?: string | null;
    }
  | {
      type: "subagent_updated";
      thread_id: string;
      subagent: SubagentSnapshot;
    }
  | {
      type: "workflow_updated";
      thread_id: string;
      workflow: WorkflowSnapshot;
    }
  | {
      /** The thread's context-window occupancy changed — emitted on
       *  assistant-message usage reports, turn completion, and
       *  compaction boundaries. Latest snapshot wins per thread.
       *  Persisted, so hydrate-replay restores the meter after a
       *  restart. */
      type: "context_usage_updated";
      thread_id: string;
      usage: ContextUsageSnapshot;
    }
  | {
      /** Legacy adapter-side usage delta. Distinct from
       *  `context_usage_updated`: a delta rather than a snapshot.
       *
       *  Provider history is the accounting source, so the backend discards
       *  this in `forward_event`; it never reaches persistence or a pane. It is
       *  declared here only so the union stays a faithful mirror of the
       *  Rust enum, and so the reducer can ignore it explicitly rather
       *  than logging it as an unknown variant. */
      type: "usage_recorded";
      thread_id: string;
      provider: AgentChatProviderKind;
      model: string | null;
      subagent: boolean;
      input_tokens: number;
      output_tokens: number;
      cache_read_tokens: number;
      cache_write_tokens: number;
      cost_usd: number | null;
    }
  | {
      type: "tasks_updated";
      thread_id: string;
      tasks: TasksSnapshot;
    }
  | {
      type: "turn_completed";
      thread_id: string;
      turn_id: string;
      status: TurnStatus;
      usage: TurnUsage | null;
    }
  | {
      type: "request_opened";
      thread_id: string;
      turn_id: string;
      request_id: string;
      request_kind: string;
      payload: unknown;
      /** Provider tool_use_id when this request maps to an in-flight
       *  tool invocation. Lets the reducer merge the approval into
       *  its originating tool_call row. `null` for standalone
       *  requests (plan, Codex server-initiated, or when the provider
       *  didn't supply one). */
      tool_use_id: string | null;
      /** When a subagent raised this approval, its demux key — lets the
       *  UI label the request "from subagent X" in the parent flow and
       *  mirror it into the drill-in. `#[serde(default)]`. */
      subagent_id?: string | null;
    }
  | {
      type: "request_resolved";
      thread_id: string;
      request_id: string;
      decision: ApprovalDecision;
    }
  | {
      type: "request_response_failed";
      thread_id: string;
      request_id: string;
      reason: "stale_provider_callback";
      message: string;
    }
  | {
      type: "session_state_changed";
      thread_id: string;
      status: SessionStatus;
    }
  | {
      type: "runtime_warning";
      thread_id: string | null;
      message: string;
      original_payload: unknown | null;
    }
  | {
      type: "resume_cursor_updated";
      thread_id: string;
      resume_cursor: unknown;
    }
  // A user turn that was written to `agent_chat_messages`, fanned out to
  // every client attached to the thread (mirrors
  // `ProviderRuntimeEvent::UserMessage`). No provider emits this: the
  // command layer mints it right after persisting the row, with the SAME
  // serialized shape as the stored envelope, so a live copy and a replayed
  // row fold through one reducer case.
  //
  // It exists because user turns are the only transcript-affecting thing
  // providers never echo. Without it, a second client watching the thread
  // received the assistant reply at a higher `persisted_id`, advanced
  // `lastPersistedEventId` past a user row it never saw, and then skipped
  // that row on every `id > cursor` tail read thereafter. The sending
  // client already painted the bubble and drops this copy by
  // `client_nonce`.
  | {
      type: "user_message";
      thread_id: string;
      text: string;
      /** On-disk image records; absent for a text-only turn and for rows
       *  persisted before the field existed. Each `path` is an absolute
       *  filesystem location the webview loads via the asset protocol. */
      images?: Array<{ path: string; media_type?: string }>;
      /** The composer's correlation token for the optimistic bubble this
       *  turn was sent as. Absent when the sender supplied none. */
      client_nonce?: string;
    }
  // Follow-up queueing (mirrors `ProviderRuntimeEvent` in
  // src-tauri/src/agent_provider/events.rs). A send that arrives while a
  // turn is in flight is queued instead of rejected.
  | {
      type: "turn_queued";
      thread_id: string;
      queued_id: string;
      /** Echoes the optimistic-send correlation token so the reducer can
       *  grey out the already-appended bubble instead of duplicating it.
       *  `null` for older callers / a remounted pane. */
      client_nonce: string | null;
      text: string;
    }
  | {
      type: "queued_turn_dispatched";
      thread_id: string;
      queued_id: string;
      turn_id: string;
      text: string;
    }
  | {
      type: "queued_turn_cancelled";
      thread_id: string;
      queued_id: string;
    }
  // Stall watchdog (mirrors `ProviderRuntimeEvent::RunStalled` in
  // src-tauri/src/agent_provider/events.rs). Emitted when a mid-turn
  // thread has produced no runtime events past the stall threshold.
  // Advisory only — transient, never persisted, and cleared by the
  // reducer on the next real activity.
  | {
      type: "run_stalled";
      thread_id: string;
      /** Seconds since the last observed runtime event for this thread. */
      silent_for_secs: number;
    };

/** Canonical provider event payload as delivered to the frontend —
 *  over the per-thread Channel for thread-scoped events, or on the
 *  `agent_chat_event` bus for thread-less warnings (empty
 *  `thread_id`). */
export interface AgentChatEventPayload {
  thread_id: string;
  event: ProviderRuntimeEvent;
  /** `agent_chat_messages.id` of the row this event was persisted as,
   *  when it was persisted at all (ephemeral kinds carry `null`). The
   *  store dedups a resume tail against the live stream on this id — see
   *  `ChatThreadSlice.lastPersistedEventId`. */
  persisted_id?: number | null;
}

export interface AgentChatTurnCheckpointPayload {
  thread_id: string;
  checkpoint: import("./commands").AgentChatTurnCheckpointRecord;
  oldest_turn_index: number;
}

export interface AgentChatTurnCheckpointRevertedPayload {
  thread_id: string;
  turn_index: number;
  transcript_cutoff_id: number;
  remaining_checkpoints: import("./commands").AgentChatTurnCheckpointRecord[];
}

export const onAgentChatTurnCheckpoint = (
  cb: EventCallback<AgentChatTurnCheckpointPayload>,
): Promise<UnlistenFn> =>
  listen<AgentChatTurnCheckpointPayload>("agent_chat_turn_checkpoint", (e) =>
    cb(e.payload),
  );

export const onAgentChatTurnCheckpointReverted = (
  cb: EventCallback<AgentChatTurnCheckpointRevertedPayload>,
): Promise<UnlistenFn> =>
  listen<AgentChatTurnCheckpointRevertedPayload>(
    "agent_chat_turn_checkpoint_reverted",
    (e) => cb(e.payload),
  );

export const onAgentChatTurnCheckpointsInvalidated = (
  cb: EventCallback<string>,
): Promise<UnlistenFn> =>
  listen<string>("agent_chat_turn_checkpoints_invalidated", (e) =>
    cb(e.payload),
  );

// ── Tunnel health ──
//
// Mirror of src-tauri/src/ssh/tunnel_supervisor.rs:TunnelStatus
// (serde tag = "kind", snake_case) and the registry's
// TunnelStatusEvent. Lets the overview/sidebar render a
// "Reconnecting…" / "Connection lost — re-push" pill on remote
// workspaces instead of a silent freeze.
export type TunnelStatus =
  | { kind: "pending" }
  | { kind: "connected"; ssh_pid: number }
  | { kind: "reconnecting"; attempt: number; delay_ms: number }
  | { kind: "circuit_open"; recent_failures: number };

export interface TunnelStatusPayload {
  workspace_id: string;
  status: TunnelStatus;
}

export const onTunnelStatusChanged = (
  cb: EventCallback<TunnelStatusPayload>,
): Promise<UnlistenFn> =>
  listen<TunnelStatusPayload>("tunnel-status-changed", (e) => cb(e.payload));
