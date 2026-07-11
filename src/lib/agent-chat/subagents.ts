import type { SubagentSnapshot, SubagentStatus } from "@/tauri/events";

import type {
  ChatViewItem,
  SubagentRunItem,
  SubagentView,
  SubagentViewStatus,
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
const STATUS_RANK: Record<SubagentViewStatus, number> = {
  pending: 0,
  running: 1,
  completed: 2,
  failed: 2,
  stopped: 2,
  // View-only forced-settle state — terminal rank so a stray `running`
  // snapshot can't quietly regress it via `mergeStatus`; the revive rule
  // in `mergeSnapshot` is the ONLY path back to `running`.
  interrupted: 2,
};

/** Merge an incoming wire `status` without regressing. A `pending`
 *  update (the serde default) never overwrites a more-advanced state;
 *  anything else (running / terminal) wins. `current` may be the
 *  view-only `interrupted`; the incoming wire status is never that.
 *
 *  Defensive: a missing/unknown incoming status (the wire type says
 *  `status` is required, but replayed dev fixtures and hand-built
 *  payloads can omit it — Rust serde-defaults it to `pending`) is
 *  treated as `pending`, i.e. a no-op. Without this, an `undefined`
 *  status would leak into the view and crash `statusTone` at render. */
export function mergeStatus(
  current: SubagentViewStatus,
  incoming: SubagentStatus,
): SubagentViewStatus {
  if (incoming === "pending" || STATUS_RANK[incoming] == null) return current;
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
  // Revive rule: a real `running` snapshot un-settles a row that was
  // inferred to have finished (interrupted, or any assumed status —
  // e.g. a Claude background task re-emitting `running` via task_progress
  // after its spawn tool_result was derived as settled).
  if (
    snap.status === "running" &&
    (view.status === "interrupted" || view.statusAssumed)
  ) {
    next.status = "running";
    next.statusAssumed = false;
  } else {
    next.status = mergeStatus(view.status, snap.status);
    // A REAL terminal snapshot (completed/failed/stopped) that wins by
    // rank clears a previously-assumed flag — the settlement is now
    // authoritative. Guarded on `view.statusAssumed` so a normal (never
    // assumed) view's shape stays byte-identical (no stray `false` key).
    if (
      view.statusAssumed &&
      next.status === snap.status &&
      snap.status !== "pending"
    ) {
      next.statusAssumed = false;
    }
  }
  if (snap.parent_item_id != null) next.parentItemId = snap.parent_item_id;
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
    view.status === "stopped" ||
    view.status === "interrupted"
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
    case "interrupted":
      return "Interrupted";
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
    case "interrupted":
      return "Interrupted";
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

export function statusTone(status: SubagentViewStatus): SubagentToneClasses {
  switch (status) {
    case "running":
    case "pending":
      return {
        text: "text-status-working",
        chipBg: "bg-status-working/15 text-status-working",
        softBg: "bg-status-working/[0.08]",
        border: "border-status-working/25",
      };
    case "interrupted":
      // Muted amber — settled-but-unresolved (forced stop / left mid-run).
      // Same hue as running (status-working) but dimmed, so it reads as
      // "was working, now halted" rather than success/failure.
      return {
        text: "text-status-working/80",
        chipBg: "bg-status-working/10 text-status-working/80",
        softBg: "bg-status-working/[0.05]",
        border: "border-status-working/20",
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
 *  the docked {@link SubagentActivityBar}. */
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

/** One currently-running subagent, flattened out of its card, with enough
 *  context to render a docked-bar row and jump back to the card it came
 *  from. */
export interface RunningSubagentEntry {
  subagent: SubagentView;
  /** The `subagent_run` card id — the jump target (design "from
   *  <reply>"). */
  cardId: string;
  /** 1-based position of the card among the thread's `subagent_run`
   *  cards. */
  cardIndex: number;
  /** "task N" label for the expand-list row, or null when the thread has
   *  only one card (the design allows omitting "from" in that case —
   *  there is nothing to disambiguate). */
  fromLabel: string | null;
}

/** Every currently-running subagent across every `subagent_run` card in
 *  the thread, no matter which reply spawned it (design "one bar for the
 *  whole thread"). Feeds the docked {@link SubagentActivityBar}: its
 *  `.length` is the bar's count, and each entry carries the "from"
 *  label + jump target for the expand list. */
export function runningSubagentEntries(
  messages: ChatViewItem[],
): RunningSubagentEntry[] {
  const cards = subagentRunItems(messages);
  const entries: RunningSubagentEntry[] = [];
  cards.forEach((card, cardIdx) => {
    for (const sub of card.subagents) {
      if (!isRunning(sub)) continue;
      entries.push({
        subagent: sub,
        cardId: card.id,
        cardIndex: cardIdx + 1,
        fromLabel: cards.length > 1 ? `task ${cardIdx + 1}` : null,
      });
    }
  });
  return entries;
}

// ── Run-state settlement (issue #153) ──
//
// Force a stuck-running subagent to a terminal VIEW state when the only
// real terminal signal (a terminal `subagent_updated` snapshot) never
// arrives — a parent-scoped `tool_result` for the spawning tool, a
// session close/error, a new user turn, or a hydrate that ends mid-run.
// Every settlement stamps `statusAssumed` so a later real `running`
// snapshot can revive it (see `mergeSnapshot`) and a later real terminal
// snapshot wins by rank and clears the flag.

/** Map every `SubagentView` inside a card / workflow-phase container
 *  through `fn`, preserving reference identity for any item (and any
 *  phase) whose agents didn't change — so the transcript array stays
 *  reference-stable where nothing settled. Non-container items pass
 *  through untouched. */
function mapItemSubagents(
  item: ChatViewItem,
  fn: (sub: SubagentView) => SubagentView,
): ChatViewItem {
  if (item.kind === "subagent_run") {
    let changed = false;
    const subs = item.subagents.map((s) => {
      const n = fn(s);
      if (n !== s) changed = true;
      return n;
    });
    return changed ? { ...item, subagents: subs } : item;
  }
  if (item.kind === "workflow_run") {
    let changed = false;
    const phases = item.phases.map((p) => {
      let phaseChanged = false;
      const agents = p.agents.map((s) => {
        const n = fn(s);
        if (n !== s) phaseChanged = true;
        return n;
      });
      if (!phaseChanged) return p;
      changed = true;
      return { ...p, agents };
    });
    return changed ? { ...item, phases } : item;
  }
  return item;
}

function mapAllSubagents(
  messages: ChatViewItem[],
  fn: (sub: SubagentView) => SubagentView,
): ChatViewItem[] {
  let changed = false;
  const next = messages.map((m) => {
    const nm = mapItemSubagents(m, fn);
    if (nm !== m) changed = true;
    return nm;
  });
  return changed ? next : messages;
}

/**
 * Settle every running subagent (in `subagent_run` cards AND
 * `workflow_run` phases) whose demux key or spawning tool matches a
 * parent-scoped `tool_result`: `sub.id === toolUseId` (Claude keys on
 * the spawning tool_use_id) or `sub.parentItemId === toolUseId`. Sets
 * `completed` (or `failed` when `isError`) + `statusAssumed`. Returns the
 * SAME array reference when nothing matched.
 */
export function settleSubagentsForToolResult(
  messages: ChatViewItem[],
  toolUseId: string,
  isError: boolean,
): ChatViewItem[] {
  const target: SubagentViewStatus = isError ? "failed" : "completed";
  return mapAllSubagents(messages, (sub) => {
    if (!isRunning(sub)) return sub;
    if (sub.id !== toolUseId && sub.parentItemId !== toolUseId) return sub;
    return { ...sub, status: target, statusAssumed: true };
  });
}

/**
 * Force every running/pending subagent (in `subagent_run` cards AND
 * `workflow_run` phases) to the view-only `interrupted` state +
 * `statusAssumed`. Used on session close/error, a new user turn, and the
 * hydrate reconciliation, so a transcript that ends mid-run never renders
 * a perpetual spinner. Returns the SAME array reference when none were
 * running.
 */
export function interruptRunningSubagents(
  messages: ChatViewItem[],
): ChatViewItem[] {
  return mapAllSubagents(messages, (sub) =>
    isRunning(sub)
      ? { ...sub, status: "interrupted", statusAssumed: true }
      : sub,
  );
}
