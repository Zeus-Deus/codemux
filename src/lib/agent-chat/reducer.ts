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

/**
 * Hard cap on retained transcript items per thread. The reducer keeps every
 * message in memory so streaming deltas can mutate the trailing assistant
 * row in place; nothing prunes it. Long-running threads (10k+ items) push
 * the spread cost on every event and the React fan-out into territory
 * that visibly stalls the main thread. The cap only fires when the thread
 * is between turns and has no pending approvals — dropping head items
 * mid-turn would orphan `tool_use_id` / `request_id` correlation lookups
 * (`findToolCallByUseId`, `findPermissionRequest`) that scan the full array.
 */
const MAX_MESSAGES_PER_THREAD = 5_000;
const TRIM_TARGET_MESSAGES = 4_000;

function maybeCapMessages(state: ChatThreadState): ChatThreadState {
  if (state.messages.length <= MAX_MESSAGES_PER_THREAD) return state;
  if (state.streaming) return state;
  if (state.pendingRequestIds.length > 0) return state;
  const drop = state.messages.length - TRIM_TARGET_MESSAGES;
  return { ...state, messages: state.messages.slice(drop) };
}

/** Tools whose UI is owned by a dedicated specialized renderer
 *  (`PlanProposalBlock` inline, `ComposerPendingInputPanel` attached
 *  to the composer) driven by the `permission_request` row. We
 *  deliberately skip creating a `ToolCallItem` for these so Stage 1's
 *  tool_use_id merge path doesn't swallow the request into a generic
 *  ToolCallCard and shadow the specialized renderer. */
const SPECIALIZED_TOOLS = new Set(["ExitPlanMode", "AskUserQuestion"]);

/** Request kinds that render via a specialized block rather than the
 *  generic approval footer. Used by the `tool_result` placeholder
 *  guard so a ghost "(pending)" tool card never materialises for a
 *  specialized tool even if the SDK starts emitting tool_result for
 *  them in a future version. */
const SPECIALIZED_REQUEST_KINDS = new Set(["plan", "user-input"]);

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
  return maybeCapMessages(appendUserMessageLocal(state, text));
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

/**
 * Local-only action: mark a permission request as resolved. Used for
 * synthetic decisions where there is no round-trip through the sidecar
 * (e.g. plan accept/reject — the sidecar has already denied + interrupted
 * the ExitPlanMode tool use, so no `request-resolved` notification ever
 * fires). Without this, the plan card stays in `pending` forever and
 * pins the transcript tail in a "user must act" state.
 */
export function markRequestResolved(
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
      resolution: { state: "resolved", decision },
    }),
    pendingRequestIds: state.pendingRequestIds.filter((id) => id !== requestId),
  };
}

export function applyEvent(
  state: ChatThreadState,
  event: ProviderRuntimeEvent,
): ChatThreadState {
  return maybeCapMessages(applyEventInner(state, event));
}

function applyEventInner(
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
      // Only continue an existing assistant message when it's both
      // the CURRENT TAIL and still streaming. If anything else has
      // landed after the last assistant (tool call, permission
      // request, turn-ended marker, …), those events mark the
      // boundary between two content blocks — the incoming deltas
      // belong to a FRESH assistant message at a new seq, otherwise
      // they'd render at the sealed block's (now stale) position and
      // visually leapfrog over the intervening items. This is the
      // AskUserQuestion "Answered appears above older tool calls"
      // bug: after the user's answer, the next text deltas would
      // back-merge onto the turn's first assistant.
      const tail = state.messages[state.messages.length - 1];
      if (
        tail &&
        tail.kind === "assistant_message" &&
        tail.streaming &&
        (!tail.turn_id || tail.turn_id === event.turn_id)
      ) {
        const lastIndex = state.messages.length - 1;
        const next: AssistantMessageItem = {
          ...tail,
          turn_id: event.turn_id,
          text: tail.text + delta.text,
          streaming: true,
        };
        return {
          ...state,
          streaming: true,
          messages: replaceItem(state.messages, lastIndex, next),
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
          // Specialized tools (ExitPlanMode, AskUserQuestion) drive
          // their own UI through a `permission_request` row — skipping
          // the ToolCallItem here prevents Stage 1's tool_use_id merge
          // from stamping `approval_request_id` on an otherwise-unused
          // tool card and suppressing the specialized renderer in
          // MessageList.
          if (SPECIALIZED_TOOLS.has(item.tool_name)) {
            return state;
          }
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
          // Defensive association: if a permission_request with a
          // matching tool_use_id already exists (request_opened
          // landed before the assistant tool_use — unusual but
          // possible depending on SDK ordering), link the new tool
          // call to it up front.
          const priorRequest = seqBumped.messages.find(
            (m): m is PermissionRequestItem =>
              m.kind === "permission_request" &&
              m.tool_use_id === item.tool_use_id,
          );
          const newToolCall: ToolCallItem = {
            kind: "tool_call",
            id: nextId("tool"),
            seq,
            tool_use_id: item.tool_use_id,
            tool_name: item.tool_name,
            input: item.input,
            status: "running",
            result_content: null,
            approval_request_id: priorRequest?.request_id ?? null,
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
          // Specialized tools never emit a tool_result in practice
          // (ExitPlanMode is denied by the sidecar, AskUserQuestion's
          // "result" is the user's answer routed back through
          // `respond-to-request`). Guard defensively: if the
          // tool_use_id matches a pending specialized request, don't
          // create a ghost "(pending)" placeholder that would
          // resurrect the very card the tool_use guard prevented.
          const specializedPending = state.messages.find(
            (m): m is PermissionRequestItem =>
              m.kind === "permission_request" &&
              m.tool_use_id === item.tool_use_id &&
              SPECIALIZED_REQUEST_KINDS.has(m.request_kind),
          );
          if (specializedPending) {
            return state;
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
            approval_request_id: null,
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
        tool_use_id: event.tool_use_id,
        resolution: { state: "pending" },
      };
      // When the request is tied to an in-flight tool_use, link the
      // two so the renderer can show an inline approval footer on the
      // ToolCallCard. Falls through without mutation when no match
      // exists (plan/user-input standalone requests, or a tool_use
      // that hasn't landed yet — the latter is unexpected in Claude's
      // ordering but handled defensively by keeping the standalone
      // row until tool_use arrives, at which point we stamp it).
      let messages: ChatViewItem[] = [...seqBumped.messages, item];
      if (event.tool_use_id) {
        const toolMatch = findToolCallByUseId(messages, event.tool_use_id);
        if (toolMatch) {
          const patched: ToolCallItem = {
            ...toolMatch.item,
            approval_request_id: event.request_id,
          };
          messages = replaceItem(messages, toolMatch.index, patched);
        }
      }
      return {
        ...seqBumped,
        messages,
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
