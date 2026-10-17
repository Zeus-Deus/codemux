import type { FollowUpTaskRow } from "@/tauri/commands";
import type { AgentChatProviderKind, ChatModelInfo } from "@/tauri/types";

import {
  delegationRow,
  isDelegatedRow,
  isLivePhase,
  providerKindOf,
  type DelegationPhase,
} from "./delegation";
import type { ChatViewItem, SubagentView, ToolCallItem } from "./types";

/**
 * Suggested follow-up tasks, frontend half of the contract.
 *
 * A Claude or Codex chat calls the host tool `suggest_tasks` when it notices
 * work outside the current request. Nothing starts: the call's input is the
 * list of tasks, and the transcript renders it as a card. A task the user
 * starts runs as its own workspace on its own git worktree; the backend
 * reports it on the parent like a delegation (`delegate:<child>` row) with
 * `parent_item_id = "<tool_use_id>#<index>"`, and keeps a durable row per
 * started task (`follow_up_list`).
 *
 * Pure and side-effect-free so every parser is unit-testable.
 */

/** Claude sees host tools as `mcp__codemux__suggest_tasks` (prefixed again
 *  by the SDK's virtual server), Codex as `codemux_mcp__codemux__…`; all end
 *  in the server-scoped name. Another server's `suggest_tasks` does not match. */
const TOOL_NAME = /(?:^|__)codemux__suggest_tasks$/;
const CODEX_ENVELOPES = new Set(["dynamicToolCall", "mcpToolCall"]);

export const MAX_SUGGESTED_TASKS = 5;

export interface SuggestedTask {
  title: string;
  task: string;
  /** The agent's suggested default, when it named a known one. */
  provider: AgentChatProviderKind | null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function text(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

function toolCall(item: ToolCallItem): { name: string; args: unknown } {
  const input = item.input;
  if (CODEX_ENVELOPES.has(item.tool_name) && isRecord(input) && typeof input.tool === "string") {
    let args = input.arguments;
    if (typeof args === "string") {
      try {
        args = JSON.parse(args);
      } catch {
        args = null;
      }
    }
    return { name: input.tool, args };
  }
  return { name: item.tool_name, args: input };
}

export function isSuggestToolName(name: string): boolean {
  return TOOL_NAME.test(name);
}

/** The tasks a call suggests, or null for any other tool. Mirrors the
 *  backend's validation loosely: a task needs a brief; a missing title
 *  falls back to the brief's first line. */
export function suggestedTasksOf(item: ToolCallItem): SuggestedTask[] | null {
  const { name, args } = toolCall(item);
  if (!isSuggestToolName(name) || !isRecord(args) || !Array.isArray(args.tasks)) return null;
  const tasks: SuggestedTask[] = [];
  for (const entry of args.tasks.slice(0, MAX_SUGGESTED_TASKS)) {
    if (!isRecord(entry)) continue;
    const task = text(entry.task);
    if (!task) continue;
    const title =
      text(entry.title).replace(/\s+/g, " ") || task.split("\n")[0].trim().slice(0, 60);
    tasks.push({ title, task, provider: providerKindOf(text(entry.provider) || null) });
  }
  return tasks;
}

// Tool-call items are copy-on-write; cache the parse by identity.
const tasksCache = new WeakMap<ToolCallItem, SuggestedTask[] | null>();

export function cachedSuggestedTasks(item: ToolCallItem): SuggestedTask[] | null {
  if (tasksCache.has(item)) return tasksCache.get(item) ?? null;
  const tasks = suggestedTasksOf(item);
  tasksCache.set(item, tasks);
  return tasks;
}

/** A `suggest_tasks` call the card can show: it parsed, and the backend did
 *  not refuse it. A refused call stays an ordinary (error) tool row. */
export function isSuggestToolCall(item: ChatViewItem): item is ToolCallItem {
  if (item.kind !== "tool_call" || item.status === "error") return false;
  const name = item.tool_name;
  if (!CODEX_ENVELOPES.has(name) && !isSuggestToolName(name)) return false;
  const tasks = cachedSuggestedTasks(item);
  return tasks != null && tasks.length > 0;
}

/** `parent_item_id` of the row a started suggestion fills. */
export function suggestionKey(toolUseId: string, index: number): string {
  return `${toolUseId}#${index}`;
}

/**
 * Started suggestions' live rows, keyed by `suggestionKey`, plus the row ids
 * they claim so the transcript does not show them again as orphan
 * delegation cards. Only rows of calls present in `messages` are claimed;
 * a row whose call was trimmed still renders on its own.
 */
export function linkSuggestedTasks(messages: readonly ChatViewItem[]): {
  viewByKey: Map<string, SubagentView>;
  claimed: Set<string>;
} {
  const viewByKey = new Map<string, SubagentView>();
  const claimed = new Set<string>();
  const calls = new Set<string>();
  for (const item of messages) {
    if (isSuggestToolCall(item)) calls.add(item.tool_use_id);
  }
  if (calls.size === 0) return { viewByKey, claimed };
  for (const item of messages) {
    if (item.kind !== "subagent_run") continue;
    for (const view of item.subagents) {
      const key = view.parentItemId;
      if (!key || !isDelegatedRow(view)) continue;
      const hash = key.lastIndexOf("#");
      if (hash <= 0 || !calls.has(key.slice(0, hash))) continue;
      viewByKey.set(key, view);
      claimed.add(view.id);
    }
  }
  return { viewByKey, claimed };
}

/** The views for one call's suggestions, by index. */
export function viewsForCall(
  call: ToolCallItem,
  viewByKey: ReadonlyMap<string, SubagentView>,
): Array<SubagentView | null> {
  const tasks = cachedSuggestedTasks(call) ?? [];
  return tasks.map((_, index) => viewByKey.get(suggestionKey(call.tool_use_id, index)) ?? null);
}

// ── What a task runs on ──

/** Provider, model and effort a suggestion starts with. `effort` null means
 *  "the model's default" until {@link resolveFollowUpEffort} pins it. */
export interface FollowUpRun {
  provider: AgentChatProviderKind;
  model: string | null;
  effort: string | null;
}

/**
 * The run a not-yet-started suggestion defaults to. The parent chat's own
 * provider, model and effort, so a follow-up runs like the chat it came
 * from; a suggestion that names another provider gets that provider's
 * default model and effort instead. Providers the backend cannot run
 * (`allowed`) are skipped.
 */
export function defaultFollowUpRun(input: {
  task: SuggestedTask;
  allowed: readonly AgentChatProviderKind[];
  chatProvider: AgentChatProviderKind | null;
  chatModel: string | null;
  chatEffort: string | null;
  defaultModel: (provider: AgentChatProviderKind) => string | null;
}): FollowUpRun {
  const { task, allowed, chatProvider, chatModel, chatEffort, defaultModel } = input;
  const canRun = (kind: AgentChatProviderKind | null): kind is AgentChatProviderKind =>
    kind != null && allowed.includes(kind);
  const fresh = (provider: AgentChatProviderKind): FollowUpRun => ({
    provider,
    model: defaultModel(provider),
    effort: null,
  });
  if (canRun(task.provider) && task.provider !== chatProvider) return fresh(task.provider);
  if (canRun(chatProvider)) {
    return { provider: chatProvider, model: chatModel ?? defaultModel(chatProvider), effort: chatEffort };
  }
  return fresh(allowed[0] ?? "claude");
}

/** The effort to send: an explicit choice the model accepts, otherwise the
 *  model's default. Kept as-is while the model's catalog is unknown. */
export function resolveFollowUpEffort(
  model: ChatModelInfo | null,
  effort: string | null,
): string | null {
  if (!model) return effort;
  if (effort && model.effort_levels.includes(effort)) return effort;
  return model.default_effort ?? null;
}

// ── A started task's row ──

export interface StartedTaskState {
  phase: DelegationPhase;
  /** The agent, model and effort it runs on, once the backend reported
   *  them. Model and effort are null when it runs on the defaults. */
  provider: AgentChatProviderKind | null;
  model: string | null;
  effort: string | null;
  /** Status line: what it is doing, its report's first line, or why it ended. */
  line: string;
  childThreadId: string | null;
  workspaceId: string | null;
  branch: string | null;
  /** "+12 −3" once the task finished and the branch was measured. */
  diff: string | null;
  merged: boolean;
  /** Finished with work left uncommitted in its worktree. */
  uncommitted: number;
  live: boolean;
}

/** Phase from the durable row alone, before the live card row arrives:
 *  a running row with no card yet is still starting its workspace. */
const OUTCOME_PHASE: Record<string, DelegationPhase> = {
  running: "starting",
  completed: "done",
  failed: "failed",
  stopped: "stopped",
};

/** Merge the durable row (always there once started) with the live card
 *  row (once the backend reported it). The live row wins on phase. */
export function startedTaskState(
  row: FollowUpTaskRow | null,
  view: SubagentView | null,
): StartedTaskState | null {
  if (!row && !view) return null;
  const live = view ? delegationRow(null, view) : null;
  const phase: DelegationPhase =
    live?.phase ?? (row ? (OUTCOME_PHASE[row.outcome] ?? "starting") : "starting");
  const line =
    live?.line ??
    (phase === "starting"
      ? "Starting its workspace…"
      : phase === "done"
        ? "Done"
        : phase === "failed"
          ? "Failed"
          : "Stopped");
  const diff =
    row?.additions != null && row.deletions != null
      ? `+${row.additions} −${row.deletions}`
      : null;
  return {
    phase,
    provider: live?.provider ?? providerKindOf(row?.provider ?? null),
    model: live?.model ?? null,
    effort: live?.effort ?? null,
    line,
    childThreadId: row?.child_thread_id ?? live?.childThreadId ?? null,
    workspaceId: row?.workspace_id || null,
    branch: row?.branch ?? null,
    diff,
    merged: row?.merged ?? false,
    uncommitted: row?.uncommitted ?? 0,
    live: isLivePhase(phase),
  };
}

/** Rows Merge all would take: finished, not merged yet. */
export function mergeableCount(rows: readonly FollowUpTaskRow[]): number {
  return rows.filter((row) => row.outcome === "completed" && !row.merged).length;
}

/** The user turn "Do it here" sends: the full brief for the model, a short
 *  line for the transcript (the card above already shows the brief). */
export function doItHereTurn(task: SuggestedTask): { text: string; displayText: string } {
  return {
    text: `Please do this suggested follow-up task now, here in this chat:\n\n${task.title}\n\n${task.task}`,
    displayText: `Do the suggested task here: ${task.title}`,
  };
}

/** One readable sentence for a Merge all result. */
export function mergeSummary(result: {
  target_branch: string | null;
  merged: Array<{ title: string }>;
  skipped: Array<{ title: string; reason: string }>;
  conflict: { title: string; branch: string; files: string[]; message: string | null } | null;
}): { text: string; failed: boolean } {
  const into = result.target_branch ? ` into ${result.target_branch}` : "";
  const n = result.merged.length;
  const merged = n > 0 ? `Merged ${n} branch${n === 1 ? "" : "es"}${into}.` : "";
  if (result.conflict) {
    const { title, branch, files, message } = result.conflict;
    const what = files.length > 0 ? `conflicts in ${files.join(", ")}` : (message ?? "git refused");
    return {
      text: [
        merged,
        `Stopped at “${title}” (${branch}): ${what}. That merge was aborted; resolve it yourself or ask the agent.`,
      ]
        .filter(Boolean)
        .join(" "),
      failed: true,
    };
  }
  const skipped = result.skipped.map((skip) => `“${skip.title}”: ${skip.reason}`).join(" ");
  return {
    text: [merged || "Nothing was merged.", skipped].filter(Boolean).join(" "),
    failed: n === 0,
  };
}
