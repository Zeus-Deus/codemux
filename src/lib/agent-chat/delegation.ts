import type { SubagentSnapshot } from "@/tauri/events";
import type { AgentChatProviderKind } from "@/tauri/types";

import { lazyToolResultStub } from "./lazy-tool-result";
import { sessionProviderLabel } from "./session-mentions";
import type {
  ChatViewItem,
  SubagentRunItem,
  SubagentView,
  ToolCallItem,
} from "./types";

/**
 * Cross-provider delegation, frontend half of the shared contract.
 *
 * A Full-access Claude or Codex chat can hand a task to another installed
 * agent with the host tool `delegate_task`. The child runs as an ordinary
 * chat in its own tab; the parent sees it three ways, all parsed here:
 *
 * - the `delegate_task` tool call (input + a JSON "started" result),
 * - a `subagent_updated` row whose id is `delegate:<child thread id>`,
 *   re-emitted by the backend on every status change,
 * - persisted notification metadata for each published result. Agent-facing
 *   framing never determines how a message renders.
 *
 * Pure and side-effect-free so every parser is unit-testable. Must not
 * import `subagents.ts`, which imports this module.
 */

/** Agent-facing wake header, also used by development fixtures. */
export const DELEGATION_RESULTS_PREFIX = "[Codemux: delegated task results]";

/** `subagent_id` prefix of a delegated row — the one marker both sides use. */
const DELEGATED_ID_PREFIX = "delegate:";

/** Codex reports host tools as `dynamicToolCall` items whose input is the
 *  whole item envelope (`{ tool, arguments, … }`). */
const CODEX_DYNAMIC_TOOL = "dynamicToolCall";

const PROVIDER_KINDS: readonly string[] = [
  "claude",
  "codex",
  "cursor",
  "grok",
  "hermes",
  "opencode",
] satisfies AgentChatProviderKind[];

function isProviderKind(value: string): value is AgentChatProviderKind {
  return PROVIDER_KINDS.includes(value);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function nonEmpty(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : null;
}

function parseJson(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    return undefined;
  }
}

// ── Row identity ──

export function isDelegatedId(id: string): boolean {
  return id.startsWith(DELEGATED_ID_PREFIX);
}

/** Whether a subagent view or wire snapshot is a cross-provider delegation
 *  (owned by the backend) rather than a provider-native subagent. */
export function isDelegatedRow(
  row: Pick<SubagentView, "id"> | Pick<SubagentSnapshot, "subagent_id">,
): boolean {
  return isDelegatedId("subagent_id" in row ? row.subagent_id : row.id);
}

/** The child chat's thread id encoded in a delegated row id, else null. */
export function childThreadIdOf(id: string): string | null {
  if (!isDelegatedId(id)) return null;
  const thread = id.slice(DELEGATED_ID_PREFIX.length);
  return thread.length > 0 ? thread : null;
}

export function delegatedSubagentId(childThreadId: string): string {
  return `${DELEGATED_ID_PREFIX}${childThreadId}`;
}

/** A `subagent_run` card made only of delegated rows. The reducer never
 *  mixes delegated and native rows in one card, so this is all-or-nothing. */
export function isDelegatedRun(item: SubagentRunItem): boolean {
  return item.subagents.length > 0 && item.subagents.every(isDelegatedRow);
}

export function providerKindOf(
  value: string | null | undefined,
): AgentChatProviderKind | null {
  const kind = value?.trim().toLowerCase();
  return kind && isProviderKind(kind) ? kind : null;
}

// ── The delegate_task tool call ──

/** The model-visible name depends on each adapter's prefixing
 *  (`mcp__codemux__delegate_task` on Claude), so match the suffix. */
export function isDelegateToolName(name: string): boolean {
  return name === "delegate_task" || name === "mcp__codemux__delegate_task" || name === "codemux_mcp__codemux__delegate_task";
}

export interface DelegateCallInput {
  provider: string;
  title: string | null;
  task: string;
  model: string | null;
  effort: string | null;
}

/** The call's arguments, or null when the call is not a delegation. Needs
 *  both `provider` and `task`, so an unrelated tool that happens to share
 *  the suffix never renders as one. */
export function delegateCallInput(
  toolName: string,
  input: unknown,
): DelegateCallInput | null {
  let args = input;
  if (toolName === CODEX_DYNAMIC_TOOL) {
    if (!isRecord(input) || !isDelegateToolName(String(input.tool))) return null;
    args = typeof input.arguments === "string" ? parseJson(input.arguments) : input.arguments;
  } else if (!isDelegateToolName(toolName)) {
    return null;
  }
  if (!isRecord(args)) return null;
  const provider = nonEmpty(args.provider);
  const task = nonEmpty(args.task);
  if (!provider || !task) return null;
  return {
    provider,
    task,
    title: nonEmpty(args.title),
    model: nonEmpty(args.model),
    effort: nonEmpty(args.effort),
  };
}

/** What a `delegate_task` call returned: the child thread it started (the
 *  live row carries everything else), or the refusal sentence. */
export type DelegateResult =
  | { kind: "started"; thread: string | null; taskId?: string | null }
  | { kind: "error"; message: string };

/** Text of a tool result in any shape the adapters produce: a plain
 *  string, MCP `[{ type: "text", text }]` (bare or under `content`), the
 *  Codex item envelope's `contentItems: [{ type: "inputText", text }]`, or
 *  a lazy stub's preview. */
function resultText(content: unknown): string | null {
  if (typeof content === "string") return content;
  if (Array.isArray(content)) {
    const parts = content
      .map((part) => (isRecord(part) && typeof part.text === "string" ? part.text : null))
      .filter((part): part is string => part != null);
    return parts.length > 0 ? parts.join("\n") : null;
  }
  if (!isRecord(content)) return null;
  const stub = lazyToolResultStub(content);
  if (stub) return stub.preview;
  if (Array.isArray(content.contentItems)) return resultText(content.contentItems);
  if (Array.isArray(content.content)) return resultText(content.content);
  if (typeof content.text === "string") return content.text;
  return null;
}

function startedFrom(value: unknown): DelegateResult | null {
  if (!isRecord(value) || !isRecord(value.started)) return null;
  const taskId=nonEmpty(value.task_id) ?? nonEmpty(value.started.task_id);
  return { kind: "started", thread: nonEmpty(value.child_thread_id) ?? nonEmpty(value.started.child_thread_id) ?? nonEmpty(value.started.thread), ...(taskId ? {taskId} : {}) };
}

/**
 * Parse the `delegate_task` result. Success is JSON text
 * `{"started":{…},"note":"…"}`; a refusal is one plain sentence flagged as
 * an error (`isError` on Claude, `success: false` on Codex). Returns null
 * while there is no result, or when a non-error result is unreadable.
 */
export function parseDelegateResult(
  content: unknown,
  isError = false,
): DelegateResult | null {
  if (content == null) return null;
  const direct = startedFrom(content);
  if (direct) return direct;
  const failed =
    isError ||
    (isRecord(content) && (content.success === false || content.isError === true));
  const text = resultText(content)?.trim();
  if (!text) return failed ? { kind: "error", message: "Delegation failed." } : null;
  if (!failed) return startedFrom(parseJson(text));
  return { kind: "error", message: firstLine(text) };
}

/** Input + parsed result of one delegate call. */
type DelegateCallInfo = { input: DelegateCallInput; result: DelegateResult | null };

// Tool-call items are copy-on-write, so identity caches the parse: the slot
// builder re-runs on every streamed token and must not re-parse JSON.
const callInfoCache = new WeakMap<ToolCallItem, DelegateCallInfo | null>();

export function delegateCallInfo(item: ToolCallItem): DelegateCallInfo | null {
  if (callInfoCache.has(item)) return callInfoCache.get(item) ?? null;
  const input = delegateCallInput(item.tool_name, item.input);
  const info = input
    ? {
        input,
        result: parseDelegateResult(item.result_content, item.status === "error"),
      }
    : null;
  callInfoCache.set(item, info);
  return info;
}

export function isDelegateToolCall(item: ChatViewItem): item is ToolCallItem {
  if (item.kind !== "tool_call") return false;
  // Cheap reject before the cached parse: almost every call is something else.
  if (item.tool_name !== CODEX_DYNAMIC_TOOL && !isDelegateToolName(item.tool_name)) {
    return false;
  }
  const info = delegateCallInfo(item);
  return info !== null && info.result?.kind !== "error" && item.status !== "error";
}

// ── Linking calls to their rows ──

export interface DelegationLinks {
  /** Delegate tool-call id → the delegated row it started. */
  viewByCallId: Map<string, SubagentView>;
  /** Delegated row ids already represented by a call. The rest are
   *  orphans (their call was trimmed or never persisted) and render at
   *  their own card's position. */
  claimed: Set<string>;
}

/** Link by task, child or tool-call identity. An in-flight call without an
 * exact identity stays separate until its result arrives. */
export function linkDelegations(messages: readonly ChatViewItem[]): DelegationLinks {
  const viewByCallId = new Map<string, SubagentView>();
  const claimed = new Set<string>();
  const rows: Array<{ view: SubagentView; turn: number }> = [];
  const calls: Array<{ call: ToolCallItem; turn: number }> = [];
  // Turns split at each sent user message, like the transcript's segments.
  let turn = 0;
  for (const item of messages) {
    if (item.kind === "user_message" && !item.inflight) {
      turn += 1;
    } else if (item.kind === "subagent_run") {
      for (const view of item.subagents) {
        if (isDelegatedRow(view)) rows.push({ view, turn });
      }
    } else if (isDelegateToolCall(item)) {
      calls.push({ call: item, turn });
    }
  }
  if (rows.length === 0) return { viewByCallId, claimed };

  const link = (call: ToolCallItem, view: SubagentView | undefined) => {
    if (!view || claimed.has(view.id)) return;
    viewByCallId.set(call.id, view);
    claimed.add(view.id);
  };
  const byId = new Map(rows.map(({ view }) => [view.id, view]));
  const byTaskId = new Map(rows.flatMap(({view}) => view.taskId ? [[view.taskId, view] as const] : []));
  const byCallId = new Map(rows.flatMap(({view}) => view.parentItemId ? [[view.parentItemId, view] as const] : []));
  for (const {call} of calls) {
    const result=delegateCallInfo(call)?.result;
    if (result?.kind === "error") continue;
    const exact=byCallId.get(call.tool_use_id) ?? byCallId.get(call.id) ?? (result?.kind === "started" ? (result.taskId ? byTaskId.get(result.taskId) : undefined) ?? (result.thread ? byId.get(delegatedSubagentId(result.thread)) : undefined) : undefined);
    link(call,exact);
  }
  return { viewByCallId, claimed };
}

// ── The row the card and strip render ──

/** Pill labels, in the order a group summary lists them. */
export const DELEGATION_PHASE_LABEL = {
  starting: "Queued",
  working: "Running",
  waiting: "Waiting",
  paused: "Waiting",
  done: "Completed",
  failed: "Failed",
  stopped: "Stopped",
};

export type DelegationPhase = keyof typeof DELEGATION_PHASE_LABEL;

/** In flight: the child has not settled yet. */
export function isLivePhase(phase: DelegationPhase): boolean {
  return phase !== "done" && phase !== "failed" && phase !== "stopped";
}

export interface DelegationRow {
  provider: AgentChatProviderKind | null;
  providerLabel: string;
  title: string;
  model: string | null;
  effort: string | null;
  phase: DelegationPhase;
  /** One line: what it is doing, its report's first line, or why it ended. */
  line: string;
  childThreadId: string | null;
  /** In flight with a known child, so Stop applies. */
  live: boolean;
}

/** Strip a markdown line down to its words for a one-line excerpt. */
function plainLine(text: string): string {
  return text
    .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
    .replace(/[`*_]/g, "")
    .replace(/^\s*(?:#+|[-+>]|\d+\.)\s+/, "")
    .replace(/\s+/g, " ")
    .trim();
}

function firstLine(text: string): string {
  const line = text.split("\n").find((candidate) => plainLine(candidate).length > 0);
  return plainLine(line ?? text);
}

/** A whole report flattened to one line, so a truncated row still shows
 *  more than a lead-in like "Two notes:". */
function excerpt(text: string): string {
  return text
    .split("\n")
    .map(plainLine)
    .filter((line) => line.length > 0)
    .join(" ")
    .slice(0, 400);
}

/** Phase and line from the backend's status + activity. "Waiting for your
 *  answer…" and "Paused: …" are the contract's activity strings for a
 *  running child that is blocked on the user or a usage limit. */
function viewState(view: SubagentView): { phase: DelegationPhase; line: string } {
  const activity = view.activity?.trim() || null;
  const result = view.resultText?.trim() ? excerpt(view.resultText) : null;
  switch (view.status) {
    case "pending":
      return { phase: "starting", line: activity ?? "Starting…" };
    case "running": {
      const phase = /^waiting for your answer/i.test(activity ?? "")
        ? "waiting"
        : /^paused\b/i.test(activity ?? "")
          ? "paused"
          : "working";
      return { phase, line: activity ?? "Working in its workspace" };
    }
    case "completed":
      return { phase: "done", line: result ?? "Done" };
    case "failed":
      return { phase: "failed", line: result ?? activity ?? "Failed" };
    case "stopped":
    case "interrupted":
      return { phase: "stopped", line: result ?? activity ?? "Stopped" };
  }
}

/** Short label from a brief: its first line, capped like the handler caps
 *  titles. Only used when the model sent no title at all. */
function titleFromTask(task: string): string {
  const line = firstLine(task);
  return line.length > 60 ? `${line.slice(0, 59).trimEnd()}…` : line;
}

/**
 * One delegation, merged from whatever exists yet: the tool call alone
 * while it is starting, the live row once the backend reports it. The row
 * wins on every field it carries — it is the authoritative, persisted state.
 */
export function delegationRow(
  call: ToolCallItem | null,
  view: SubagentView | null,
): DelegationRow {
  const info = call ? delegateCallInfo(call) : null;
  const result = info?.result ?? null;
  const providerRaw = view?.agentType ?? info?.input.provider ?? null;
  const childThreadId =
    (view ? childThreadIdOf(view.id) : null) ??
    (result?.kind === "started" ? result.thread : null);

  const { phase, line } = view
    ? viewState(view)
    : result?.kind === "error" || call?.status === "error"
      ? {
          phase: "failed" as const,
          line: result?.kind === "error" ? result.message : "Delegation failed.",
        }
      : { phase: "starting" as const, line: "Starting…" };

  return {
    provider: providerKindOf(providerRaw),
    providerLabel: view?.name?.trim() || sessionProviderLabel(providerRaw ?? ""),
    title:
      view?.description?.trim() ||
      info?.input.title ||
      (info ? titleFromTask(info.input.task) : "Delegated task"),
    model: view?.model?.trim() || info?.input.model || null,
    effort: view?.effort?.trim() || info?.input.effort || null,
    phase,
    line,
    childThreadId,
    live: isLivePhase(phase) && childThreadId != null,
  };
}

/** "7s", "2m 14s", "1h 05m" — compact, for a label that ticks. */
export function formatDelegationElapsed(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  if (total < 60) return `${total}s`;
  const minutes = Math.floor(total / 60);
  if (minutes < 60) return `${minutes}m ${String(total % 60).padStart(2, "0")}s`;
  return `${Math.floor(minutes / 60)}h ${String(minutes % 60).padStart(2, "0")}m`;
}

/** "gpt-6.1-codex · high": a delegation's quiet label beside its title.
 *  Native subagents carry no effort, so theirs is the model alone. */
export function modelEffortLabel(row: {
  model?: string | null;
  effort?: string | null;
}): string | null {
  return [row.model, row.effort].filter(Boolean).join(" · ") || null;
}

/** "1 working · 1 waiting for you · 1 done", in phase order. A task still
 *  starting counts as working. */
export function delegationPhaseSummary(rows: readonly DelegationRow[]): string {
  const labels = {starting: "working",working: "working",waiting: "waiting",paused: "waiting",done: "done",failed: "failed",stopped: "stopped"};
  const counts = new Map(["working","waiting","done","failed","stopped"].map(label => [label,0]));
  for (const { phase } of rows) {
    const label = labels[phase];
    counts.set(label, (counts.get(label) ?? 0) + 1);
  }
  return [...counts]
    .filter(([, count]) => count > 0)
    .map(([label, count]) => `${count} ${label.toLowerCase()}`)
    .join(" · ");
}
