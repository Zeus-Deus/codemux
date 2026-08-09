import type {
  ChatViewItem,
  ReasoningItem,
  SubagentRunItem,
  ToolCallItem,
  TurnEndedItem,
  UserMessageItem,
  WorkflowRunItem,
} from "@/lib/agent-chat/types";
import { hasToolResultImages } from "@/lib/agent-chat/tool-result-images";

import { isTaskSummaryTool } from "./TaskSummaryCard";

/** A mechanical step rendered through the compact work log. */
export type ActivityStep = ReasoningItem | ToolCallItem;

export interface TurnFoldBody {
  kind: "turn_fold";
  turnId: string;
  label: string;
  expanded: boolean;
  hiddenCount: number;
  failedCount: number;
}

export type SlotBody =
  | { kind: "item"; item: ChatViewItem }
  | { kind: "activity"; items: ActivityStep[]; working: boolean }
  | { kind: "subagent_stretch"; runs: SubagentRunItem[] }
  | TurnFoldBody;

export interface TranscriptSlot {
  key: string;
  messageId: string;
  /** Turn-boundary anchor used by the send/scroll contract. */
  scrollAnchor: boolean;
  side: "user" | "assistant";
  /** Kept for renderer compatibility; the refined timeline no longer draws
   *  an avatar rail for prose or mechanical work. */
  showAvatar: boolean;
  /** Begins a new visual turn and receives the larger top rhythm. */
  turnStart: boolean;
  body: SlotBody;
}

type PresentationEntry =
  /** `revealed` marks an item the user pulled back out of an expanded turn
   *  fold — it was explicitly asked for, so the quiet-observation filter
   *  below leaves it alone. */
  | { kind: "item"; item: ChatViewItem; revealed?: boolean }
  | { kind: "turn_fold"; body: TurnFoldBody };

interface TurnSegment {
  user: UserMessageItem | null;
  items: ChatViewItem[];
}

/** A successful observational call carries no signal on its own. Bursts still
 * roll up into one work-log line; errors, in-flight calls, approvals, and
 * image-bearing reads always remain visible. */
function isQuietObservationalTool(step: ActivityStep): boolean {
  return (
    step.kind === "tool_call" &&
    step.status === "done" &&
    step.approval_request_id == null &&
    (step.tool_name === "Read" ||
      step.tool_name === "Grep" ||
      step.tool_name === "Glob") &&
    !hasToolResultImages(step.result_content)
  );
}

/**
 * A tool call that folds into the work log. It excludes:
 *  - approval-gated calls (`approval_request_id` set) — the inline
 *    approval footer must render on a standalone `ToolCallCard`.
 *  - TodoWrite / task-summary calls — `TaskSummaryCard` stays a visible
 *    checklist.
 * (`subagent_run` orchestration events are `kind: "subagent_run"`, not
 * `tool_call`, so they never satisfy this predicate. Contiguous spawn groups
 * are merged later into one work-log stretch.)
 */
function isGroupableTool(item: ChatViewItem): item is ToolCallItem {
  return (
    item.kind === "tool_call" &&
    item.approval_request_id == null &&
    !isTaskSummaryTool(item)
  );
}

function isActivityStep(item: ChatViewItem): item is ActivityStep {
  return item.kind === "reasoning" || isGroupableTool(item);
}

function splitTurns(messages: ChatViewItem[]): TurnSegment[] {
  const segments: TurnSegment[] = [];
  let current: TurnSegment | null = null;
  for (const item of messages) {
    if (item.kind === "user_message") {
      if (current) segments.push(current);
      current = { user: item, items: [item] };
      continue;
    }
    if (!current) current = { user: null, items: [] };
    current.items.push(item);
  }
  if (current) segments.push(current);
  return segments;
}

function lastTurnEnd(segment: TurnSegment): TurnEndedItem | null {
  for (let i = segment.items.length - 1; i >= 0; i--) {
    const item = segment.items[i];
    if (item.kind === "turn_ended") return item;
  }
  return null;
}

function terminalAssistantId(segment: TurnSegment): string | null {
  for (let i = segment.items.length - 1; i >= 0; i--) {
    const item = segment.items[i];
    if (item.kind === "assistant_message" && item.text.trim()) return item.id;
  }
  return null;
}

function subagentRunSettled(item: SubagentRunItem): boolean {
  return item.subagents.every(
    (subagent) =>
      subagent.status !== "running" &&
      subagent.status !== "pending",
  );
}

function workflowSettled(item: WorkflowRunItem): boolean {
  return (
    item.status === "completed" ||
    item.status === "failed" ||
    item.status === "stopped"
  );
}

function isFoldableSettledItem(
  item: ChatViewItem,
  terminalId: string | null,
  pendingRequestIds: ReadonlySet<string>,
): boolean {
  switch (item.kind) {
    case "assistant_message":
      return item.id !== terminalId;
    case "reasoning":
      return true;
    case "tool_call":
      return !(
        item.approval_request_id && pendingRequestIds.has(item.approval_request_id)
      );
    case "subagent_run":
      return subagentRunSettled(item);
    case "workflow_run":
      return workflowSettled(item) &&
        !(item.approvalRequestId && pendingRequestIds.has(item.approvalRequestId));
    default:
      return false;
  }
}

function itemFailed(item: ChatViewItem): boolean {
  if (item.kind === "tool_call") return item.status === "error";
  if (item.kind === "workflow_run") return item.status === "failed";
  if (item.kind === "subagent_run") {
    return item.subagents.some((subagent) => subagent.status === "failed");
  }
  return false;
}

function formatWorkedDuration(ms: number): string {
  const totalSeconds = Math.max(1, Math.round(ms / 1_000));
  if (totalSeconds < 60) return `${totalSeconds}s`;
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  if (minutes < 60) return seconds ? `${minutes}m ${seconds}s` : `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  const remainingMinutes = minutes % 60;
  return remainingMinutes ? `${hours}h ${remainingMinutes}m` : `${hours}h`;
}

function foldLabel(user: UserMessageItem | null, ended: TurnEndedItem): string {
  const startedAt = user?.created_at;
  const completedAt = ended.completed_at;
  const duration =
    startedAt != null && completedAt != null && completedAt >= startedAt
      ? formatWorkedDuration(completedAt - startedAt)
      : null;
  const stopped =
    ended.status.kind === "error" && ended.status.subtype === "interrupted";
  if (stopped) return duration ? `You stopped after ${duration}` : "You stopped this response";
  return duration ? `Worked for ${duration}` : "Worked";
}

function turnIdFor(segment: TurnSegment, ended: TurnEndedItem): string {
  if (ended.turn_id) return ended.turn_id;
  for (const item of segment.items) {
    if ("turn_id" in item && item.turn_id) return item.turn_id;
  }
  return `turn-at-${segment.user?.seq ?? segment.items[0]?.seq ?? 0}`;
}

function pendingRequestIds(messages: ChatViewItem[]): Set<string> {
  const ids = new Set<string>();
  for (const item of messages) {
    if (
      item.kind === "permission_request" &&
      (item.resolution.state === "pending" || item.resolution.state === "responding")
    ) {
      ids.add(item.request_id);
    }
  }
  return ids;
}

function activeTurnIndex(segments: TurnSegment[], streaming: boolean): number {
  if (!streaming) return -1;
  for (let i = segments.length - 1; i >= 0; i--) {
    const segment = segments[i];
    if (segment.user?.queued) continue;
    if (!lastTurnEnd(segment)) return i;
  }
  return -1;
}

/**
 * Turn-level presentation derivation. Settled turns retain their terminal
 * assistant answer and replace all routine process output with one quiet fold.
 * Expanding that fold restores the original chronological items.
 */
function buildPresentationEntries(
  messages: ChatViewItem[],
  streaming: boolean,
  expandedTurnIds: ReadonlySet<string>,
): { entries: PresentationEntry[]; workingStepId: string | null } {
  const segments = splitTurns(messages);
  const activeIndex = activeTurnIndex(segments, streaming);
  const pendingIds = pendingRequestIds(messages);
  const entries: PresentationEntry[] = [];
  let workingStepId: string | null = null;

  for (let segmentIndex = 0; segmentIndex < segments.length; segmentIndex++) {
    const segment = segments[segmentIndex];
    const ended = lastTurnEnd(segment);

    if (segmentIndex === activeIndex) {
      for (let i = segment.items.length - 1; i >= 0; i--) {
        const item = segment.items[i];
        if (item.kind === "turn_ended") continue;
        if (isActivityStep(item)) workingStepId = item.id;
        break;
      }
    }

    if (!ended || !segment.user) {
      for (const item of segment.items) {
        if (item.kind === "turn_ended" && item.status.kind !== "error") continue;
        entries.push({ kind: "item", item });
      }
      continue;
    }

    const terminalId = terminalAssistantId(segment);
    const hidden = segment.items.filter((item) =>
      isFoldableSettledItem(item, terminalId, pendingIds),
    );
    const hiddenIds = new Set(hidden.map((item) => item.id));
    const turnId = turnIdFor(segment, ended);
    const expanded = expandedTurnIds.has(turnId);
    const body: TurnFoldBody = {
      kind: "turn_fold",
      turnId,
      label: foldLabel(segment.user, ended),
      expanded,
      hiddenCount: hidden.length,
      failedCount: hidden.filter(itemFailed).length,
    };
    let foldInserted = false;

    for (const item of segment.items) {
      if (item.kind === "turn_ended" && item.status.kind !== "error") continue;
      if (hiddenIds.has(item.id) && !foldInserted) {
        entries.push({ kind: "turn_fold", body });
        foldInserted = true;
      }
      if (hiddenIds.has(item.id) && !expanded) continue;
      entries.push({ kind: "item", item, revealed: hiddenIds.has(item.id) });
    }
  }

  return { entries, workingStepId };
}

export function buildTranscriptSlots(
  messages: ChatViewItem[],
  streaming = false,
  expandedTurnIds: ReadonlySet<string> = new Set(),
): TranscriptSlot[] {
  // Request rows owned by an inline tool/workflow footer do not render twice.
  const mergedRequestIds = new Set<string>();
  for (const item of messages) {
    if (item.kind === "tool_call" && item.approval_request_id) {
      mergedRequestIds.add(item.approval_request_id);
    }
    if (item.kind === "workflow_run" && item.approvalRequestId) {
      mergedRequestIds.add(item.approvalRequestId);
    }
  }

  const { entries, workingStepId } = buildPresentationEntries(
    messages,
    streaming,
    expandedTurnIds,
  );
  const bodies: SlotBody[] = [];
  let run: ActivityStep[] = [];
  let runRevealed = false;
  const flush = () => {
    if (run.length === 0) return;
    const working =
      workingStepId != null && run.some((step) => step.id === workingStepId);
    // A lone settled Read/Grep/Glob carries no signal — drop it rather than
    // leave a one-line work-log row behind. An in-flight one still surfaces,
    // and so does one the user pulled out of an expanded turn fold.
    if (
      !working &&
      !runRevealed &&
      run.length === 1 &&
      isQuietObservationalTool(run[0])
    ) {
      run = [];
      runRevealed = false;
      return;
    }
    const hasTool = run.some((step) => step.kind === "tool_call");
    if (hasTool) {
      bodies.push({ kind: "activity", items: run, working });
    } else {
      for (const step of run) bodies.push({ kind: "item", item: step });
    }
    run = [];
    runRevealed = false;
  };

  for (const entry of entries) {
    if (entry.kind === "turn_fold") {
      flush();
      bodies.push(entry.body);
      continue;
    }
    const { item } = entry;
    if (
      item.kind === "permission_request" &&
      mergedRequestIds.has(item.request_id)
    ) {
      continue;
    }
    if (isActivityStep(item)) {
      run.push(item);
      if (entry.revealed) runRevealed = true;
      continue;
    }
    flush();
    if (item.kind === "subagent_run") {
      // Consecutive spawn groups are one transcript event even when the
      // reducer stores one canonical run per turn. Non-error turn-ended
      // markers were already dropped above, so they do not split a stretch;
      // prose, a visible tool/result, or any other rendered row does.
      const tail = bodies[bodies.length - 1];
      if (tail?.kind === "subagent_stretch") {
        tail.runs.push(item);
      } else {
        bodies.push({ kind: "subagent_stretch", runs: [item] });
      }
      continue;
    }
    bodies.push({ kind: "item", item });
  }
  flush();

  const slots: TranscriptSlot[] = [];
  let prevSide: "user" | "assistant" | null = null;
  for (const body of bodies) {
    const isUser = body.kind === "item" && body.item.kind === "user_message";
    const side: "user" | "assistant" = isUser ? "user" : "assistant";
    const turnStart = side === "user" || prevSide === "user" || prevSide === null;
    const { key, messageId, scrollAnchor } = slotIdentity(body);
    slots.push({
      key,
      messageId,
      scrollAnchor,
      side,
      showAvatar: false,
      turnStart,
      body,
    });
    prevSide = side;
  }
  return slots;
}

export function reuseTranscriptSlots(
  prev: TranscriptSlot[],
  next: TranscriptSlot[],
): TranscriptSlot[] {
  const prevByKey = new Map(prev.map((slot) => [slot.key, slot]));
  let allReused = prev.length === next.length;
  const result = next.map((nextSlot, index) => {
    const prevSlot = prevByKey.get(nextSlot.key);
    if (prevSlot && slotsEquivalent(prevSlot, nextSlot)) {
      if (prevSlot !== prev[index]) allReused = false;
      return prevSlot;
    }
    allReused = false;
    return nextSlot;
  });
  return allReused ? prev : result;
}

function slotsEquivalent(a: TranscriptSlot, b: TranscriptSlot): boolean {
  return (
    a.key === b.key &&
    a.messageId === b.messageId &&
    a.scrollAnchor === b.scrollAnchor &&
    a.side === b.side &&
    a.showAvatar === b.showAvatar &&
    a.turnStart === b.turnStart &&
    bodiesEquivalent(a.body, b.body)
  );
}

function bodiesEquivalent(a: SlotBody, b: SlotBody): boolean {
  if (a.kind === "item" && b.kind === "item") return a.item === b.item;
  if (a.kind === "turn_fold" && b.kind === "turn_fold") {
    return (
      a.turnId === b.turnId &&
      a.label === b.label &&
      a.expanded === b.expanded &&
      a.hiddenCount === b.hiddenCount &&
      a.failedCount === b.failedCount
    );
  }
  if (a.kind === "activity" && b.kind === "activity") {
    return (
      a.working === b.working &&
      a.items.length === b.items.length &&
      a.items.every((item, index) => item === b.items[index])
    );
  }
  if (a.kind === "subagent_stretch" && b.kind === "subagent_stretch") {
    if (a.runs.length !== b.runs.length) return false;
    for (let i = 0; i < a.runs.length; i++) {
      if (a.runs[i] !== b.runs[i]) return false;
    }
    return true;
  }
  return false;
}

function slotIdentity(body: SlotBody): {
  key: string;
  messageId: string;
  scrollAnchor: boolean;
} {
  if (body.kind === "activity") {
    const id = `run:${body.items[0].id}`;
    return { key: id, messageId: id, scrollAnchor: false };
  }
  if (body.kind === "turn_fold") {
    const id = `turn-fold:${body.turnId}`;
    return { key: id, messageId: id, scrollAnchor: false };
  }
  if (body.kind === "subagent_stretch") {
    // The first canonical run anchors the visual stretch. Appending another
    // turn updates this row in place instead of remounting it in LegendList.
    const id = `subagent-stretch:${body.runs[0].id}`;
    return { key: id, messageId: id, scrollAnchor: false };
  }
  return {
    key: body.item.id,
    messageId: body.item.id,
    scrollAnchor: body.item.kind === "user_message",
  };
}
