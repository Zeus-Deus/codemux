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
  // Sticky once set: a provider that only flags the first task event for a
  // background job must not have a later unflagged snapshot promote it
  // back to "real subagent" (the same additive-merge rule as every other
  // field — a snapshot never clobbers a known value with an absent one).
  if (snap.background_task) next.backgroundTask = true;
  if (snap.parent_item_id != null) next.parentItemId = snap.parent_item_id;
  if (snap.name != null) next.name = snap.name;
  if (snap.agent_type != null) next.agentType = snap.agent_type;
  if (snap.description != null) next.description = snap.description;
  // Sticky like every other merged field: `task_progress` ticks carry no
  // `task_kind`, so a null must never un-classify a known watch loop.
  if (snap.task_kind != null) next.taskKind = snap.task_kind;
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

/** Whole-group rollup for one spawn group's settled one-line summary.
 *
 *  Every field is nullable on purpose: the settled line prints what the app
 *  actually knows and omits the rest rather than fabricating a number.
 *
 *  - `elapsedMs` is the **longest** row, not the sum. Subagents in a group
 *    run in parallel, so summing them would report several minutes of
 *    wall-clock for a run the user watched take one.
 *  - `totalTokens` is null unless at least one provider reported usage;
 *    summing a partial set would silently under-report.
 *  - `toolCount` always resolves (provider count, else counted child tool
 *    calls), so it is a plain number. */
export interface SubagentGroupRollup {
  doneCount: number;
  activeCount: number;
  elapsedMs: number | null;
  totalTokens: number | null;
  toolCount: number;
}

export function subagentGroupRollup(
  views: readonly SubagentView[],
  now: number,
): SubagentGroupRollup {
  let doneCount = 0;
  let activeCount = 0;
  let elapsedMs: number | null = null;
  let totalTokens: number | null = null;
  let toolCount = 0;

  for (const view of views) {
    if (isRunning(view)) activeCount += 1;
    if (isDone(view)) doneCount += 1;
    const elapsed = subagentElapsedMs(view, now);
    if (elapsed != null) elapsedMs = Math.max(elapsedMs ?? 0, elapsed);
    if (view.totalTokens != null) {
      totalTokens = (totalTokens ?? 0) + view.totalTokens;
    }
    toolCount += subagentToolCount(view);
  }

  return { doneCount, activeCount, elapsedMs, totalTokens, toolCount };
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

/**
 * The row's latest output, for the inline expansion under a rail row.
 *
 * Unlike {@link subagentActivityLine} — a single ellipsized line for the
 * collapsed row — this keeps the provider's full multi-line result so the
 * expansion can render it `pre-wrap`. Precedence mirrors the activity line
 * (result → provider summary → latest tool call) and returns `null` rather
 * than a placeholder, so the caller decides how to say "nothing yet".
 */
export function subagentLatestOutput(view: SubagentView): string | null {
  const result = view.resultText?.trim();
  if (result) return result;
  const activity = view.activity?.trim();
  if (activity) return activity;
  const tool = latestToolCall(view);
  if (tool) {
    const { verb, target, meta } = describeToolCall(tool);
    return [target ? `${verb} ${target}` : verb, meta]
      .filter((part) => part.length > 0)
      .join(" · ");
  }
  return null;
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

/**
 * Whether a subagent row should read as **live activity** right now.
 *
 * Running/pending is necessary but not sufficient, for two independent
 * reasons:
 *
 * - A **watch loop** (`taskKind === "monitor"`) is never agent work at
 *   all, streaming or not. Counting it would inflate "N subagents
 *   running" and keep the amber progress bar up for a thread whose only
 *   remaining activity is a CI poll — the exact thing the calm
 *   `monitoring` status exists to replace. The task still keeps its
 *   transcript card, so the user can always see what is being watched.
 * - A provider **background task** (a background shell command) can
 *   legitimately outlive the turn and never report a terminal status, so
 *   once the thread stops streaming it is a job that happens to still be
 *   alive — not the agent working. Counting it would keep the docked
 *   activity bar and its spinner up forever after the run ended,
 *   including across a restart (the snapshots are persisted and
 *   hydrate-replayed).
 *
 * The two classifications are complementary and a row can carry both:
 * `taskKind` is precise but needs the SDK to report `task_type`, while
 * `backgroundTask` is derived from the launch registry and works
 * regardless. Either one alone is enough to drop the row from the bar.
 */
export function isLiveActivity(
  view: SubagentView,
  streaming: boolean,
): boolean {
  if (!isRunning(view)) return false;
  if (isMonitorTask(view)) return false;
  return streaming || !view.backgroundTask;
}

/** Count of currently-running subagents across the whole thread — feeds
 *  the docked {@link SubagentActivityBar}. `streaming` is the thread's
 *  live-run flag; see {@link isLiveActivity}. */
export function countRunningSubagents(
  messages: ChatViewItem[],
  streaming = true,
): number {
  let n = 0;
  for (const card of subagentRunItems(messages)) {
    for (const sub of card.subagents) {
      if (sub.status === "running" && isLiveActivity(sub, streaming)) n += 1;
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

/** Whether a row is a background watch loop rather than delegated agent work.
 *  Absent `taskKind` (every provider but Claude, and Claude on an SDK that
 *  does not report task types) reads as agent work. */
export function isMonitorTask(view: SubagentView): boolean {
  return view.taskKind === "monitor";
}

/** Every currently-running subagent across every `subagent_run` card in
 *  the thread, no matter which reply spawned it (design "one bar for the
 *  whole thread"). Feeds the docked {@link SubagentActivityBar}: its
 *  `.length` is the bar's count, and each entry carries the "from"
 *  label + jump target for the expand list.
 *
 *  `streaming` is the thread's live-run flag, and watch loops never
 *  qualify at all — see {@link isLiveActivity} for both exclusions. */
export function runningSubagentEntries(
  messages: ChatViewItem[],
  streaming = true,
): RunningSubagentEntry[] {
  const cards = subagentRunItems(messages);
  const entries: RunningSubagentEntry[] = [];
  cards.forEach((card, cardIdx) => {
    for (const sub of card.subagents) {
      if (!isLiveActivity(sub, streaming)) continue;
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

// ── Spawn waves (the Subagents pane's grouping) ──

/**
 * One spawn wave: a `subagent_run` card together with the turn that
 * spawned it. The pane folds each wave to a single header row so tens of
 * finished agents read as a handful of groups instead of a growing list.
 */
export interface SubagentWave {
  /** The card id — stable React key and fold-state key. */
  id: string;
  /** Id of the user message that spawned the wave, or null when none
   *  precedes the card (a workflow-only or partially hydrated
   *  transcript). Consecutive waves sharing it came from one turn. */
  promptId: string | null;
  /** First line of that prompt, or null. The pane shows it once as a
   *  divider per turn; the wave's own title is {@link subagentWaveTitle}. */
  prompt: string | null;
  subagents: SubagentView[];
}

/**
 * Group a transcript's subagents by spawn wave, in transcript order.
 *
 * A wave's prompt is the nearest preceding `user_message` in seq order —
 * user messages carry no `turn_id`, but the transcript is sequenced, so
 * the last prompt before a card is the turn that spawned it. The store's
 * array is not seq-ordered (a queued follow-up parks at the tail with a
 * far-future seq, and the card the current turn spawns lands after it),
 * so this walks a seq-sorted copy with the transcript's comparator and
 * skips bubbles still queued: they have not spawned anything yet. A
 * subagent id reported by more than one card stays in the wave that
 * first saw it and takes the latest view, mirroring the reducer's
 * non-regressing merge. Empty cards are skipped.
 */
export function subagentWaves(messages: ChatViewItem[]): SubagentWave[] {
  const waves: SubagentWave[] = [];
  const home = new Map<string, SubagentView[]>();
  let prompt: string | null = null;
  let promptId: string | null = null;
  const ordered = messages.slice();
  ordered.sort((a, b) => a.seq - b.seq || a.id.localeCompare(b.id));
  for (const item of ordered) {
    if (item.kind === "user_message") {
      if (item.queued || item.inflight) continue;
      const line = firstLine(item.text);
      prompt = line.length > 0 ? line : null;
      promptId = prompt == null ? null : item.id;
      continue;
    }
    if (item.kind !== "subagent_run") continue;
    const subagents: SubagentView[] = [];
    for (const subagent of item.subagents) {
      const existing = home.get(subagent.id);
      if (existing) {
        const idx = existing.findIndex((s) => s.id === subagent.id);
        if (idx >= 0) existing[idx] = subagent;
        continue;
      }
      subagents.push(subagent);
      home.set(subagent.id, subagents);
    }
    if (subagents.length > 0) {
      waves.push({ id: item.id, promptId, prompt, subagents });
    }
  }
  return waves;
}

/**
 * Wave header title: what the agents were asked to do. Each agent
 * contributes its spawn description (the "3-5 word" task label), else
 * its name / type; repeats collapse to a count, so a wave reads
 * "Locate host inventory rows" or "Explore ×3 · Verify". Not the user
 * prompt: one orchestrating turn can spawn a dozen waves, and the prompt
 * would title every one of them identically.
 *
 * "Ran N subagents" only when no agent reported a label at all.
 */
export function subagentWaveTitle(wave: SubagentWave): string {
  const counts = new Map<string, number>();
  for (const s of wave.subagents) {
    const label = firstLine(s.description ?? s.name ?? s.agentType ?? "");
    if (label.length === 0) continue;
    counts.set(label, (counts.get(label) ?? 0) + 1);
  }
  if (counts.size === 0) {
    const n = wave.subagents.length;
    return `Ran ${n} subagent${n === 1 ? "" : "s"}`;
  }
  return Array.from(counts, ([label, n]) => (n > 1 ? `${label} ×${n}` : label))
    .join(" · ");
}

/**
 * Whole-wave status for the header glyph. Precedence: a failure surfaces
 * even when the wave is folded (failed > running > halted > completed),
 * so a red header is never hidden behind a green one.
 */
export function subagentWaveStatus(
  subagents: readonly SubagentView[],
): SubagentViewStatus {
  let status: SubagentViewStatus = "completed";
  let rank = 0;
  for (const subagent of subagents) {
    const r =
      subagent.status === "failed"
        ? 3
        : isRunning(subagent)
          ? 2
          : subagent.status === "stopped" || subagent.status === "interrupted"
            ? 1
            : 0;
    if (r > rank) {
      rank = r;
      status = isRunning(subagent) ? "running" : subagent.status;
    }
  }
  return status;
}

/**
 * Ordinals for repeated names inside one wave ("Explore 1", "Explore 2").
 * Unique names get null so a lone "Verify" is not suffixed with a
 * pointless "1".
 */
export function subagentOrdinals(
  subagents: readonly SubagentView[],
): Map<string, number | null> {
  const label = (s: SubagentView) => s.name ?? s.agentType ?? "Subagent";
  const counts = new Map<string, number>();
  for (const s of subagents) {
    counts.set(label(s), (counts.get(label(s)) ?? 0) + 1);
  }
  const seen = new Map<string, number>();
  const out = new Map<string, number | null>();
  for (const s of subagents) {
    const name = label(s);
    if ((counts.get(name) ?? 0) < 2) {
      out.set(s.id, null);
      continue;
    }
    const next = (seen.get(name) ?? 0) + 1;
    seen.set(name, next);
    out.set(s.id, next);
  }
  return out;
}
