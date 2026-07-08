import type {
  ApprovalDecision,
  CompletedItem,
  ContentDelta,
  ProviderRuntimeEvent,
  SubagentSnapshot,
  WorkflowSnapshot,
} from "@/tauri/events";

import { mergeSnapshot, newSubagentView } from "./subagents";
import {
  emptyThreadState,
  type AssistantMessageItem,
  type ChatThreadState,
  type ChatViewItem,
  type PermissionRequestItem,
  type ReasoningItem,
  type SubagentRunItem,
  type SubagentView,
  type ToolCallItem,
  type UserMessageItem,
  type WorkflowRunItem,
} from "./types";
import { mergeWorkflowSnapshot, newWorkflowRunItem } from "./workflows";

/**
 * Seq offset for QUEUED user messages so they always sort to the very
 * bottom of the transcript (below the streaming assistant turn they're
 * parked behind), regardless of how many items stream in after they were
 * enqueued. The 5,000-item cap keeps real seqs far below this; among
 * themselves queued items keep FIFO order via the monotonic `nextSeq`
 * they add on top. Cleared (re-seq'd to a normal tail seq) when a queued
 * turn dispatches.
 */
const QUEUED_SEQ_BASE = 1_000_000_000;

/**
 * Wall-clock source for reasoning-block timing. Injectable so tests can
 * pin `started_at` / `duration_ms` deterministically; production uses
 * `Date.now`. Threaded through `applyEvent` / `appendUserMessage` rather
 * than read from a module global so the reducer stays a pure function of
 * (state, event, clock).
 */
export type Clock = () => number;
const defaultClock: Clock = () => Date.now();

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

/**
 * Locate the trailing reasoning item for `turnId`, mirroring
 * `findTrailingAssistant`. Scans back skipping non-reasoning rows; bails
 * once it crosses into a different turn so a prior turn's block is never
 * mistaken for the active one.
 */
function findTrailingReasoning(
  messages: ChatViewItem[],
  turnId: string,
): { index: number; item: ReasoningItem } | null {
  for (let i = messages.length - 1; i >= 0; i--) {
    const item = messages[i];
    if (item.kind !== "reasoning") continue;
    if (item.turn_id && item.turn_id !== turnId) return null;
    return { index: i, item };
  }
  return null;
}

/**
 * Seal the trailing reasoning block when it is still streaming. Called
 * before appending any NON-thinking item so a reasoning block that was
 * interrupted (a tool call, assistant text, approval, new turn, or user
 * message landing after it) flips out of its "Thinking…" state instead of
 * shimmering forever. No-op — returns the same reference — when the tail
 * is not a streaming reasoning item, so it is cheap to call unconditionally
 * at boundary sites. Computes `duration_ms` from `started_at` when known.
 */
function sealTrailingReasoningList(
  messages: ChatViewItem[],
  now: Clock,
): ChatViewItem[] {
  const lastIndex = messages.length - 1;
  const tail = messages[lastIndex];
  if (!tail || tail.kind !== "reasoning" || !tail.streaming) return messages;
  const sealed: ReasoningItem = {
    ...tail,
    streaming: false,
    duration_ms:
      tail.started_at != null
        ? Math.max(0, now() - tail.started_at)
        : tail.duration_ms,
  };
  return replaceItem(messages, lastIndex, sealed);
}

function sealTrailingReasoning(
  state: ChatThreadState,
  now: Clock,
): ChatThreadState {
  const sealed = sealTrailingReasoningList(state.messages, now);
  if (sealed === state.messages) return state;
  return { ...state, messages: sealed };
}

// ---------------------------------------------------------------------------
// Shared item-construction helpers
//
// These operate on a bare `(messages, nextSeq)` context so the main
// transcript AND a subagent's sub-transcript build items through the exact
// same tail-merge / sealing discipline (locked decision: the drill-in
// reuses every existing renderer, so its items must be built identically).
// Thread-level side effects (streaming flag, pendingRequestIds) stay in the
// event handlers that call these.
// ---------------------------------------------------------------------------

interface ListCtx {
  messages: ChatViewItem[];
  nextSeq: number;
}

function takeSeqList(ctx: ListCtx): { seq: number; ctx: ListCtx } {
  return {
    seq: ctx.nextSeq,
    ctx: { messages: ctx.messages, nextSeq: ctx.nextSeq + 1 },
  };
}

/** Append a thinking delta with the reasoning tail-merge rule. */
function appendThinkingDelta(
  ctx: ListCtx,
  text: string,
  turnId: string,
  now: Clock,
): ListCtx {
  const lastIndex = ctx.messages.length - 1;
  const tail = ctx.messages[lastIndex];
  if (
    tail &&
    tail.kind === "reasoning" &&
    tail.streaming &&
    (!tail.turn_id || tail.turn_id === turnId)
  ) {
    const next: ReasoningItem = {
      ...tail,
      turn_id: turnId,
      text: tail.text + text,
      streaming: true,
    };
    return {
      messages: replaceItem(ctx.messages, lastIndex, next),
      nextSeq: ctx.nextSeq,
    };
  }
  const { seq, ctx: c2 } = takeSeqList(ctx);
  const newReasoning: ReasoningItem = {
    kind: "reasoning",
    id: nextId("reasoning"),
    seq,
    turn_id: turnId,
    text,
    streaming: true,
    started_at: now(),
  };
  return { messages: [...c2.messages, newReasoning], nextSeq: c2.nextSeq };
}

/** Append a text delta: seal any trailing reasoning, then tail-merge into
 *  the streaming assistant message (or start a fresh one). */
function appendTextDelta(
  ctx: ListCtx,
  text: string,
  turnId: string,
  now: Clock,
): ListCtx {
  const messages = sealTrailingReasoningList(ctx.messages, now);
  const tail = messages[messages.length - 1];
  if (
    tail &&
    tail.kind === "assistant_message" &&
    tail.streaming &&
    (!tail.turn_id || tail.turn_id === turnId)
  ) {
    const next: AssistantMessageItem = {
      ...tail,
      turn_id: turnId,
      text: tail.text + text,
      streaming: true,
    };
    return {
      messages: replaceItem(messages, messages.length - 1, next),
      nextSeq: ctx.nextSeq,
    };
  }
  const { seq, ctx: c2 } = takeSeqList({ messages, nextSeq: ctx.nextSeq });
  const newAssistant: AssistantMessageItem = {
    kind: "assistant_message",
    id: nextId("assistant"),
    seq,
    turn_id: turnId,
    text,
    streaming: true,
  };
  return { messages: [...c2.messages, newAssistant], nextSeq: c2.nextSeq };
}

/** Apply a `content_delta` (text / thinking; tool_input is a no-op) to a
 *  message list. Returns the same ctx reference-equal messages when
 *  nothing rendered. */
function applyContentDeltaToList(
  ctx: ListCtx,
  delta: ContentDelta,
  turnId: string,
  now: Clock,
): ListCtx {
  if (delta.kind === "thinking") {
    return appendThinkingDelta(ctx, delta.text, turnId, now);
  }
  if (delta.kind === "text") {
    return appendTextDelta(ctx, delta.text, turnId, now);
  }
  return ctx;
}

/** Apply a completed item to a message list with the same discipline the
 *  main transcript uses (seal reasoning at non-thinking boundaries,
 *  tail-merge streaming assistant/reasoning, attach tool results). */
function applyCompletedItemToList(
  ctx: ListCtx,
  item: CompletedItem,
  turnId: string,
  now: Clock,
): ListCtx {
  switch (item.kind) {
    case "assistant_text": {
      const messages = sealTrailingReasoningList(ctx.messages, now);
      const existing = findTrailingAssistant(messages, turnId);
      if (existing && existing.item.streaming) {
        const next: AssistantMessageItem = {
          ...existing.item,
          turn_id: turnId,
          text: item.text,
          streaming: false,
        };
        return {
          messages: replaceItem(messages, existing.index, next),
          nextSeq: ctx.nextSeq,
        };
      }
      const { seq, ctx: c2 } = takeSeqList({ messages, nextSeq: ctx.nextSeq });
      const newAssistant: AssistantMessageItem = {
        kind: "assistant_message",
        id: nextId("assistant"),
        seq,
        turn_id: turnId,
        text: item.text,
        streaming: false,
      };
      return { messages: [...c2.messages, newAssistant], nextSeq: c2.nextSeq };
    }
    case "assistant_thinking": {
      const found = findTrailingReasoning(ctx.messages, turnId);
      if (found && found.item.streaming) {
        const finalized: ReasoningItem = {
          ...found.item,
          turn_id: turnId,
          text: item.text,
          streaming: false,
          duration_ms:
            found.item.started_at != null
              ? Math.max(0, now() - found.item.started_at)
              : found.item.duration_ms,
        };
        return {
          messages: replaceItem(ctx.messages, found.index, finalized),
          nextSeq: ctx.nextSeq,
        };
      }
      const { seq, ctx: c2 } = takeSeqList(ctx);
      const newReasoning: ReasoningItem = {
        kind: "reasoning",
        id: nextId("reasoning"),
        seq,
        turn_id: turnId,
        text: item.text,
        streaming: false,
      };
      return { messages: [...c2.messages, newReasoning], nextSeq: c2.nextSeq };
    }
    case "tool_use": {
      const messages = sealTrailingReasoningList(ctx.messages, now);
      if (SPECIALIZED_TOOLS.has(item.tool_name)) {
        return { messages, nextSeq: ctx.nextSeq };
      }
      const found = findToolCallByUseId(messages, item.tool_use_id);
      if (found) {
        const next: ToolCallItem = {
          ...found.item,
          tool_name: item.tool_name,
          input: item.input,
          // Result-first ordering already stamped `completed_at`; only
          // fill `started_at` if this is genuinely the first time we
          // observe the use landing.
          started_at: found.item.started_at ?? now(),
        };
        return {
          messages: replaceItem(messages, found.index, next),
          nextSeq: ctx.nextSeq,
        };
      }
      const { seq, ctx: c2 } = takeSeqList({ messages, nextSeq: ctx.nextSeq });
      const priorRequest = c2.messages.find(
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
        started_at: now(),
      };
      return { messages: [...c2.messages, newToolCall], nextSeq: c2.nextSeq };
    }
    case "tool_result": {
      const messages = sealTrailingReasoningList(ctx.messages, now);
      const found = findToolCallByUseId(messages, item.tool_use_id);
      if (found) {
        const next: ToolCallItem = {
          ...found.item,
          status: item.is_error ? "error" : "done",
          result_content: item.content,
          completed_at: now(),
        };
        return {
          messages: replaceItem(messages, found.index, next),
          nextSeq: ctx.nextSeq,
        };
      }
      const specializedPending = messages.find(
        (m): m is PermissionRequestItem =>
          m.kind === "permission_request" &&
          m.tool_use_id === item.tool_use_id &&
          SPECIALIZED_REQUEST_KINDS.has(m.request_kind),
      );
      if (specializedPending) {
        return { messages, nextSeq: ctx.nextSeq };
      }
      const { seq, ctx: c2 } = takeSeqList({ messages, nextSeq: ctx.nextSeq });
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
        completed_at: now(),
      };
      return { messages: [...c2.messages, placeholder], nextSeq: c2.nextSeq };
    }
    default: {
      warnOnce(
        `item_completed/${(item as { kind: string }).kind}`,
        item,
      );
      return ctx;
    }
  }
}

/** Hard cap on retained items inside a single subagent's sub-transcript,
 *  mirroring the main transcript cap. */
const MAX_SUBAGENT_ITEMS = 500;
const TRIM_TARGET_SUBAGENT_ITEMS = 400;

function capSubagentItems(items: ChatViewItem[]): ChatViewItem[] {
  if (items.length <= MAX_SUBAGENT_ITEMS) return items;
  return items.slice(items.length - TRIM_TARGET_SUBAGENT_ITEMS);
}

/**
 * Locate a subagent view by id, or create it. New subagents join the
 * trailing `subagent_run` card when it is the transcript tail (one card
 * per contiguous spawn group per turn); otherwise a fresh card is opened.
 * Returns the (possibly new) messages array plus the card / sub indices.
 */
function locateOrCreateSubagent(
  state: ChatThreadState,
  subagentId: string,
  turnId: string | null,
  now: Clock,
): {
  messages: ChatViewItem[];
  nextSeq: number;
  cardIndex: number;
  subIndex: number;
} {
  for (let i = state.messages.length - 1; i >= 0; i--) {
    const card = state.messages[i];
    if (card.kind !== "subagent_run") continue;
    const subIndex = card.subagents.findIndex((s) => s.id === subagentId);
    if (subIndex >= 0) {
      return {
        messages: state.messages,
        nextSeq: state.nextSeq,
        cardIndex: i,
        subIndex,
      };
    }
  }

  // A new subagent — seal any trailing reasoning, then join the tail card
  // or open a new one.
  const sealed = sealTrailingReasoningList(state.messages, now);
  const tailIndex = sealed.length - 1;
  const tail = sealed[tailIndex];
  const view = newSubagentView(subagentId, now());
  if (tail && tail.kind === "subagent_run") {
    const nextCard: SubagentRunItem = {
      ...tail,
      subagents: [...tail.subagents, view],
    };
    return {
      messages: replaceItem(sealed, tailIndex, nextCard),
      nextSeq: state.nextSeq,
      cardIndex: tailIndex,
      subIndex: nextCard.subagents.length - 1,
    };
  }
  const newCard: SubagentRunItem = {
    kind: "subagent_run",
    id: nextId("subrun"),
    seq: state.nextSeq,
    turn_id: turnId,
    subagents: [view],
  };
  return {
    messages: [...sealed, newCard],
    nextSeq: state.nextSeq + 1,
    cardIndex: sealed.length,
    subIndex: 0,
  };
}

/** Write a mutated subagent view back into its card. */
function replaceSubagent(
  messages: ChatViewItem[],
  cardIndex: number,
  subIndex: number,
  nextView: SubagentView,
): ChatViewItem[] {
  const card = messages[cardIndex];
  if (card.kind !== "subagent_run") return messages;
  const subs = card.subagents.slice();
  subs[subIndex] = nextView;
  return replaceItem(messages, cardIndex, { ...card, subagents: subs });
}

// ---------------------------------------------------------------------------
// Workflow routing
//
// A `Workflow` tool run gets its own top-level `WorkflowRunItem` (not a
// card of many — one per launch) and every subagent it spawns is routed
// into that item's phases instead of the generic `subagent_run` card.
// Attribution rides on `SubagentSnapshot.workflow_id` / `.phase`, set
// server-side (translate.rs) only while a workflow is active, so a
// subagent spawned outside a workflow is byte-identical to pre-workflow
// behavior — it never reaches this code path.
// ---------------------------------------------------------------------------

function findWorkflow(
  messages: ChatViewItem[],
  workflowId: string,
): { index: number; item: WorkflowRunItem } | null {
  for (let i = messages.length - 1; i >= 0; i--) {
    const item = messages[i];
    if (item.kind === "workflow_run" && item.workflowId === workflowId) {
      return { index: i, item };
    }
  }
  return null;
}

function findWorkflowByApprovalRequestId(
  messages: ChatViewItem[],
  requestId: string,
): { index: number; item: WorkflowRunItem } | null {
  for (let i = messages.length - 1; i >= 0; i--) {
    const item = messages[i];
    if (item.kind === "workflow_run" && item.approvalRequestId === requestId) {
      return { index: i, item };
    }
  }
  return null;
}

/** Locate a subagent living inside any workflow's phases (searched
 *  separately from `subagent_run` cards since the two containers have
 *  different shapes). */
function findSubagentInWorkflows(
  messages: ChatViewItem[],
  subagentId: string,
): { itemIndex: number; phaseIndex: number; subIndex: number } | null {
  for (let i = messages.length - 1; i >= 0; i--) {
    const item = messages[i];
    if (item.kind !== "workflow_run") continue;
    for (let p = 0; p < item.phases.length; p++) {
      const subIndex = item.phases[p].agents.findIndex((a) => a.id === subagentId);
      if (subIndex >= 0) return { itemIndex: i, phaseIndex: p, subIndex };
    }
  }
  return null;
}

/** Phase key a subagent attributes to: its own `phase` hint, else the
 *  workflow's last planned phase title, else a catch-all "Run" bucket. */
function workflowPhaseKeyFor(item: WorkflowRunItem, hint: string | null | undefined): string {
  if (hint) return hint;
  const last = item.plannedPhases[item.plannedPhases.length - 1];
  return last?.title ?? "Run";
}

/** Write a mutated subagent view back into its workflow phase. */
function replaceWorkflowSubagent(
  messages: ChatViewItem[],
  itemIndex: number,
  phaseIndex: number,
  subIndex: number,
  nextView: SubagentView,
): ChatViewItem[] {
  const item = messages[itemIndex];
  if (item.kind !== "workflow_run") return messages;
  const phase = item.phases[phaseIndex];
  const agents = phase.agents.slice();
  agents[subIndex] = nextView;
  const phases = item.phases.slice();
  phases[phaseIndex] = { ...phase, agents };
  return replaceItem(messages, itemIndex, { ...item, phases });
}

/** Merge a workflow-attributed `subagent_updated` snapshot into the
 *  matching `WorkflowRunItem`'s phases, creating both the phase bucket
 *  and the subagent view on first sight. Falls back to the generic
 *  `subagent_run` routing when the workflow item hasn't landed yet (an
 *  out-of-order snapshot) so the update is never dropped. */
function applyWorkflowSubagentUpdated(
  state: ChatThreadState,
  snap: SubagentSnapshot,
  now: Clock,
): ChatThreadState {
  const workflowId = snap.workflow_id;
  if (!workflowId) return applyGenericSubagentUpdated(state, snap, now);
  const found = findWorkflow(state.messages, workflowId);
  if (!found) return applyGenericSubagentUpdated(state, snap, now);
  const existing = findSubagentInWorkflows(state.messages, snap.subagent_id);
  if (existing) {
    const item = state.messages[existing.itemIndex];
    if (item.kind !== "workflow_run") return state;
    const sub = item.phases[existing.phaseIndex].agents[existing.subIndex];
    const nextView = mergeSnapshot(sub, snap);
    return {
      ...state,
      messages: replaceWorkflowSubagent(
        state.messages,
        existing.itemIndex,
        existing.phaseIndex,
        existing.subIndex,
        nextView,
      ),
    };
  }
  const phaseKey = workflowPhaseKeyFor(found.item, snap.phase);
  const view = mergeSnapshot(newSubagentView(snap.subagent_id, now()), snap);
  const phaseIndex = found.item.phases.findIndex((p) => p.title === phaseKey);
  const phases =
    phaseIndex >= 0
      ? found.item.phases.map((p, i) =>
          i === phaseIndex ? { ...p, agents: [...p.agents, view] } : p,
        )
      : [...found.item.phases, { title: phaseKey, detail: null, agents: [view] }];
  return {
    ...state,
    messages: replaceItem(state.messages, found.index, { ...found.item, phases }),
  };
}

/** Stop every workflow still `running`/`pending_approval` — used when
 *  the thread's turn dies with no other terminal signal coming for it. */
function stopRunningWorkflows(messages: ChatViewItem[]): ChatViewItem[] {
  return messages.map((m) => {
    if (m.kind !== "workflow_run") return m;
    if (m.status !== "running" && m.status !== "pending_approval") return m;
    return { ...m, status: "stopped" };
  });
}

/** Merge a `workflow_updated` snapshot into its item (non-null fields
 *  win, status stays monotonic), creating the item on first sight. */
function applyWorkflowUpdated(
  state: ChatThreadState,
  snap: WorkflowSnapshot,
  now: Clock,
): ChatThreadState {
  const sealed = sealTrailingReasoning(state, now);
  const found = findWorkflow(sealed.messages, snap.workflow_id);
  if (found) {
    const next = mergeWorkflowSnapshot(found.item, snap);
    return { ...sealed, messages: replaceItem(sealed.messages, found.index, next) };
  }
  const { seq, next: seqBumped } = takeSeq(sealed);
  const item = newWorkflowRunItem(nextId("workflow"), seq, now(), snap);
  return { ...seqBumped, messages: [...seqBumped.messages, item] };
}

/** Route a `subagent_id`-tagged content/item event into its subagent's
 *  sub-transcript using the shared item-builders. Checks workflow phases
 *  first (a workflow-attributed subagent's own snapshot always lands
 *  before its content), falling back to the generic `subagent_run` card
 *  path unchanged. */
function routeSubagentItem(
  state: ChatThreadState,
  subagentId: string,
  turnId: string,
  build: (ctx: ListCtx) => ListCtx,
  now: Clock,
): ChatThreadState {
  const wfLoc = findSubagentInWorkflows(state.messages, subagentId);
  if (wfLoc) {
    const item = state.messages[wfLoc.itemIndex];
    if (item.kind !== "workflow_run") return state;
    const sub = item.phases[wfLoc.phaseIndex].agents[wfLoc.subIndex];
    const ctx = build({ messages: sub.items, nextSeq: state.nextSeq });
    if (ctx.messages === sub.items && ctx.nextSeq === state.nextSeq) return state;
    const nextView: SubagentView = { ...sub, items: capSubagentItems(ctx.messages) };
    return {
      ...state,
      nextSeq: ctx.nextSeq,
      messages: replaceWorkflowSubagent(
        state.messages,
        wfLoc.itemIndex,
        wfLoc.phaseIndex,
        wfLoc.subIndex,
        nextView,
      ),
    };
  }
  const loc = locateOrCreateSubagent(state, subagentId, turnId, now);
  const card = loc.messages[loc.cardIndex];
  if (card.kind !== "subagent_run") return state;
  const sub = card.subagents[loc.subIndex];
  const ctx = build({ messages: sub.items, nextSeq: loc.nextSeq });
  if (ctx.messages === sub.items && ctx.nextSeq === loc.nextSeq) {
    // Nothing changed inside the sub-transcript; still commit any new
    // card that `locateOrCreate` may have appended.
    if (loc.messages === state.messages) return state;
    return { ...state, messages: loc.messages, nextSeq: loc.nextSeq };
  }
  const nextView: SubagentView = {
    ...sub,
    items: capSubagentItems(ctx.messages),
  };
  return {
    ...state,
    nextSeq: ctx.nextSeq,
    messages: replaceSubagent(loc.messages, loc.cardIndex, loc.subIndex, nextView),
  };
}

/** Best-effort current turn id from the trailing turn-bearing item, so a
 *  freshly-opened card records which turn spawned it. */
function trailingTurnId(messages: ChatViewItem[]): string | null {
  for (let i = messages.length - 1; i >= 0; i--) {
    const m = messages[i];
    if ("turn_id" in m && m.turn_id) return m.turn_id;
  }
  return null;
}

/** Merge a `subagent_updated` snapshot into its view (non-null fields win,
 *  status stays monotonic), creating the card / view when first seen.
 *  Snapshots carrying `workflow_id` (stamped server-side only while a
 *  workflow is active) route into that workflow's phases instead. */
function applySubagentUpdated(
  state: ChatThreadState,
  snap: SubagentSnapshot,
  now: Clock,
): ChatThreadState {
  if (snap.workflow_id) {
    return applyWorkflowSubagentUpdated(state, snap, now);
  }
  return applyGenericSubagentUpdated(state, snap, now);
}

function applyGenericSubagentUpdated(
  state: ChatThreadState,
  snap: SubagentSnapshot,
  now: Clock,
): ChatThreadState {
  const turnId = trailingTurnId(state.messages);
  const loc = locateOrCreateSubagent(state, snap.subagent_id, turnId, now);
  const card = loc.messages[loc.cardIndex];
  if (card.kind !== "subagent_run") return state;
  const sub = card.subagents[loc.subIndex];
  const nextView = mergeSnapshot(sub, snap);
  return {
    ...state,
    nextSeq: loc.nextSeq,
    messages: replaceSubagent(loc.messages, loc.cardIndex, loc.subIndex, nextView),
  };
}

function appendUserMessageLocal(
  state: ChatThreadState,
  text: string,
  now: Clock,
  clientNonce?: string,
): ChatThreadState {
  const sealed = sealTrailingReasoning(state, now);
  const { seq, next } = takeSeq(sealed);
  const item: UserMessageItem = {
    kind: "user_message",
    id: nextId("user"),
    seq,
    text,
    ...(clientNonce ? { clientNonce } : {}),
  };
  return { ...next, messages: [...next.messages, item] };
}

/**
 * Local-only action: append a user turn the composer just submitted.
 * Providers do not echo user messages back in the event stream, so the
 * UI inserts them optimistically. `clientNonce` (when provided) lets a
 * later `turn_queued` event reconcile this exact bubble instead of
 * duplicating it, and lets an error path roll it back
 * (`removeUserMessageByNonce`).
 */
export function appendUserMessage(
  state: ChatThreadState,
  text: string,
  now: Clock = defaultClock,
  clientNonce?: string,
): ChatThreadState {
  return maybeCapMessages(appendUserMessageLocal(state, text, now, clientNonce));
}

/**
 * Local-only action: roll back an optimistic user bubble by its client
 * nonce. Used when the send RPC fails outright so no orphan bubble is
 * left behind (fixes the pre-queue orphan bug). No-op when not found.
 */
export function removeUserMessageByNonce(
  state: ChatThreadState,
  clientNonce: string,
): ChatThreadState {
  const idx = state.messages.findIndex(
    (m) => m.kind === "user_message" && m.clientNonce === clientNonce,
  );
  if (idx < 0) return state;
  return { ...state, messages: state.messages.filter((_, i) => i !== idx) };
}

/** Find a queued user bubble by its backend queued id. */
function findQueuedUserMessage(
  messages: ChatViewItem[],
  queuedId: string,
): number {
  return messages.findIndex(
    (m) => m.kind === "user_message" && m.queued?.queuedId === queuedId,
  );
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
  now: Clock = defaultClock,
): ChatThreadState {
  return maybeCapMessages(applyEventInner(state, event, now));
}

function applyEventInner(
  state: ChatThreadState,
  event: ProviderRuntimeEvent,
  now: Clock,
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
      // Subagent-tagged deltas stream into that subagent's own
      // sub-transcript (built with the shared item-builders), never the
      // parent flow. The parent streaming flag is owned by
      // session_state_changed, so it isn't touched here.
      if (event.subagent_id) {
        return routeSubagentItem(
          state,
          event.subagent_id,
          event.turn_id,
          (ctx) => applyContentDeltaToList(ctx, delta, event.turn_id, now),
          now,
        );
      }
      if (delta.kind === "tool_input") {
        // tool_input deltas are not rendered in the transcript.
        return state;
      }
      // text / thinking deltas render into the parent transcript through
      // the shared builder (seal-on-boundary + tail-merge discipline);
      // both mark the turn as streaming.
      const ctx = applyContentDeltaToList(
        { messages: state.messages, nextSeq: state.nextSeq },
        delta,
        event.turn_id,
        now,
      );
      return {
        ...state,
        streaming: true,
        messages: ctx.messages,
        nextSeq: ctx.nextSeq,
      };
    }

    case "item_completed": {
      const item = event.item;
      // Subagent-tagged items reuse CompletedItem unchanged and route into
      // that subagent's sub-transcript, so the drill-in renders through
      // every existing renderer (locked decision 1).
      if (event.subagent_id) {
        return routeSubagentItem(
          state,
          event.subagent_id,
          event.turn_id,
          (ctx) => applyCompletedItemToList(ctx, item, event.turn_id, now),
          now,
        );
      }
      const ctx = applyCompletedItemToList(
        { messages: state.messages, nextSeq: state.nextSeq },
        item,
        event.turn_id,
        now,
      );
      if (ctx.messages === state.messages && ctx.nextSeq === state.nextSeq) {
        return state;
      }
      return { ...state, messages: ctx.messages, nextSeq: ctx.nextSeq };
    }

    case "subagent_updated": {
      return applySubagentUpdated(state, event.subagent, now);
    }

    case "workflow_updated": {
      return applyWorkflowUpdated(state, event.workflow, now);
    }

    case "turn_completed": {
      // A completed turn is a hard boundary — seal any trailing streaming
      // reasoning block alongside the assistant message.
      state = sealTrailingReasoning(state, now);
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
        // A workflow still running/pending-approval when its turn dies
        // has no other terminal signal coming (the sidecar tears the
        // session down) — stop it here so it doesn't spin forever.
        messages = stopRunningWorkflows(messages);
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
      // An approval prompt is a non-thinking boundary — seal any trailing
      // streaming reasoning block before the request row lands.
      state = sealTrailingReasoning(state, now);
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
        // A subagent's approval bubbles into the parent flow (locked
        // decision 4) tagged with its demux key so the UI can label it
        // "from subagent X" and the drill-in can mirror it.
        subagent_id: event.subagent_id ?? null,
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
        // A `Workflow` tool call gated on approval: link the request and
        // flip the run card to `pending_approval` so the UI can render
        // the gate inline instead of (or alongside) the standalone
        // permission row.
        const wfMatch = findWorkflow(messages, event.tool_use_id);
        if (wfMatch) {
          const patchedWf: WorkflowRunItem = {
            ...wfMatch.item,
            approvalRequestId: event.request_id,
            status: "pending_approval",
          };
          messages = replaceItem(messages, wfMatch.index, patchedWf);
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
      let messages = state.messages;
      if (found) {
        const next: PermissionRequestItem = {
          ...found.item,
          resolution: { state: "resolved", decision: event.decision },
        };
        messages = replaceItem(messages, found.index, next);
      }
      // A workflow gated on this request resumes (`running`) on allow,
      // or is considered abandoned (`stopped`) on deny/cancel — the
      // sidecar never restarts a denied Workflow tool call.
      const wfMatch = findWorkflowByApprovalRequestId(messages, event.request_id);
      if (wfMatch) {
        const decision = event.decision.decision;
        const nextStatus = decision === "allow" || decision === "allow_for_session"
          ? "running"
          : "stopped";
        // Keep `approvalRequestId` linked after resolution: the workflow
        // card owns this request's row for good (transcript-slots keeps
        // suppressing the standalone resolved block — otherwise a stray
        // "Allowed" line would reappear under the card). The approval UI
        // itself only renders while status === "pending_approval".
        const patchedWf: WorkflowRunItem = {
          ...wfMatch.item,
          status: nextStatus,
        };
        messages = replaceItem(messages, wfMatch.index, patchedWf);
      }
      if (!found && !wfMatch) return state;
      return {
        ...state,
        messages,
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

    case "turn_queued": {
      // A follow-up send parked behind the active turn. Prefer to
      // reconcile with the optimistic bubble the composer already
      // appended (matched on client_nonce) so we don't duplicate it;
      // re-seq it into the QUEUED band so it pins to the bottom below
      // the streaming turn.
      const nonce = event.client_nonce;
      const existingIdx =
        nonce != null
          ? state.messages.findIndex(
              (m) => m.kind === "user_message" && m.clientNonce === nonce,
            )
          : -1;
      if (existingIdx >= 0) {
        const existing = state.messages[existingIdx] as UserMessageItem;
        const { seq, next } = takeSeq(state);
        const updated: UserMessageItem = {
          ...existing,
          queued: { queuedId: event.queued_id },
          seq: QUEUED_SEQ_BASE + seq,
        };
        return { ...next, messages: replaceItem(next.messages, existingIdx, updated) };
      }
      // No optimistic bubble (e.g. a remounted pane that never saw the
      // send) — reconstruct the greyed item straight from the event.
      const { seq, next } = takeSeq(state);
      const item: UserMessageItem = {
        kind: "user_message",
        id: nextId("queued"),
        seq: QUEUED_SEQ_BASE + seq,
        text: event.text,
        queued: { queuedId: event.queued_id },
        ...(nonce != null ? { clientNonce: nonce } : {}),
      };
      return { ...next, messages: [...next.messages, item] };
    }

    case "queued_turn_dispatched": {
      // The queued turn is now the active turn — promote its bubble to a
      // normal user message and re-seq it to the tail so it renders at
      // its real dispatch position (after everything the prior turn
      // produced), not where it was typed.
      const idx = findQueuedUserMessage(state.messages, event.queued_id);
      if (idx < 0) return state;
      const existing = state.messages[idx] as UserMessageItem;
      const { seq, next } = takeSeq(state);
      const { queued: _dropped, ...rest } = existing;
      const promoted: UserMessageItem = { ...rest, seq };
      return { ...next, messages: replaceItem(next.messages, idx, promoted) };
    }

    case "queued_turn_cancelled": {
      // The queued turn was cancelled (by the user, or by session
      // close/error) — remove its greyed bubble.
      const idx = findQueuedUserMessage(state.messages, event.queued_id);
      if (idx < 0) return state;
      return { ...state, messages: state.messages.filter((_, i) => i !== idx) };
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
