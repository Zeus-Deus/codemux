import type { ApprovalDecision, ProviderRuntimeEvent } from "@/tauri/events";

import {
  emptyThreadState,
  type AssistantMessageItem,
  type ChatThreadState,
  type ChatViewItem,
  type PermissionRequestItem,
  type ToolCallItem,
} from "./types";

let idCounter = 0;
function nextId(prefix: string): string {
  idCounter += 1;
  return `${prefix}-${idCounter}`;
}

const warnedVariants = new Set<string>();
function warnOnce(variant: string, payload: unknown) {
  if (warnedVariants.has(variant)) return;
  warnedVariants.add(variant);
  console.warn(`[agent-chat reducer] unhandled event variant: ${variant}`, payload);
}

/**
 * Consume the thread's `nextSeq` for a newly-created item. Returns the
 * seq to assign and a state clone with `nextSeq` bumped.
 *
 * Only call this when APPENDING a new item. Items that are MUTATED in
 * place (delta text append, tool_result attach, permission_request
 * resolve) must preserve their original seq.
 */
function takeSeq(state: ChatThreadState): { seq: number; next: ChatThreadState } {
  return { seq: state.nextSeq, next: { ...state, nextSeq: state.nextSeq + 1 } };
}

function replaceItem(
  messages: ChatViewItem[],
  index: number,
  next: ChatViewItem,
): ChatViewItem[] {
  const copy = messages.slice();
  copy[index] = next;
  return copy;
}

function findTrailingAssistant(
  messages: ChatViewItem[],
  turnId: string,
): { index: number; item: AssistantMessageItem } | null {
  for (let i = messages.length - 1; i >= 0; i--) {
    const item = messages[i];
    if (item.kind !== "assistant_message") continue;
    if (item.turn_id && item.turn_id !== turnId) return null;
    return { index: i, item };
  }
  return null;
}

function findToolCallByUseId(
  messages: ChatViewItem[],
  toolUseId: string,
): { index: number; item: ToolCallItem } | null {
  for (let i = messages.length - 1; i >= 0; i--) {
    const item = messages[i];
    if (item.kind === "tool_call" && item.tool_use_id === toolUseId) {
      return { index: i, item };
    }
  }
  return null;
}

function findPermissionRequest(
  messages: ChatViewItem[],
  requestId: string,
): { index: number; item: PermissionRequestItem } | null {
  for (let i = messages.length - 1; i >= 0; i--) {
    const item = messages[i];
    if (item.kind === "permission_request" && item.request_id === requestId) {
      return { index: i, item };
    }
  }
  return null;
}

function appendUserMessageLocal(
  state: ChatThreadState,
  text: string,
): ChatThreadState {
  const { seq, next } = takeSeq(state);
  const item: ChatViewItem = {
    kind: "user_message",
    id: nextId("user"),
    seq,
    text,
  };
  return { ...next, messages: [...next.messages, item] };
}

/**
 * Local-only action: append a user turn the composer just submitted.
 * Providers do not echo user messages back in the event stream, so the
 * UI inserts them optimistically.
 */
export function appendUserMessage(
  state: ChatThreadState,
  text: string,
): ChatThreadState {
  return appendUserMessageLocal(state, text);
}

/**
 * Local-only action: mark a permission request as "responding" so the
 * UI can render a pending state while the Tauri invoke is in flight.
 */
export function markRequestResponding(
  state: ChatThreadState,
  requestId: string,
  decision: ApprovalDecision,
): ChatThreadState {
  const found = findPermissionRequest(state.messages, requestId);
  if (!found) return state;
  return {
    ...state,
    messages: replaceItem(state.messages, found.index, {
      ...found.item,
      resolution: { state: "responding", decision },
    }),
  };
}

export function applyEvent(
  state: ChatThreadState,
  event: ProviderRuntimeEvent,
): ChatThreadState {
  switch (event.type) {
    case "session_configured": {
      // Nothing to render in the transcript yet; the composer/footer
      // read session config via separate commands.
      return state;
    }

    case "session_state_changed": {
      const status = event.status;
      if (status.status === "running") {
        if (state.streaming) return state;
        return { ...state, streaming: true };
      }
      if (
        status.status === "ready" ||
        status.status === "closed" ||
        status.status === "error"
      ) {
        if (!state.streaming) return state;
        return { ...state, streaming: false };
      }
      return state;
    }

    case "content_delta": {
      const delta = event.delta;
      if (delta.kind !== "text") {
        // Thinking deltas and tool_input deltas are not rendered in
        // Step 2's transcript.
        return state;
      }
      const existing = findTrailingAssistant(state.messages, event.turn_id);
      if (existing) {
        const next: AssistantMessageItem = {
          ...existing.item,
          turn_id: event.turn_id,
          text: existing.item.text + delta.text,
          streaming: true,
        };
        return {
          ...state,
          streaming: true,
          messages: replaceItem(state.messages, existing.index, next),
        };
      }
      const { seq, next: seqBumped } = takeSeq(state);
      const newAssistant: AssistantMessageItem = {
        kind: "assistant_message",
        id: nextId("assistant"),
        seq,
        turn_id: event.turn_id,
        text: delta.text,
        streaming: true,
      };
      return {
        ...seqBumped,
        streaming: true,
        messages: [...seqBumped.messages, newAssistant],
      };
    }

    case "item_completed": {
      const item = event.item;
      switch (item.kind) {
        case "assistant_text": {
          const existing = findTrailingAssistant(state.messages, event.turn_id);
          if (existing && existing.item.streaming) {
            const next: AssistantMessageItem = {
              ...existing.item,
              turn_id: event.turn_id,
              text: item.text,
              streaming: false,
            };
            return {
              ...state,
              messages: replaceItem(state.messages, existing.index, next),
            };
          }
          const { seq, next: seqBumped } = takeSeq(state);
          const newAssistant: AssistantMessageItem = {
            kind: "assistant_message",
            id: nextId("assistant"),
            seq,
            turn_id: event.turn_id,
            text: item.text,
            streaming: false,
          };
          return {
            ...seqBumped,
            messages: [...seqBumped.messages, newAssistant],
          };
        }
        case "assistant_thinking": {
          // Not rendered in Step 2.
          return state;
        }
        case "tool_use": {
          // Tool input may have streamed in via content_delta tool_input;
          // we ignore those deltas in Step 2 and attach the finalised
          // tool_use here. If the same tool_use_id already exists (e.g.
          // a tool_result came first), attach to it.
          const found = findToolCallByUseId(state.messages, item.tool_use_id);
          if (found) {
            const next: ToolCallItem = {
              ...found.item,
              tool_name: item.tool_name,
              input: item.input,
            };
            return {
              ...state,
              messages: replaceItem(state.messages, found.index, next),
            };
          }
          const { seq, next: seqBumped } = takeSeq(state);
          const newToolCall: ToolCallItem = {
            kind: "tool_call",
            id: nextId("tool"),
            seq,
            tool_use_id: item.tool_use_id,
            tool_name: item.tool_name,
            input: item.input,
            status: "running",
            result_content: null,
          };
          return { ...seqBumped, messages: [...seqBumped.messages, newToolCall] };
        }
        case "tool_result": {
          const found = findToolCallByUseId(state.messages, item.tool_use_id);
          if (found) {
            const next: ToolCallItem = {
              ...found.item,
              status: item.is_error ? "error" : "done",
              result_content: item.content,
            };
            return {
              ...state,
              messages: replaceItem(state.messages, found.index, next),
            };
          }
          // Result arrived before we saw the tool_use. Create a placeholder
          // carrying the result so nothing is lost; the tool_use item
          // handler will attach when the use shows up.
          const { seq, next: seqBumped } = takeSeq(state);
          const placeholder: ToolCallItem = {
            kind: "tool_call",
            id: nextId("tool"),
            seq,
            tool_use_id: item.tool_use_id,
            tool_name: "(pending)",
            input: null,
            status: item.is_error ? "error" : "done",
            result_content: item.content,
          };
          return {
            ...seqBumped,
            messages: [...seqBumped.messages, placeholder],
          };
        }
        default: {
          warnOnce(`item_completed/${(item as { kind: string }).kind}`, item);
          return state;
        }
      }
    }

    case "turn_completed": {
      // Seal any still-streaming assistant message for this turn.
      let messages = state.messages;
      const existing = findTrailingAssistant(messages, event.turn_id);
      if (existing && existing.item.streaming) {
        const sealed: AssistantMessageItem = {
          ...existing.item,
          streaming: false,
        };
        messages = replaceItem(messages, existing.index, sealed);
      }
      // On error status, surface a single turn_ended item so the user
      // sees the failure in-flow. Success/max_turns/max_budget are
      // silent — the absent streaming + completed content is enough.
      if (event.status.kind === "error") {
        const { seq, next: seqBumped } = takeSeq({ ...state, messages });
        return {
          ...seqBumped,
          streaming: false,
          messages: [
            ...seqBumped.messages,
            {
              kind: "turn_ended",
              id: nextId("turn-end"),
              seq,
              turn_id: event.turn_id,
              status: event.status,
            },
          ],
        };
      }
      return { ...state, streaming: false, messages };
    }

    case "request_opened": {
      const existing = findPermissionRequest(state.messages, event.request_id);
      if (existing) return state;
      const { seq, next: seqBumped } = takeSeq(state);
      const item: PermissionRequestItem = {
        kind: "permission_request",
        id: nextId("req"),
        seq,
        request_id: event.request_id,
        turn_id: event.turn_id,
        request_kind: event.request_kind,
        payload: event.payload,
        resolution: { state: "pending" },
      };
      return {
        ...seqBumped,
        messages: [...seqBumped.messages, item],
        pendingRequestIds: seqBumped.pendingRequestIds.includes(event.request_id)
          ? seqBumped.pendingRequestIds
          : [...seqBumped.pendingRequestIds, event.request_id],
      };
    }

    case "request_resolved": {
      const found = findPermissionRequest(state.messages, event.request_id);
      if (!found) return state;
      const next: PermissionRequestItem = {
        ...found.item,
        resolution: { state: "resolved", decision: event.decision },
      };
      return {
        ...state,
        messages: replaceItem(state.messages, found.index, next),
        pendingRequestIds: state.pendingRequestIds.filter(
          (id) => id !== event.request_id,
        ),
      };
    }

    case "runtime_warning": {
      // Runtime warnings carry SDK-lifecycle debug strings meant for
      // devtools, not the transcript. Surface to console only.
      console.warn("[agent-chat]", event.message, event.original_payload ?? event);
      return state;
    }

    case "resume_cursor_updated": {
      // Transcript-level no-op: the store slice tracks resume_cursor
      // outside of ChatThreadState so the reducer stays pure.
      return state;
    }

    default: {
      warnOnce((event as { type: string }).type ?? "unknown", event);
      return state;
    }
  }
}

export function createEmptyThreadState(): ChatThreadState {
  return emptyThreadState();
}

// Test helper — resets the monotonic id counter so tests get stable ids.
export function __resetReducerIdCounterForTests() {
  idCounter = 0;
  warnedVariants.clear();
}
