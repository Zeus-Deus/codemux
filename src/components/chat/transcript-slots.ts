import { isWorkspaceLaunchCall } from "@/lib/agent-chat/workspace-launch";
import {
  isDelegatedRun,
  isDelegateToolCall,
  linkDelegations,
} from "@/lib/agent-chat/delegation";
import type {
  ChatViewItem,
  ReasoningItem,
  SubagentRunItem,
  SubagentView,
  ToolCallItem,
  TurnEndedItem,
  UserMessageItem,
  WorkflowRunItem,
} from "@/lib/agent-chat/types";
import { isAppReceipt } from "@/lib/agent-chat/types";
import {
  isSuggestToolCall as isSuggestToolCallGuard,
  linkSuggestedTasks,
  viewsForCall,
} from "@/lib/agent-chat/follow-ups";

/** Not a type guard: a refused `suggest_tasks` call is still a tool call,
 *  so the negative branch must not narrow `tool_call` away. */
function isSuggestToolCall(item: ChatViewItem): boolean {
  return isSuggestToolCallGuard(item);
}
import { isInlineHtmlRender } from "@/lib/agent-chat/html-render";
import { pluginCardFromToolItem } from "@/components/plugins/cards";
import { hasToolResultImages } from "@/lib/agent-chat/tool-result-images";

import { isTaskSummaryTool } from "./TaskSummaryCard";
import {
  summarizeTurnChanges,
  turnChangesEqual,
  type TurnChangeSummary,
} from "./turn-changes";

/** A mechanical step rendered through the compact work log. */
export type ActivityStep = ReasoningItem | ToolCallItem;

/** Anything that shares one work-log line: steps and subagent runs. */
export type WorkEntry = ActivityStep | SubagentRunItem;

export interface TurnFoldBody {
  kind: "turn_fold";
  turnId: string;
  label: string;
  expanded: boolean;
  /** The work-entry ids holding this fold open: the user opened those work
   *  logs while the turn ran (see `buildTranscriptSlots`). Collapsing the
   *  fold has to release every one of them. */
  pinnedBy: readonly string[];
  /** 0 when the turn has nothing to fold (a plain answer): the row is then
   *  just its duration, not a toggle. */
  hiddenCount: number;
  failedCount: number;
  /** Files the turn's edit calls wrote; absent when it wrote none. */
  changes?: TurnChangeSummary;
  /** Set while the turn still runs: the header ticks "Working for …" from
   *  this start time (null when the prompt carries none) and folds nothing
   *  yet. It holds the slot the settled "Worked for …" fold takes, so the
   *  turn's frame stays put when it ends. */
  liveSince?: number | null;
}

/** One cross-provider delegation on a transcript card: the `delegate_task`
 *  call that started it, the live row the backend reports for it, or both
 *  once they are linked. */
export interface DelegationEntry {
  /** Stable key: the call's id, else the orphan row's id. */
  key: string;
  call: ToolCallItem | null;
  view: SubagentView | null;
}

export type SlotBody =
  | {
      kind: "item";
      item: ChatViewItem;
      /** Running narration renders without the settled answer’s footer. */
      commentary?: boolean;
      /** Belongs to the turn still running, so a `running` call on it really
       *  is executing. Absent for a settled turn. */
      turnLive?: boolean;
    }
  | {
      kind: "activity";
      items: WorkEntry[];
      /** Holds the active turn's newest work entry: the live row. */
      working: boolean;
      /** Belongs to the turn still running, so the user opening it should
       *  pin the fold that will settle over it. */
      turnLive: boolean;
    }
  /** Contiguous delegations, one card. Never folded into the work log or
   *  a settled turn: the hand-off is the turn's visible outcome. */
  | { kind: "delegation"; entries: DelegationEntry[] }
  /** A `suggest_tasks` call: the suggested follow-up tasks, with the live
   *  row of each one the user started (by index, null when not started).
   *  Never folded: the user acts on it after the turn. */
  | {
      kind: "suggested_tasks";
      call: ToolCallItem;
      views: ReadonlyArray<SubagentView | null>;
    }
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
  | { kind: "item"; item: ChatViewItem; revealed?: boolean; turnLive?: boolean }
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
 *  - `html_render` pages — the page is part of the reply, not process.
 *  - plugin cards (`plugin_invoke` results carrying one) — likewise.
 *  - `delegate_task` calls — they render as the delegation card.
 *  - `suggest_tasks` calls — they render as the suggested-tasks card.
 *  - `workspace_launch` calls — the hand-off stays visible beside the reply.
 * (`subagent_run` items are not tool calls; the slot builder joins them into
 * the same work-log line separately.)
 */
function isGroupableTool(item: ChatViewItem): item is ToolCallItem {
  return (
    item.kind === "tool_call" &&
    item.approval_request_id == null &&
    !isTaskSummaryTool(item) &&
    !isInlineHtmlRender(item) &&
    pluginCardFromToolItem(item) === undefined &&
    !isDelegateToolCall(item) &&
    !isSuggestToolCall(item) &&
    !isWorkspaceLaunchCall(item)
  );
}

function isActivityStep(item: ChatViewItem): item is ActivityStep {
  return item.kind === "reasoning" || isGroupableTool(item);
}

function isWorkEntry(item: ChatViewItem): item is WorkEntry {
  return (
    (item.kind === "subagent_run" && !isDelegatedRun(item)) ||
    isActivityStep(item)
  );
}

/** A row that belongs on the delegation card rather than anywhere else. */
function isDelegationItem(item: ChatViewItem): boolean {
  return (
    (item.kind === "subagent_run" && isDelegatedRun(item)) ||
    isDelegateToolCall(item)
  );
}

function splitTurns(messages: ChatViewItem[]): TurnSegment[] {
  const segments: TurnSegment[] = [];
  let current: TurnSegment | null = null;
  for (const item of messages) {
    if (item.kind === "user_message" && !item.inflight) {
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

/** The boundary that actually ends the turn. An `interim` marker is the
 *  provider yielding to wait on background work before resuming the same
 *  turn, so it neither settles the segment nor stamps its duration. */
function lastTurnEnd(segment: TurnSegment): TurnEndedItem | null {
  for (let i = segment.items.length - 1; i >= 0; i--) {
    const item = segment.items[i];
    if (item.kind === "turn_ended" && !item.interim) return item;
  }
  return null;
}

/** Success boundaries render nothing, and neither does the error that
 *  closed a usage-limited run: its `usage_limit` row is the record. */
function silentTurnEnd(item: TurnEndedItem): boolean {
  return item.status.kind !== "error" || item.usageLimited === true;
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
  // A delegation outlives its turn and stays where it was handed off,
  // suggested tasks wait for the user after it, and a workspace launch
  // stays beside the reply that announced it.
  if (
    isDelegationItem(item) ||
    isSuggestToolCall(item) ||
    (item.kind === "tool_call" && isWorkspaceLaunchCall(item))
  )
    return false;
  switch (item.kind) {
    case "assistant_message":
      return item.id !== terminalId;
    case "reasoning":
      return true;
    case "tool_call":
      // An `html_render` page, or a plugin's card, stays beside the answer it belongs to.
      return !(
        (item.approval_request_id && pendingRequestIds.has(item.approval_request_id)) ||
        isInlineHtmlRender(item) ||
        pluginCardFromToolItem(item) !== undefined
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

export function formatWorkedDuration(ms: number): string {
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

function liveTurnHeader(user: UserMessageItem): TurnFoldBody {
  return {
    kind: "turn_fold",
    turnId: `live:${user.id}`,
    label: "Working",
    expanded: false,
    pinnedBy: [],
    hiddenCount: 0,
    failedCount: 0,
    liveSince: user.created_at ?? null,
  };
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
 *
 * `expandedTurnIds` holds turn ids the user expanded, plus the ids of work
 * entries whose log the user opened mid-run. A fold that settles over such
 * an entry starts expanded, so the log being read does not vanish when the
 * turn ends.
 */
function buildPresentationEntries(
  messages: ChatViewItem[],
  streaming: boolean,
  expandedTurnIds: ReadonlySet<string>,
): {
  entries: PresentationEntry[];
  workingEntryId: string | null;
  commentaryIds: Set<string>;
} {
  const segments = splitTurns(messages);
  const activeIndex = activeTurnIndex(segments, streaming);
  const pendingIds = pendingRequestIds(messages);
  const entries: PresentationEntry[] = [];
  const commentaryIds = new Set<string>();
  let workingEntryId: string | null = null;

  for (let segmentIndex = 0; segmentIndex < segments.length; segmentIndex++) {
    const segment = segments[segmentIndex];
    const ended = lastTurnEnd(segment);
    const terminalId = terminalAssistantId(segment);

    // Only a finished turn's last answer is the reply; everything else the
    // assistant said along the way is narration.
    for (const item of segment.items) {
      if (
        item.kind === "assistant_message" &&
        (segmentIndex === activeIndex || item.id !== terminalId)
      ) {
        commentaryIds.add(item.id);
      }
    }

    if (segmentIndex === activeIndex) {
      for (let i = segment.items.length - 1; i >= 0; i--) {
        const item = segment.items[i];
        if (item.kind === "turn_ended" || isAppReceipt(item)) continue;
        if (isWorkEntry(item)) workingEntryId = item.id;
        break;
      }
    }

    if (!ended || !segment.user) {
      const turnLive = segmentIndex === activeIndex;
      for (const item of segment.items) {
        if (item.kind === "turn_ended" && silentTurnEnd(item)) continue;
        entries.push({ kind: "item", item, turnLive });
        if (turnLive && item === segment.user) {
          entries.push({ kind: "turn_fold", body: liveTurnHeader(segment.user) });
        }
      }
      continue;
    }

    const hidden = segment.items.filter((item) =>
      isFoldableSettledItem(item, terminalId, pendingIds),
    );
    const hiddenIds = new Set(hidden.map((item) => item.id));
    const turnId = turnIdFor(segment, ended);
    // Read pins off every entry, not just the foldable ones: a log whose
    // first entry is still running (a background subagent) stays out of
    // `hidden`, yet the rest of that log would fold away under the reader.
    const pinnedBy = segment.items
      .filter((item) => isWorkEntry(item) && expandedTurnIds.has(item.id))
      .map((item) => item.id);
    const expanded = expandedTurnIds.has(turnId) || pinnedBy.length > 0;
    const body: TurnFoldBody = {
      kind: "turn_fold",
      turnId,
      label: foldLabel(segment.user, ended),
      expanded,
      pinnedBy,
      hiddenCount: hidden.length,
      failedCount: hidden.filter(itemFailed).length,
    };
    const changes = summarizeTurnChanges(segment.items);
    if (changes) body.changes = changes;

    for (const item of segment.items) {
      if (item.kind === "turn_ended" && silentTurnEnd(item)) continue;
      if (hiddenIds.has(item.id) && !expanded) continue;
      entries.push({ kind: "item", item, revealed: hiddenIds.has(item.id) });
      // The fold sits right under the prompt, where the live header stood,
      // even when nothing precedes the answer and there is nothing to fold.
      if (item === segment.user) entries.push({ kind: "turn_fold", body });
    }
  }

  return { entries, workingEntryId, commentaryIds };
}

export function buildTranscriptSlots(
  messages: ChatViewItem[],
  streaming = false,
  expandedTurnIds: ReadonlySet<string> = new Set(),
): TranscriptSlot[] {
  // Request rows owned by an inline tool/workflow footer do not render twice.
  // The delegation card has no such footer, so a gated delegate call keeps
  // its standalone row.
  const mergedRequestIds = new Set<string>();
  for (const item of messages) {
    if (
      item.kind === "tool_call" &&
      item.approval_request_id &&
      !isDelegationItem(item) &&
      !isSuggestToolCall(item)
    ) {
      mergedRequestIds.add(item.approval_request_id);
    }
    if (item.kind === "workflow_run" && item.approvalRequestId) {
      mergedRequestIds.add(item.approvalRequestId);
    }
  }

  const { entries, workingEntryId, commentaryIds } = buildPresentationEntries(
    messages,
    streaming,
    expandedTurnIds,
  );
  const links = linkDelegations(messages);
  // Started suggestions render on their suggestions card, not as orphans.
  const suggested = linkSuggestedTasks(messages);
  const bodies: SlotBody[] = [];
  // Contiguous delegations share one card, whatever mix of calls and
  // orphan rows they arrive as.
  let delegations: DelegationEntry[] = [];
  const flushDelegations = () => {
    if (delegations.length === 0) return;
    bodies.push({ kind: "delegation", entries: delegations });
    delegations = [];
  };
  // One uninterrupted stretch of mechanical work. Subagent runs join the
  // stretch instead of splitting it; prose, approvals, task checklists and
  // any other visible row end it.
  let run: WorkEntry[] = [];
  let runRevealed = false;
  // App receipts anchored inside a run remain independent rows just after
  // that run. They never settle/split provider work or own its disclosure.
  let runReceipts: SlotBody[] = [];
  const flushReceipts = () => {
    bodies.push(...runReceipts);
    runReceipts = [];
  };
  let runTurnLive = false;
  const flush = () => {
    if (run.length === 0) return;
    const working =
      workingEntryId != null && run.some((entry) => entry.id === workingEntryId);
    // A lone settled Read/Grep/Glob carries no signal — drop it rather than
    // leave a one-line work-log row behind. An in-flight one still surfaces,
    // and so does one the user pulled out of an expanded turn fold.
    const only = run.length === 1 ? run[0] : null;
    if (
      !working &&
      !runRevealed &&
      only != null &&
      only.kind !== "subagent_run" &&
      isQuietObservationalTool(only)
    ) {
      run = [];
      runRevealed = false;
      flushReceipts();
      runTurnLive = false;
      return;
    }
    const hasWork = run.some(
      (entry) => entry.kind === "tool_call" || entry.kind === "subagent_run",
    );
    if (hasWork) {
      bodies.push({
        kind: "activity",
        items: run,
        working,
        turnLive: runTurnLive,
      });
    } else {
      for (const entry of run) bodies.push({ kind: "item", item: entry });
    }
    run = [];
    runRevealed = false;
    flushReceipts();
    runTurnLive = false;
  };

  for (const entry of entries) {
    if (entry.kind === "turn_fold") {
      flush();
      flushDelegations();
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
    if (isDelegationItem(item)) {
      if (item.kind === "tool_call") {
        flush();
        delegations.push({
          key: item.id,
          call: item,
          view: links.viewByCallId.get(item.id) ?? null,
        });
      } else if (item.kind === "subagent_run") {
        // Rows a call already shows render there; only orphans land here.
        // A card of claimed rows renders nothing and splits nothing.
        const orphans = item.subagents.filter(
          (view) => !links.claimed.has(view.id) && !suggested.claimed.has(view.id),
        );
        if (orphans.length === 0) continue;
        flush();
        for (const view of orphans) {
          delegations.push({ key: view.id, call: null, view });
        }
      }
      continue;
    }
    if (isSuggestToolCall(item)) {
      flush();
      flushDelegations();
      const call = item as ToolCallItem;
      bodies.push({
        kind: "suggested_tasks",
        call,
        views: viewsForCall(call, suggested.viewByKey),
      });
      continue;
    }
    // Non-error turn-ended markers were already dropped above, so they do not
    // split a stretch either.
    if (isWorkEntry(item)) {
      flushDelegations();
      run.push(item);
      if (entry.revealed) runRevealed = true;
      if (entry.turnLive) runTurnLive = true;
      continue;
    }
    if (isAppReceipt(item) && run.length > 0) {
      runReceipts.push({ kind: "item", item });
      continue;
    }
    flush();
    flushDelegations();
    bodies.push({
      kind: "item",
      item,
      commentary: commentaryIds.has(item.id),
      ...(entry.turnLive ? { turnLive: true } : {}),
    });
  }
  flush();
  flushDelegations();

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
  if (a.kind === "item" && b.kind === "item") {
    return (
      a.item === b.item &&
      a.commentary === b.commentary &&
      a.turnLive === b.turnLive
    );
  }
  if (a.kind === "turn_fold" && b.kind === "turn_fold") {
    return (
      a.turnId === b.turnId &&
      a.label === b.label &&
      a.expanded === b.expanded &&
      a.pinnedBy.length === b.pinnedBy.length &&
      a.pinnedBy.every((id, index) => id === b.pinnedBy[index]) &&
      a.hiddenCount === b.hiddenCount &&
      a.failedCount === b.failedCount &&
      a.liveSince === b.liveSince &&
      turnChangesEqual(a.changes, b.changes)
    );
  }
  if (a.kind === "activity" && b.kind === "activity") {
    return (
      a.working === b.working &&
      a.turnLive === b.turnLive &&
      a.items.length === b.items.length &&
      a.items.every((item, index) => item === b.items[index])
    );
  }
  if (a.kind === "suggested_tasks" && b.kind === "suggested_tasks") {
    return (
      a.call === b.call &&
      a.views.length === b.views.length &&
      a.views.every((view, index) => view === b.views[index])
    );
  }
  if (a.kind === "delegation" && b.kind === "delegation") {
    return (
      a.entries.length === b.entries.length &&
      a.entries.every((entry, index) => {
        const other = b.entries[index];
        return (
          entry.key === other.key &&
          entry.call === other.call &&
          entry.view === other.view
        );
      })
    );
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
  if (body.kind === "suggested_tasks") {
    const id = `suggested:${body.call.id}`;
    return { key: id, messageId: id, scrollAnchor: false };
  }
  if (body.kind === "delegation") {
    // The first entry is the hand-off that opened the card, so the key
    // holds still as later delegations join it.
    const id = `delegation:${body.entries[0].key}`;
    return { key: id, messageId: id, scrollAnchor: false };
  }
  return {
    key: body.item.id,
    messageId: body.item.id,
    scrollAnchor: body.item.kind === "user_message" && !body.item.inflight,
  };
}
