import type { ChatViewItem, ToolCallItem } from "@/lib/agent-chat/types";

import { isTaskSummaryTool } from "./TaskSummaryCard";

/**
 * Pure transcript layout logic (design D2/D4/D6). Turns the ordered
 * `ChatViewItem[]` into render slots, folding contiguous runs of ≥2
 * completed tool calls into one tool-group slot and computing per-row
 * turn boundaries (avatar / top spacing). Kept side-effect-free and
 * exported so the grouping + turn-boundary rules can be unit-tested
 * directly (jsdom can't exercise real scrolling).
 */

export type SlotBody =
  | { kind: "item"; item: ChatViewItem }
  | { kind: "toolGroup"; items: ToolCallItem[] };

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

/** Minimum consecutive completed tool calls that fold into a group card. */
const GROUP_MIN = 2;

/** Tools whose own card is a prominent standalone surface (a diff), so
 *  they never fold into a group where that surface would be hidden. */
const STANDALONE_TOOLS = new Set(["Edit", "MultiEdit", "Write"]);

/** A completed, ungated, non-special tool call — safe to fold into a
 *  group. Running / pending-approval / error / TodoWrite / diff calls are
 *  excluded so they always render as their own, always-visible card.
 *  `subagent_run` orchestration cards are not tool calls, so they never
 *  fold — each renders as its own standalone full-width slot (like a
 *  diff), and it breaks any surrounding tool run. */
function isGroupable(item: ChatViewItem): item is ToolCallItem {
  return (
    item.kind === "tool_call" &&
    item.status === "done" &&
    item.approval_request_id == null &&
    !STANDALONE_TOOLS.has(item.tool_name) &&
    !isTaskSummaryTool(item)
  );
}

export function buildTranscriptSlots(
  messages: ChatViewItem[],
): TranscriptSlot[] {
  // Permission requests already owned by a tool card's inline footer
  // (linked via approval_request_id) don't get their own row.
  const mergedRequestIds = new Set<string>();
  for (const m of messages) {
    if (m.kind === "tool_call" && m.approval_request_id) {
      mergedRequestIds.add(m.approval_request_id);
    }
  }

  // Pass 1 — grouping.
  const bodies: SlotBody[] = [];
  let run: ToolCallItem[] = [];
  const flush = () => {
    if (run.length === 0) return;
    if (run.length >= GROUP_MIN) {
      bodies.push({ kind: "toolGroup", items: run });
    } else {
      for (const t of run) bodies.push({ kind: "item", item: t });
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

    if (isGroupable(item)) {
      run.push(item);
      continue;
    }
    flush();
    bodies.push({ kind: "item", item });
  }
  flush();

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
  if (body.kind === "toolGroup") {
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
