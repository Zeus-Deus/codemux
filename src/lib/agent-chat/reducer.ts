import type {
  ApprovalDecision,
  CompletedItem,
  ContentDelta,
  ContextUsageSnapshot,
  ProviderRuntimeEvent,
  SubagentSnapshot,
  WorkflowSnapshot,
} from "@/tauri/events";

import { runtimeNoticeFromWarning } from "./runtime-notice";
import {
  interruptRunningSubagents,
  mergeSnapshot,
  newSubagentView,
  settleSubagentsForToolResult,
} from "./subagents";
import {
  emptyThreadState,
  type AssistantMessageItem,
  type ChatThreadState,
  type ChatViewItem,
  type PermissionRequestItem,
  type ReasoningItem,
  type RuntimeNoticeItem,
  type SubagentRunItem,
  type SubagentView,
  type ToolCallItem,
  type UserMessageImage,
  type UserMessageItem,
  type WorkflowPhaseView,
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
    // Merging an empty delta into the existing streaming tail changes
    // nothing — keep the ctx reference-stable rather than cloning the row
    // (tail is the assistant, so no reasoning was sealed above).
    if (text.length === 0) return ctx;
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
  // No streaming assistant tail to merge into. Providers (notably the
  // partial-message stream) routinely open a fresh text content block with
  // an empty first delta. Materializing an assistant_message for it would
  // render a near-blank row AND, being a non-step item, settle the live
  // Activity run mid-turn — so the transcript reads finished-but-empty while
  // the turn is still working. Drop the empty / whitespace-only delta; a
  // later non-empty delta creates the row. The guard is deterministic, so a
  // hydrate/replay of the same event sequence produces the identical item
  // count as live streaming.
  if (text.trim().length === 0) return ctx;
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
      // No streaming assistant to seal. A completion whose text is empty
      // (a turn whose only text block was empty — Layer 1 dropped its
      // deltas) must not materialize a blank settled row: there is nothing
      // to render and nothing to seal. Return the (reasoning-sealed)
      // messages so the turn still settles cleanly.
      if (item.text.length === 0) {
        return { messages, nextSeq: ctx.nextSeq };
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
 *  the thread's turn dies with no other terminal signal coming for it.
 *  Returns the SAME array reference when no workflow changed, so callers
 *  can cheaply detect a no-op (issue #153: the session-close settle path
 *  keys off reference identity to avoid a needless state churn). */
function stopRunningWorkflows(messages: ChatViewItem[]): ChatViewItem[] {
  let changed = false;
  const next: ChatViewItem[] = messages.map((m) => {
    if (m.kind !== "workflow_run") return m;
    if (m.status !== "running" && m.status !== "pending_approval") return m;
    changed = true;
    return { ...m, status: "stopped" };
  });
  return changed ? next : messages;
}

/** Settle a workflow whose spawning `Workflow` tool_use just produced a
 *  parent-scoped `tool_result` (the raw spawn result leaked through
 *  because the adapter's demux lost track — issue #153): flip the run to
 *  `completed`/`failed` and settle its still-running phase agents
 *  (`completed` on success; view-only `interrupted` on error) with
 *  `statusAssumed` so a later real snapshot can revive/confirm them.
 *  Same-ref when there is no matching in-flight workflow. */
function settleWorkflowForToolResult(
  messages: ChatViewItem[],
  toolUseId: string,
  isError: boolean,
): ChatViewItem[] {
  const found = findWorkflow(messages, toolUseId);
  if (!found) return messages;
  if (
    found.item.status !== "running" &&
    found.item.status !== "pending_approval"
  ) {
    return messages;
  }
  const agentTarget: SubagentView["status"] = isError
    ? "interrupted"
    : "completed";
  const phases: WorkflowPhaseView[] = found.item.phases.map((p) => {
    let phaseChanged = false;
    const agents: SubagentView[] = p.agents.map((a) => {
      if (a.status === "running" || a.status === "pending") {
        phaseChanged = true;
        return { ...a, status: agentTarget, statusAssumed: true };
      }
      return a;
    });
    return phaseChanged ? { ...p, agents } : p;
  });
  const nextItem: WorkflowRunItem = {
    ...found.item,
    status: isError ? "failed" : "completed",
    phases,
  };
  return replaceItem(messages, found.index, nextItem);
}

/** Store the latest context-window occupancy snapshot for the thread.
 *
 *  Latest snapshot wins wholesale — the provider re-reports the full
 *  reading every time, so there is nothing to accumulate. Two
 *  exceptions:
 *
 *  - Malformed readings are dropped. A non-finite or negative
 *    `used_tokens` would blank/NaN the meter, so we keep the previous
 *    (still-plausible) snapshot instead.
 *  - `total_processed_tokens` is a monotonic lifetime counter, but only
 *    travels on snapshots where it exceeds `used_tokens`. A later
 *    snapshot that omits it (or reports a smaller value) must not make
 *    the "Total processed" row shrink or vanish, so the larger known
 *    value is merged forward.
 */
function applyContextUsage(
  state: ChatThreadState,
  usage: ContextUsageSnapshot,
): ChatThreadState {
  const used = usage.used_tokens;
  if (typeof used !== "number" || !Number.isFinite(used) || used < 0) {
    return state;
  }
  const prevTotal = finiteOrNull(state.contextUsage?.total_processed_tokens);
  const nextTotal = finiteOrNull(usage.total_processed_tokens);
  const total =
    prevTotal !== null && (nextTotal === null || nextTotal < prevTotal)
      ? prevTotal
      : nextTotal;
  return {
    ...state,
    contextUsage: { ...usage, total_processed_tokens: total },
  };
}

function finiteOrNull(value: number | null | undefined): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
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

/** Whether a bubble carrying this correlation token is already on screen.
 *  The dedup key for every path that can deliver the same user turn twice:
 *  an optimistic append racing its own persisted row, a live fan-out
 *  racing a cursor tail read, or a `turn_queued` bubble meeting the
 *  envelope written when the turn dispatched. */
export function hasUserMessageNonce(
  state: ChatThreadState,
  nonce: string,
): boolean {
  return state.messages.some(
    (m) => m.kind === "user_message" && m.clientNonce === nonce,
  );
}

function appendUserMessageLocal(
  state: ChatThreadState,
  text: string,
  now: Clock,
  clientNonce?: string,
  images?: UserMessageImage[],
): ChatThreadState {
  let sealed = sealTrailingReasoning(state, now);
  // A new user turn while nothing is streaming is a fresh boundary —
  // mirror the backend `SubagentTracker::clear_thread` on send (issue
  // #153): interrupt any leftover running subagents and stop any leftover
  // running workflow from a PRIOR turn so they don't spin under the new
  // turn. When STREAMING (this is a queued follow-up parked behind an
  // active turn) leave them alone — that turn is still live. On hydrate
  // replay this also runs for each persisted user_message and self-
  // corrects: a later replayed `running` snapshot revives, a later
  // terminal snapshot wins by rank.
  if (!state.streaming) {
    const settled = interruptRunningSubagents(
      stopRunningWorkflows(sealed.messages),
    );
    if (settled !== sealed.messages) sealed = { ...sealed, messages: settled };
  }
  const { seq, next } = takeSeq(sealed);
  const item: UserMessageItem = {
    kind: "user_message",
    id: nextId("user"),
    seq,
    text,
    ...(clientNonce ? { clientNonce } : {}),
    // Only stamp `images` when there actually are some, so a text-only
    // turn's item stays byte-identical to the pre-images shape (keeps
    // existing snapshot-style assertions honest).
    ...(images && images.length > 0 ? { images } : {}),
  };
  // Sending a new turn (including the one-click "Continue run") clears the
  // interrupted flag: the optimistic bubble is the user resuming the run.
  return {
    ...next,
    interrupted: false,
    messages: [...next.messages, item],
  };
}

/**
 * Local-only action: append a user turn the composer just submitted.
 * Providers do not echo user messages back in the event stream, so the
 * UI inserts them optimistically. `clientNonce` (when provided) lets a
 * later `turn_queued` event reconcile this exact bubble instead of
 * duplicating it, and lets an error path roll it back
 * (`removeUserMessageByNonce`).
 *
 * `images` (when present) attaches the turn's paste/drop/picker images
 * to the bubble so they render alongside the text — `data:` URLs on an
 * optimistic send, absolute filesystem paths on a hydrate replay.
 */
export function appendUserMessage(
  state: ChatThreadState,
  text: string,
  now: Clock = defaultClock,
  clientNonce?: string,
  images?: UserMessageImage[],
): ChatThreadState {
  return maybeCapMessages(
    appendUserMessageLocal(state, text, now, clientNonce, images),
  );
}

/**
 * Local-only action: roll back an optimistic user bubble by its client
 * nonce. Used when the send RPC fails outright so no orphan bubble is
 * left behind (fixes the pre-queue orphan bug). No-op when not found.
 *
 * `restoreInterrupted` re-arms the `interrupted` flag when the rolled-back
 * send was a resume attempt on an interrupted thread: `appendUserMessage`
 * optimistically clears `interrupted` (the bubble is the user resuming), so a
 * failed send must put it back or the "Run interrupted" divider + Continue
 * chip vanish with no recovery affordance after a single failed click. The
 * caller passes the pre-append `interrupted` value so a send on a
 * never-interrupted thread never spuriously grows the affordance.
 */
export function removeUserMessageByNonce(
  state: ChatThreadState,
  clientNonce: string,
  restoreInterrupted = false,
): ChatThreadState {
  const idx = state.messages.findIndex(
    (m) => m.kind === "user_message" && m.clientNonce === clientNonce,
  );
  if (idx < 0) {
    // Bubble already gone (or never appended): still honor an interrupted
    // restore so a failed resume keeps its affordance.
    return restoreInterrupted && !state.interrupted
      ? { ...state, interrupted: true }
      : state;
  }
  const messages = state.messages.filter((_, i) => i !== idx);
  return restoreInterrupted
    ? { ...state, interrupted: true, messages }
    : { ...state, messages };
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

/** Restore a locally-responding request after a retryable IPC/provider
 * failure. Terminal `resolved`/`failed` requests are intentionally left
 * untouched so a late promise rejection can never resurrect them. */
export function markRequestPending(
  state: ChatThreadState,
  requestId: string,
): ChatThreadState {
  const found = findPermissionRequest(state.messages, requestId);
  if (!found || found.item.resolution.state !== "responding") return state;
  return {
    ...state,
    messages: replaceItem(state.messages, found.index, {
      ...found.item,
      resolution: { state: "pending" },
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
  // The stall notice is transient: ANY real event other than a fresh
  // `run_stalled` means the thread is no longer silent, so clear it up
  // front. Rebinding here (rather than clearing per-case) means switch
  // arms that return `state` untouched still carry the cleared flag.
  if (event.type !== "run_stalled" && state.stalled !== null) {
    state = { ...state, stalled: null };
  }
  switch (event.type) {
    case "run_stalled": {
      const next = { silentForSecs: event.silent_for_secs };
      if (state.stalled && state.stalled.silentForSecs === next.silentForSecs) {
        return state;
      }
      return { ...state, stalled: next };
    }

    case "user_message": {
      // The one place a user turn enters the transcript from outside the
      // composer. Two callers, deliberately sharing this case:
      //   - hydrate replay, folding a persisted `user_message` row;
      //   - the live stream, folding the backend's fan-out of that same
      //     row to every client attached to the thread.
      // Sharing matters because they are the same bytes: a client can
      // receive a turn live and then read the identical row in a tail,
      // and only one bubble may result.
      //
      // `client_nonce` is that guard. It is on the optimistic bubble the
      // sender appended, on the greyed bubble `turn_queued` reconstructs,
      // and on this envelope — so whoever already has the turn on screen
      // skips it, and whoever does not inserts it in row order.
      if (event.client_nonce && hasUserMessageNonce(state, event.client_nonce)) {
        return state;
      }
      // Persisted `images` carry absolute paths; the bubble's display
      // shape wants them under `src`, which `resolveAssetSrc` routes
      // through the asset protocol at render time.
      const images: UserMessageImage[] | undefined = event.images?.map(
        (image) => ({ src: image.path, mediaType: image.media_type }),
      );
      return appendUserMessageLocal(
        state,
        event.text,
        now,
        event.client_nonce,
        images,
      );
    }

    case "session_configured": {
      // Nothing to render in the transcript yet; the composer/footer
      // read session config via separate commands.
      return state;
    }

    case "session_state_changed": {
      const status = event.status;
      if (status.status === "running") {
        // A fresh turn started — clear any stale interrupted flag so the
        // "Run interrupted" divider / Continue chip drop immediately.
        if (state.streaming && !state.interrupted) return state;
        return { ...state, streaming: true, interrupted: false };
      }
      if (status.status === "ready") {
        if (!state.streaming) return state;
        return { ...state, streaming: false };
      }
      if (status.status === "closed" || status.status === "error") {
        // Session teardown is a hard boundary with no further terminal
        // signal coming for anything still in flight (issue #153): stop
        // running workflows and interrupt running/pending subagents so a
        // persisted transcript never resurrects a perpetual spinner.
        // Settle even when `streaming` is already false (the running flag
        // is cleared independently of subagent lifetimes).
        const settled = interruptRunningSubagents(
          stopRunningWorkflows(state.messages),
        );
        // Belt-and-braces: a death path that couldn't recover the turn id
        // still surfaces as an error while streaming. Mark the thread
        // interrupted so the Continue affordance appears even without a
        // `child_exited` `turn_completed`. Closed is a clean stop and
        // never sets it.
        const interrupted =
          status.status === "error" && state.streaming ? true : state.interrupted;
        if (
          settled === state.messages &&
          !state.streaming &&
          interrupted === state.interrupted
        ) {
          return state;
        }
        return { ...state, streaming: false, interrupted, messages: settled };
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
        // Live output means the run recovered — drop any interrupted flag.
        interrupted: false,
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
      let messages = ctx.messages;
      // A parent-scoped `tool_result` for a spawning tool settles any
      // subagent the adapter's demux lost track of (issue #153): normally
      // the Claude adapter suppresses the spawn tool_result and emits a
      // terminal snapshot, but on a sidecar restart/resume the raw
      // parent-scoped result leaks through and no terminal snapshot ever
      // arrives — leaving the row stuck "running" forever. Derive the
      // settlement here (revivable by a later real `running` snapshot for
      // Claude background tasks).
      if (item.kind === "tool_result") {
        messages = settleSubagentsForToolResult(
          messages,
          item.tool_use_id,
          item.is_error,
        );
        messages = settleWorkflowForToolResult(
          messages,
          item.tool_use_id,
          item.is_error,
        );
      }
      if (messages === state.messages && ctx.nextSeq === state.nextSeq) {
        return state;
      }
      return { ...state, messages, nextSeq: ctx.nextSeq };
    }

    case "subagent_updated": {
      return applySubagentUpdated(state, event.subagent, now);
    }

    case "workflow_updated": {
      return applyWorkflowUpdated(state, event.workflow, now);
    }

    case "context_usage_updated": {
      return applyContextUsage(state, event.usage);
    }

    case "tasks_updated": {
      return { ...state, tasks: event.tasks, tasksUpdatedAt: now() };
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
        // A synthetic child-exit completion (provider watchdog) marks the
        // run as interrupted so the transcript shows the "Run interrupted"
        // divider and the composer offers a Continue chip. A user-initiated
        // stop lands here too but with subtype "interrupted", NOT
        // "child_exited", so it never nags with the Continue affordance.
        const interrupted =
          event.status.subtype === "child_exited" ? true : state.interrupted;
        const { seq, next: seqBumped } = takeSeq({ ...state, messages });
        return {
          ...seqBumped,
          streaming: false,
          interrupted,
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
      // A clean (non-error) completion clears any lingering `interrupted`
      // flag. The stdout-reader / exit-watchdog race can persist BOTH a
      // synthetic `child_exited` completion and the real success completion
      // for the same turn; the success is the later of the two (the watchdog
      // only synthesizes while `active_turn` is still set, which the result
      // message clears), so clearing here — live AND on hydrate replay — keeps
      // a genuinely-finished run from being mislabelled "Run interrupted".
      return { ...state, streaming: false, interrupted: false, messages };
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

    case "request_response_failed": {
      const found = findPermissionRequest(state.messages, event.request_id);
      const settled: ChatThreadState = {
        ...state,
        pendingRequestIds: state.pendingRequestIds.filter(
          (id) => id !== event.request_id,
        ),
      };
      if (!found) return settled;

      // A request that already reached the terminal `resolved` state keeps
      // it. Providers conflate "unknown request" with "already answered"
      // (Codex drops the pending entry the moment it is answered), so a
      // duplicate respond — same thread open in two windows, or a retry
      // after a slow first call — reports a failure for a request the user
      // successfully answered. Overwriting would durably (this event is
      // persisted and replayed) flip an answered approval to "expired", and
      // would un-resume a workflow that `request_resolved` already set
      // running. Same reasoning as `markRequestPending`.
      if (found.item.resolution.state === "resolved") return settled;

      let messages = replaceItem(state.messages, found.index, {
        ...found.item,
        resolution: {
          state: "failed",
          reason: event.reason,
          message: event.message,
        },
      });

      // A request-linked tool/workflow cannot receive a later result once
      // its provider callback is gone. Settle those visual owners too so a
      // stale approval never leaves a spinner or pending workflow card.
      if (found.item.tool_use_id) {
        const toolMatch = findToolCallByUseId(
          messages,
          found.item.tool_use_id,
        );
        if (toolMatch && toolMatch.item.status === "running") {
          messages = replaceItem(messages, toolMatch.index, {
            ...toolMatch.item,
            status: "error",
            completed_at: now(),
          });
        }
      }
      const wfMatch = findWorkflowByApprovalRequestId(
        messages,
        event.request_id,
      );
      if (wfMatch) {
        messages = replaceItem(messages, wfMatch.index, {
          ...wfMatch.item,
          status: "stopped",
        });
      }

      return {
        ...state,
        messages,
        pendingRequestIds: state.pendingRequestIds.filter(
          (id) => id !== event.request_id,
        ),
      };
    }

    case "runtime_warning": {
      // Most runtime warnings carry SDK-lifecycle debug strings meant for
      // devtools, not the transcript. The classifier promotes only the
      // user-facing ones (provider rate-limit rejection, enumerated
      // assistant errors) to an inline notice; the rest stay console-only.
      const notice = runtimeNoticeFromWarning(
        event.message,
        event.original_payload,
      );
      if (notice == null) {
        console.warn(
          "[agent-chat]",
          event.message,
          event.original_payload ?? event,
        );
        return state;
      }
      // A notice is a non-thinking boundary — seal any trailing reasoning
      // before it lands.
      const sealed = sealTrailingReasoning(state, now);
      const { seq, next } = takeSeq(sealed);
      const item: RuntimeNoticeItem = {
        kind: "runtime_notice",
        id: nextId("notice"),
        seq,
        message: notice,
      };
      return { ...next, messages: [...next.messages, item] };
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
      // A dispatched follow-up means a live turn is starting — clear any
      // interrupted flag so the Continue chip / divider drop.
      return {
        ...next,
        interrupted: false,
        messages: replaceItem(next.messages, idx, promoted),
      };
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
