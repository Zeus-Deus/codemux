/**
 * Reducer arms for editing and reordering queued turns.
 *
 * Queued bubbles sit at the bottom of the transcript by `seq` (see
 * `QUEUED_SEQ_BASE` in the reducer), and the composer strip lists them in
 * array order. A reorder therefore permutes both among the bubbles it
 * names: each takes the array slot and seq its new position held, so every
 * other row stays exactly where it was.
 */
import type { ProviderRuntimeEvent } from "@/tauri/events";

import { replaceTranscriptItem } from "./subagents";
import type { ChatThreadState, UserMessageItem } from "./types";

type Edited = Extract<ProviderRuntimeEvent, { type: "queued_turn_edited" }>;
type Reordered = Extract<ProviderRuntimeEvent, { type: "queued_turns_reordered" }>;

function queuedIndex(state: ChatThreadState, queuedId: string): number {
  return state.messages.findIndex(
    (m) => m.kind === "user_message" && m.queued?.queuedId === queuedId,
  );
}

export function editQueuedTurn(state: ChatThreadState, event: Edited): ChatThreadState {
  const index = queuedIndex(state, event.queued_id);
  if (index < 0) return state;
  const item = state.messages[index] as UserMessageItem;
  if (item.text === event.text) return state;
  return {
    ...state,
    messages: replaceTranscriptItem(state.messages, index, { ...item, text: event.text }),
  };
}

export function reorderQueuedTurns(
  state: ChatThreadState,
  event: Reordered,
): ChatThreadState {
  const indices = event.queued_ids
    .map((id) => queuedIndex(state, id))
    .filter((index) => index >= 0);
  if (indices.length < 2) return state;
  const items = indices.map((index) => state.messages[index] as UserMessageItem);
  const slots = [...indices].sort((a, b) => a - b);
  const seqs = items.map((item) => item.seq).sort((a, b) => a - b);
  let messages = state.messages;
  items.forEach((item, position) => {
    const slot = slots[position];
    const current = messages[slot];
    if (current === item && item.seq === seqs[position]) return;
    messages = replaceTranscriptItem(messages, slot, { ...item, seq: seqs[position] });
  });
  return messages === state.messages ? state : { ...state, messages };
}

/** The transcript without queued turns: a queued turn shows in the queue
 *  list above the composer and joins the transcript when it dispatches. */
export function withoutQueuedTurns<T extends { kind: string }>(messages: T[]): T[] {
  const kept = messages.filter(
    (m) => !(m.kind === "user_message" && "queued" in m && m.queued),
  );
  return kept.length === messages.length ? messages : kept;
}
