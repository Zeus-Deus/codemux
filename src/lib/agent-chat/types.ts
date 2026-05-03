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
}

export interface AssistantMessageItem {
  kind: "assistant_message";
  id: ChatItemId;
  seq: number;
  turn_id: string | null;
  text: string;
  streaming: boolean;
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
