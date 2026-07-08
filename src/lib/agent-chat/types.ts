import type {
  ApprovalDecision,
  ProviderRuntimeEvent,
  TurnStatus,
} from "@/tauri/events";

export type ChatItemId = string;

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
}

export interface AssistantMessageItem {
  kind: "assistant_message";
  id: ChatItemId;
  seq: number;
  turn_id: string | null;
  text: string;
  streaming: boolean;
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
  resolution:
    | { state: "pending" }
    | { state: "responding"; decision: ApprovalDecision }
    | { state: "resolved"; decision: ApprovalDecision };
}

export interface TurnEndedItem {
  kind: "turn_ended";
  id: ChatItemId;
  seq: number;
  turn_id: string;
  status: TurnStatus;
}

export type ChatViewItem =
  | UserMessageItem
  | AssistantMessageItem
  | ReasoningItem
  | ToolCallItem
  | PermissionRequestItem
  | TurnEndedItem;

export interface ChatThreadState {
  messages: ChatViewItem[];
  streaming: boolean;
  pendingRequestIds: string[];
  /** Next `seq` to assign to a freshly-appended item. Strictly
   *  increasing; never reset across a silent session restart so the
   *  migrated transcript stays ordered relative to new items. */
  nextSeq: number;
}

export function emptyThreadState(): ChatThreadState {
  return {
    messages: [],
    streaming: false,
    pendingRequestIds: [],
    nextSeq: 0,
  };
}

export type AnyProviderEvent = ProviderRuntimeEvent;
