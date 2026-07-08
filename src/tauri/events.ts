import { emit, listen, type UnlistenFn } from "@tauri-apps/api/event";

export type { UnlistenFn };
import type {
  AppStateSnapshot,
  AuthStatePayload,
  OrchestratorTriggerResult,
  PresetStoreSnapshot,
  TerminalStatusPayload,
  ThemeColors,
  UserSettings,
} from "./types";
import type { AutomationRunView } from "./commands";

export type EventCallback<T> = (payload: T) => void;

export const onAppStateChanged = (cb: EventCallback<AppStateSnapshot>): Promise<UnlistenFn> =>
  listen<AppStateSnapshot>("app-state-changed", (e) => cb(e.payload));

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

export const onOpenflowCycle = (
  cb: EventCallback<OrchestratorTriggerResult>,
): Promise<UnlistenFn> =>
  listen<OrchestratorTriggerResult>("openflow-cycle", (e) => cb(e.payload));

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
    }
  | {
      type: "item_completed";
      thread_id: string;
      turn_id: string;
      item: CompletedItem;
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
    }
  | {
      type: "request_resolved";
      thread_id: string;
      request_id: string;
      decision: ApprovalDecision;
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
    };

/** Canonical provider event payload as delivered to the frontend —
 *  over the per-thread Channel for thread-scoped events, or on the
 *  `agent_chat_event` bus for thread-less warnings (empty
 *  `thread_id`). */
export interface AgentChatEventPayload {
  thread_id: string;
  event: ProviderRuntimeEvent;
}

/** Emitted when the background run-start checkpoint (issue #80) lands,
 *  so the pane header can reveal the restore affordance without
 *  polling. Stays on the GLOBAL event bus (not the per-thread
 *  Channel): the checkpoint task outlives the start_session command
 *  and there is exactly one event per run, so a broadcast with a
 *  thread-id filter on the subscriber side is the right transport.
 *  Mirrors AgentChatCheckpointEventPayload in
 *  src-tauri/src/commands/agent_chat.rs. */
export interface AgentChatCheckpointPayload {
  thread_id: string;
  checkpoint: import("./commands").AgentChatCheckpointRecord;
}

export const onAgentChatCheckpoint = (
  cb: EventCallback<AgentChatCheckpointPayload>,
): Promise<UnlistenFn> =>
  listen<AgentChatCheckpointPayload>("agent_chat_checkpoint", (e) =>
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
