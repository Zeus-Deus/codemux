import type { ReasoningItem, ToolCallItem } from "@/lib/agent-chat/types";

import type { ActivityStep } from "./transcript-slots";
import { computeLineDiff } from "./DiffView";
import { describeToolCall } from "./ToolCallStatus";

/**
 * Pure derivations for the Activity block's step rows and rolled-up
 * header (summary sentence, counters, duration). Kept side-effect-free
 * and exported so they can be unit-tested without rendering.
 */

export type StepStatus = "running" | "done" | "error";

export interface StepView {
  id: string;
  /** Short, fixed-width-ish verb: `read`, `grep`, `edit`, `run`, `think`. */
  verb: string;
  /** One-line, truncatable summary (path / command / thought first line). */
  summary: string;
  /** Right-aligned dim meta: `2 hits`, `+9 −1`, `ok`, `running`, `failed`. */
  meta: string;
  status: StepStatus;
}

/** tool_name → short mono verb. Falls back to the lowercased name. */
const VERB_BY_TOOL: Record<string, string> = {
  Read: "read",
  Write: "write",
  Edit: "edit",
  MultiEdit: "edit",
  NotebookEdit: "edit",
  Bash: "run",
  Glob: "glob",
  Grep: "grep",
  WebFetch: "fetch",
  WebSearch: "search",
};

function isReasoning(step: ActivityStep): step is ReasoningItem {
  return step.kind === "reasoning";
}

function firstLine(text: string): string {
  const line = text.split("\n").find((l) => l.trim().length > 0) ?? "";
  return line.trim();
}

export function stepStatus(step: ActivityStep): StepStatus {
  if (isReasoning(step)) return step.streaming ? "running" : "done";
  if (step.status === "running") return "running";
  if (step.status === "error") return "error";
  return "done";
}

/** Verb + one-line summary for a step row (and the working live line). */
export function describeStep(step: ActivityStep): { verb: string; summary: string } {
  if (isReasoning(step)) {
    return { verb: "think", summary: firstLine(step.text) };
  }
  const { target, argument } = describeToolCall(step);
  const summary = [target, argument].filter(Boolean).join(" ");
  const verb = VERB_BY_TOOL[step.tool_name] ?? step.tool_name.toLowerCase();
  return { verb, summary };
}

/** Right-aligned mono meta for a step row. */
export function stepMeta(step: ActivityStep): string {
  const status = stepStatus(step);
  if (status === "running") return "running";
  if (status === "error") return "failed";
  if (isReasoning(step)) return "";
  switch (step.tool_name) {
    case "Edit":
    case "MultiEdit":
    case "Write": {
      const counts = editCounts(step);
      return counts ? `+${counts.added} −${counts.removed}` : "done";
    }
    case "Grep": {
      const hits = grepHitCount(step.result_content);
      return hits != null ? `${hits} hit${hits === 1 ? "" : "s"}` : "done";
    }
    case "Bash":
      return "ok";
    default:
      return "done";
  }
}

export function toStepView(step: ActivityStep): StepView {
  const { verb, summary } = describeStep(step);
  return { id: step.id, verb, summary, meta: stepMeta(step), status: stepStatus(step) };
}

/** Live one-liner for the working header: the currently running step
 *  (running tool / streaming thought) if any, else the last step. */
export function deriveLiveAction(steps: ActivityStep[]): string {
  const current =
    [...steps].reverse().find((s) => stepStatus(s) === "running") ??
    steps[steps.length - 1];
  if (!current) return "Working…";
  const { verb, summary } = describeStep(current);
  return summary ? `${verb} ${summary}` : verb;
}

export interface ActivityCounts {
  total: number;
  done: number;
  running: number;
  failed: number;
}

export function deriveActivityCounts(steps: ActivityStep[]): ActivityCounts {
  let running = 0;
  let failed = 0;
  for (const s of steps) {
    const st = stepStatus(s);
    if (st === "running") running += 1;
    else if (st === "error") failed += 1;
  }
  return { total: steps.length, running, failed, done: steps.length - running - failed };
}

/** Working header counter, e.g. `7 done · 1 running` (+ `· 1 failed`). */
export function deriveWorkingCounter(steps: ActivityStep[]): string {
  const { done, running, failed } = deriveActivityCounts(steps);
  let out = `${done} done · ${running} running`;
  if (failed > 0) out += ` · ${failed} failed`;
  return out;
}

export type ToolFamily = "read" | "edit" | "command" | "web" | "other";

const FAMILY_BY_TOOL: Record<string, ToolFamily> = {
  Read: "read",
  Grep: "read",
  Glob: "read",
  WebSearch: "web",
  WebFetch: "web",
  Edit: "edit",
  MultiEdit: "edit",
  Write: "edit",
  NotebookEdit: "edit",
  Bash: "command",
};

/**
 * Settled-header summary sentence, extending ToolGroupCard's
 * `deriveGroupTitle` idiom: read-heavy → "Explored the codebase",
 * command-heavy → "Ran commands", edit-heavy → "Edited files", mixed →
 * something sensible. Reasoning-only runs never reach here (the slot
 * builder keeps them as ReasoningBlocks).
 */
export function deriveActivitySummary(steps: ActivityStep[]): string {
  let reads = 0;
  let edits = 0;
  let commands = 0;
  let webCount = 0;
  let other = 0;
  for (const s of steps) {
    if (isReasoning(s)) continue;
    switch (FAMILY_BY_TOOL[s.tool_name] ?? "other") {
      case "read":
        reads += 1;
        break;
      case "edit":
        edits += 1;
        break;
      case "command":
        commands += 1;
        break;
      case "web":
        webCount += 1;
        break;
      default:
        // `other`-family tools (Task/agent spawns, MCP tools, …) still
        // count toward the fallback tally, so an all-`other` run reports
        // its real step count instead of "Worked through 0 steps".
        other += 1;
    }
  }
  const reading = reads + webCount;
  const onlyReads = reading > 0 && edits === 0 && commands === 0;
  const onlyEdits = edits > 0 && reading === 0 && commands === 0;
  const onlyCommands = commands > 0 && reading === 0 && edits === 0;
  if (onlyReads) return "Explored the codebase";
  if (onlyEdits) return "Edited files";
  if (onlyCommands) return "Ran commands";
  if (edits > 0 && reading > 0 && commands === 0) return "Explored and edited files";
  if (commands > 0) return "Ran commands and inspected the code";
  const toolCount = reads + edits + commands + webCount + other;
  return `Worked through ${toolCount} step${toolCount === 1 ? "" : "s"}`;
}

/**
 * Rolled-up run duration in ms: earliest known start → latest known
 * completion across every step. Returns `null` when nothing is timestamped
 * or the span is under a second (hydrated transcripts replay through the
 * reducer at one instant, so their spans collapse to ~0 — omit rather than
 * show a bogus "0s"). Reasoning contributes `started_at .. started_at +
 * duration_ms`; tools contribute `started_at .. completed_at`.
 */
export function deriveActivityDurationMs(steps: ActivityStep[]): number | null {
  let minStart = Infinity;
  let maxEnd = -Infinity;
  for (const s of steps) {
    if (isReasoning(s)) {
      if (s.started_at != null) {
        minStart = Math.min(minStart, s.started_at);
        if (s.duration_ms != null) {
          maxEnd = Math.max(maxEnd, s.started_at + s.duration_ms);
        }
      }
      continue;
    }
    if (s.started_at != null) minStart = Math.min(minStart, s.started_at);
    if (s.completed_at != null) maxEnd = Math.max(maxEnd, s.completed_at);
  }
  if (!Number.isFinite(minStart) || !Number.isFinite(maxEnd)) return null;
  const span = maxEnd - minStart;
  return span >= 1000 ? span : null;
}

/** `1m 12s` / `8s`. */
export function formatActivityDuration(ms: number): string {
  const totalSeconds = Math.round(ms / 1000);
  if (totalSeconds < 60) return `${totalSeconds}s`;
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return `${minutes}m ${seconds}s`;
}

interface EditCounts {
  added: number;
  removed: number;
}

/** Added / removed line counts for an Edit-family tool, reusing the same
 *  LCS diff the inline DiffView renders. `null` when no diff-able input. */
function editCounts(item: ToolCallItem): EditCounts | null {
  const input = isRecord(item.input) ? item.input : null;
  if (!input) return null;
  let oldText: string;
  let newText: string;
  switch (item.tool_name) {
    case "Write": {
      const content = strOrNull(input.content) ?? strOrNull(input.contents);
      if (content == null) return null;
      oldText = "";
      newText = content;
      break;
    }
    case "MultiEdit": {
      const edits = Array.isArray(input.edits) ? input.edits : [];
      const olds: string[] = [];
      const news: string[] = [];
      for (const edit of edits) {
        if (!isRecord(edit)) continue;
        const o = strOrNull(edit.old_string);
        const nw = strOrNull(edit.new_string);
        if (o != null) olds.push(o);
        if (nw != null) news.push(nw);
      }
      if (olds.length === 0 && news.length === 0) return null;
      oldText = olds.join("\n");
      newText = news.join("\n");
      break;
    }
    default: {
      const o = strOrNull(input.old_string);
      const nw = strOrNull(input.new_string);
      if (o == null && nw == null) return null;
      oldText = o ?? "";
      newText = nw ?? "";
    }
  }
  const rows = computeLineDiff(oldText, newText);
  return {
    added: rows.filter((r) => r.type === "add").length,
    removed: rows.filter((r) => r.type === "remove").length,
  };
}

/** Count `path:line:` style Grep hits in a tool result, if parseable. */
function grepHitCount(result: unknown): number | null {
  const text = contentToString(result);
  if (!text) return null;
  let count = 0;
  for (const line of text.split("\n")) {
    if (/^.+?:\d+:/.test(line)) count += 1;
  }
  return count;
}

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function strOrNull(v: unknown): string | null {
  return typeof v === "string" ? v : null;
}

function contentToString(content: unknown): string {
  if (content == null) return "";
  if (typeof content === "string") return content;
  if (Array.isArray(content)) {
    return content
      .map((entry) => {
        if (typeof entry === "string") return entry;
        if (
          typeof entry === "object" &&
          entry !== null &&
          "text" in entry &&
          typeof (entry as { text: unknown }).text === "string"
        ) {
          return (entry as { text: string }).text;
        }
        return "";
      })
      .join("\n");
  }
  return "";
}
