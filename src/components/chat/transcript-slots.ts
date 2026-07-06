import type {
  ChatViewItem,
  ReasoningItem,
  ToolCallItem,
} from "@/lib/agent-chat/types";

import { isTaskSummaryTool } from "./TaskSummaryCard";

/**
 * Pure transcript layout logic (Activity Stream). Turns the ordered
 * `ChatViewItem[]` into render slots, folding a contiguous run of
 * mechanical steps — reasoning blocks AND tool calls (including running
 * ones and Edit/MultiEdit/Write) — into ONE `activity` slot so the
 * transcript never spams "Thought / Thought / Ran…". The assistant's
 * prose and any standalone approval / plan / task-summary surface stays
 * outside the block. Kept side-effect-free and exported so the grouping
 * rules can be unit-tested directly (jsdom can't exercise real scrolling).
 */

/** A mechanical step that folds into an Activity block. */
export type ActivityStep = ReasoningItem | ToolCallItem;

export type SlotBody =
  | { kind: "item"; item: ChatViewItem }
  | { kind: "activity"; items: ActivityStep[]; working: boolean };

export interface TranscriptSlot {
  /** Stable React key + MessageScroller `messageId`. */
  key: string;
  messageId: string;
  /** Turn-boundary anchor — set on user messages so the scroller pins a
   *  new user turn near the top. */
  scrollAnchor: boolean;
  side: "user" | "assistant";
  /** First assistant-side row of a contiguous assistant run — draws the
   *  avatar; later rows in the run leave the gutter empty. */
  showAvatar: boolean;
  /** Begins a new visual turn (drives the larger top margin). */
  turnStart: boolean;
  body: SlotBody;
}

/**
 * Minimum steps a SETTLED run needs to roll up into an Activity block. A
 * lone completed tool call (or a lone thought) keeps rendering as its own
 * card so a single action never hides behind a summary header. While the
 * run is the live WORKING tail this drops to 1 — a single running tool
 * should still surface as the "Working" line.
 */
const GROUP_MIN = 2;

/**
 * A tool call that folds into an Activity block. Unlike the old
 * tool-group folding this INCLUDES running, errored and Edit/MultiEdit/
 * Write calls (their diff is reachable via the step-row inline
 * expansion). It still excludes:
 *  - approval-gated calls (`approval_request_id` set) — the inline
 *    approval footer must render on a standalone `ToolCallCard`.
 *  - TodoWrite / task-summary calls — `TaskSummaryCard` stays a visible
 *    checklist.
 */
function isGroupableTool(item: ChatViewItem): item is ToolCallItem {
  return (
    item.kind === "tool_call" &&
    item.approval_request_id == null &&
    !isTaskSummaryTool(item)
  );
}

/** Reasoning blocks and groupable tool calls are the mechanical steps a
 *  run absorbs. Everything else (assistant/user prose, permission
 *  requests, turn-ended markers, gated / task-summary tools) breaks it. */
function isActivityStep(item: ChatViewItem): item is ActivityStep {
  return item.kind === "reasoning" || isGroupableTool(item);
}

export function buildTranscriptSlots(
  messages: ChatViewItem[],
  /** Thread-level streaming flag (turn in flight). Only the tail run of an
   *  active turn can be "working"; everything else settles. */
  streaming = false,
): TranscriptSlot[] {
  // Permission requests already owned by a tool card's inline footer
  // (linked via approval_request_id) don't get their own row.
  const mergedRequestIds = new Set<string>();
  for (const m of messages) {
    if (m.kind === "tool_call" && m.approval_request_id) {
      mergedRequestIds.add(m.approval_request_id);
    }
  }

  // Pass 1 — fold contiguous mechanical-step runs into Activity blocks.
  const bodies: SlotBody[] = [];
  let run: ActivityStep[] = [];
  // `isTail` is true only for the terminal flush (no rendered item follows
  // the run). Combined with `streaming`, it decides the working state and
  // relaxes GROUP_MIN to 1 for a single live step.
  const flush = (isTail: boolean) => {
    if (run.length === 0) return;
    const working = streaming && isTail;
    const hasTool = run.some((s) => s.kind === "tool_call");
    // A pure-reasoning run never becomes an Activity block: a lone thought
    // is not "Thought / Thought / Ran…" spam and its live streaming text is
    // better served by ReasoningBlock. It folds in only when contiguous
    // with a tool call.
    if (hasTool && (working || run.length >= GROUP_MIN)) {
      bodies.push({ kind: "activity", items: run, working });
    } else {
      for (const s of run) bodies.push({ kind: "item", item: s });
    }
    run = [];
  };
  for (const item of messages) {
    // Drop rows that render nothing so they never leave a phantom gap.
    if (
      item.kind === "permission_request" &&
      mergedRequestIds.has(item.request_id)
    ) {
      continue;
    }
    if (item.kind === "turn_ended" && item.status.kind !== "error") continue;

    if (isActivityStep(item)) {
      run.push(item);
      continue;
    }
    // A rendered standalone item follows this run → it is not the tail.
    flush(false);
    bodies.push({ kind: "item", item });
  }
  flush(true);

  // Pass 2 — turn boundaries / avatar gutter.
  const slots: TranscriptSlot[] = [];
  let prevSide: "user" | "assistant" | null = null;
  for (const body of bodies) {
    const isUser = body.kind === "item" && body.item.kind === "user_message";
    const side: "user" | "assistant" = isUser ? "user" : "assistant";
    const showAvatar = side === "assistant" && prevSide !== "assistant";
    const turnStart = side === "user" || showAvatar;
    const { key, messageId, scrollAnchor } = slotIdentity(body);
    slots.push({ key, messageId, scrollAnchor, side, showAvatar, turnStart, body });
    prevSide = side;
  }
  return slots;
}

function slotIdentity(body: SlotBody): {
  key: string;
  messageId: string;
  scrollAnchor: boolean;
} {
  if (body.kind === "activity") {
    // Key by the first step's id so the slot identity is stable as the run
    // grows and as it transitions working → settled (a streaming token then
    // mutates one memoized row and the scroller pin holds).
    const id = `run:${body.items[0].id}`;
    return { key: id, messageId: id, scrollAnchor: false };
  }
  const { item } = body;
  return {
    key: item.id,
    messageId: item.id,
    scrollAnchor: item.kind === "user_message",
  };
}
