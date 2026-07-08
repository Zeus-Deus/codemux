import type { SubagentSnapshot, SubagentStatus } from "@/tauri/events";

import type {
  ChatViewItem,
  SubagentRunItem,
  SubagentView,
  ToolCallItem,
} from "./types";

/**
 * Pure subagent view helpers, shared by the reducer, the orchestration
 * card, the drill-in, and the pane header. Kept side-effect-free so the
 * snapshot merge, status precedence, and the derived elapsed / tool-count
 * / activity fallbacks can be unit-tested directly.
 */

/** Rank used to keep `status` monotonic across dribbled snapshots: a
 *  provider that re-emits the default `pending` must never regress a row
 *  that is already running or finished. */
const STATUS_RANK: Record<SubagentStatus, number> = {
  pending: 0,
  running: 1,
  completed: 2,
  failed: 2,
  stopped: 2,
};

/** Merge an incoming wire `status` without regressing. A `pending`
 *  update (the serde default) never overwrites a more-advanced state;
 *  anything else (running / terminal) wins. */
export function mergeStatus(
  current: SubagentStatus,
  incoming: SubagentStatus,
): SubagentStatus {
  if (incoming === "pending") return current;
  if (STATUS_RANK[incoming] < STATUS_RANK[current]) return current;
  return incoming;
}

/** Deterministic small hash → tone index (design tone cycle). */
export function toneIndexForId(id: string): number {
  let h = 0;
  for (let i = 0; i < id.length; i++) {
    h = (h * 31 + id.charCodeAt(i)) | 0;
  }
  return Math.abs(h) % 5;
}

/** Fresh view for a newly-seen subagent id (pre-snapshot stub). */
export function newSubagentView(id: string, startedAt: number): SubagentView {
  return {
    id,
    status: "pending",
    items: [],
    startedAt,
    toneIndex: toneIndexForId(id),
  };
}

/** Merge a wire snapshot into a view: non-null fields win, status stays
 *  monotonic. Snake_case wire names → camelCase view names. */
export function mergeSnapshot(
  view: SubagentView,
  snap: SubagentSnapshot,
): SubagentView {
  const next: SubagentView = { ...view };
  next.status = mergeStatus(view.status, snap.status);
  if (snap.name != null) next.name = snap.name;
  if (snap.agent_type != null) next.agentType = snap.agent_type;
  if (snap.model != null) next.model = snap.model;
  if (snap.activity != null) next.activity = snap.activity;
  if (snap.result_text != null) next.resultText = snap.result_text;
  if (snap.tool_use_count != null) next.toolUseCount = snap.tool_use_count;
  if (snap.total_tokens != null) next.totalTokens = snap.total_tokens;
  if (snap.duration_ms != null) next.durationMs = snap.duration_ms;
  return next;
}

export function isRunning(view: SubagentView): boolean {
  return view.status === "running" || view.status === "pending";
}

export function isDone(view: SubagentView): boolean {
  return (
    view.status === "completed" ||
    view.status === "failed" ||
    view.status === "stopped"
  );
}

/** Tool-count: provider usage when present, else count of child tool
 *  calls in the sub-transcript (the fallback Synara never built). */
export function subagentToolCount(view: SubagentView): number {
  if (view.toolUseCount != null) return view.toolUseCount;
  return view.items.filter((i) => i.kind === "tool_call").length;
}

/** Elapsed ms: provider duration when present, else derived from the
 *  first-seen timestamp against the supplied clock. */
export function subagentElapsedMs(
  view: SubagentView,
  now: number,
): number | null {
  if (view.durationMs != null) return view.durationMs;
  if (view.startedAt != null) return Math.max(0, now - view.startedAt);
  return null;
}

/** "2m 41s" style compact duration. */
export function formatElapsed(ms: number): string {
  const total = Math.max(0, Math.round(ms / 1000));
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${m}m ${s.toString().padStart(2, "0")}s`;
}

/** Mono meta line: "2m 41s · 28 tools". Omits a segment it can't derive. */
export function subagentMetaLine(view: SubagentView, now: number): string {
  const parts: string[] = [];
  const elapsed = subagentElapsedMs(view, now);
  if (elapsed != null) parts.push(formatElapsed(elapsed));
  const tools = subagentToolCount(view);
  parts.push(`${tools} ${tools === 1 ? "tool" : "tools"}`);
  return parts.join(" · ");
}

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
  TodoWrite: "todo",
};

function firstString(...values: unknown[]): string | null {
  for (const v of values) {
    if (typeof v === "string" && v.length > 0) return v;
  }
  return null;
}

/** Describe a child tool call as `verb target · meta` parts for the
 *  activity line and the inline peek rows. */
export function describeToolCall(item: ToolCallItem): {
  verb: string;
  target: string;
  meta: string;
} {
  const verb = VERB_BY_TOOL[item.tool_name] ?? item.tool_name.toLowerCase();
  const input = (item.input ?? {}) as Record<string, unknown>;
  const target =
    firstString(
      input.file_path,
      input.command,
      input.pattern,
      input.path,
      input.url,
      input.query,
    ) ?? "";

  let meta = "";
  const oldStr = input.old_string;
  const newStr = input.new_string;
  if (typeof oldStr === "string" && typeof newStr === "string") {
    const removed = oldStr.length ? oldStr.split("\n").length : 0;
    const added = newStr.length ? newStr.split("\n").length : 0;
    meta = `+${added} −${removed}`;
  } else if (item.status === "running") {
    meta = "running";
  } else if (item.status === "error") {
    meta = "error";
  } else if (item.status === "done") {
    meta = "ok";
  }
  return { verb, target, meta };
}

/** Latest child tool call (for the derived activity line + the peek). */
export function latestToolCall(view: SubagentView): ToolCallItem | null {
  for (let i = view.items.length - 1; i >= 0; i--) {
    const item = view.items[i];
    if (item.kind === "tool_call") return item;
  }
  return null;
}

/** Up-to-N most recent child tool calls (newest last), for the peek. */
export function recentToolCalls(view: SubagentView, n: number): ToolCallItem[] {
  const out: ToolCallItem[] = [];
  for (let i = view.items.length - 1; i >= 0 && out.length < n; i--) {
    const item = view.items[i];
    if (item.kind === "tool_call") out.push(item);
  }
  return out.reverse();
}

function firstLine(text: string): string {
  const line = text.split("\n").find((l) => l.trim().length > 0);
  return (line ?? text).trim();
}

/** Activity-line precedence (locked decision 6): provider summary →
 *  latest child tool call as `verb target` → status text. Done rows show
 *  the result first line or "Done". */
export function subagentActivityLine(view: SubagentView): string {
  if (isRunning(view)) {
    if (view.activity) return view.activity;
    const tool = latestToolCall(view);
    if (tool) {
      const { verb, target } = describeToolCall(tool);
      return target ? `${verb} ${target}` : verb;
    }
    return "Working…";
  }
  if (view.resultText) return firstLine(view.resultText);
  if (view.activity) return view.activity;
  switch (view.status) {
    case "completed":
      return "Done";
    case "failed":
      return "Failed";
    case "stopped":
      return "Stopped";
    default:
      return "Pending";
  }
}

/** Status label for the breadcrumb / banner. */
export function subagentStatusLabel(view: SubagentView): string {
  switch (view.status) {
    case "running":
      return "Running";
    case "pending":
      return "Starting";
    case "completed":
      return "Completed";
    case "failed":
      return "Failed";
    case "stopped":
      return "Stopped";
  }
}

/** Design-token classes keyed off status: running = status-working
 *  (amber), completed = status-open (green), failed = status-attention
 *  (red), stopped / pending = muted. No hardcoded colours. */
export interface SubagentToneClasses {
  text: string;
  chipBg: string;
  softBg: string;
  border: string;
}

export function statusTone(status: SubagentStatus): SubagentToneClasses {
  switch (status) {
    case "running":
    case "pending":
      return {
        text: "text-status-working",
        chipBg: "bg-status-working/15 text-status-working",
        softBg: "bg-status-working/[0.08]",
        border: "border-status-working/25",
      };
    case "completed":
      return {
        text: "text-status-open",
        chipBg: "bg-status-open/15 text-status-open",
        softBg: "bg-status-open/[0.08]",
        border: "border-status-open/25",
      };
    case "failed":
      return {
        text: "text-status-attention",
        chipBg: "bg-status-attention/15 text-status-attention",
        softBg: "bg-status-attention/[0.08]",
        border: "border-status-attention/25",
      };
    case "stopped":
      return {
        text: "text-muted-foreground",
        chipBg: "bg-foreground/10 text-muted-foreground",
        softBg: "bg-foreground/[0.04]",
        border: "border-border",
      };
  }
}

// ── Whole-thread lookups (used by the pane header + drill-in) ──

/** Every subagent run card in a transcript. */
export function subagentRunItems(messages: ChatViewItem[]): SubagentRunItem[] {
  return messages.filter(
    (m): m is SubagentRunItem => m.kind === "subagent_run",
  );
}

/** Count of currently-running subagents across the whole thread — feeds
 *  the pane sub-header "N subagents running" pill. */
export function countRunningSubagents(messages: ChatViewItem[]): number {
  let n = 0;
  for (const card of subagentRunItems(messages)) {
    for (const sub of card.subagents) {
      if (sub.status === "running") n += 1;
    }
  }
  return n;
}

/** Locate a subagent view by id anywhere in the transcript. */
export function findSubagentView(
  messages: ChatViewItem[],
  id: string,
): SubagentView | null {
  for (const card of subagentRunItems(messages)) {
    const sub = card.subagents.find((s) => s.id === id);
    if (sub) return sub;
  }
  return null;
}

/** 1-based ordinal of a subagent within its card (for the breadcrumb
 *  glyph). Returns 0 when not found. */
export function subagentOrdinal(messages: ChatViewItem[], id: string): number {
  for (const card of subagentRunItems(messages)) {
    const idx = card.subagents.findIndex((s) => s.id === id);
    if (idx >= 0) return idx + 1;
  }
  return 0;
}
