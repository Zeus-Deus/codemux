import type { WorkflowSnapshot } from "@/tauri/events";

import { subagentElapsedMs } from "./subagents";
import type {
  ChatViewItem,
  SubagentView,
  WorkflowPhaseView,
  WorkflowRunItem,
  WorkflowRunStatus,
} from "./types";

/**
 * Pure `Workflow` tool run view helpers, mirroring `subagents.ts`'s role
 * for subagents: shared by the reducer (merge / creation), the workflow
 * card, and the pane header. Kept side-effect-free so status precedence,
 * phase derivation, and the stat rollups can be unit-tested directly.
 */

const WORKFLOW_STATUS_RANK: Record<WorkflowRunStatus, number> = {
  pending_approval: 0,
  running: 1,
  completed: 2,
  failed: 2,
  stopped: 2,
};

/** Merge an incoming wire status without regressing a terminal run back
 *  to `running`/`pending_approval` (e.g. a stray duplicate launch
 *  snapshot arriving after completion). Approval-linkage transitions
 *  (`request_opened` / `request_resolved`) bypass this — they set
 *  `status` directly since they represent an out-of-band decision, not a
 *  workflow snapshot. */
export function mergeWorkflowStatus(
  current: WorkflowRunStatus,
  incoming: WorkflowRunStatus,
): WorkflowRunStatus {
  if (WORKFLOW_STATUS_RANK[incoming] < WORKFLOW_STATUS_RANK[current]) return current;
  return incoming;
}

function isWorkflowRunStatus(s: string): s is WorkflowRunStatus {
  return (
    s === "pending_approval" ||
    s === "running" ||
    s === "completed" ||
    s === "failed" ||
    s === "stopped"
  );
}

/** Fresh `WorkflowRunItem` from the launch snapshot. `id`/`seq` are
 *  supplied by the reducer (the only place that owns id/seq allocation),
 *  mirroring how `newSubagentView` takes its id from the caller. */
export function newWorkflowRunItem(
  id: string,
  seq: number,
  startedAt: number,
  snap: WorkflowSnapshot,
): WorkflowRunItem {
  const plannedPhases = (snap.phases ?? []).map((p) => ({
    title: p.title,
    detail: p.detail ?? null,
  }));
  return {
    kind: "workflow_run",
    id,
    seq,
    workflowId: snap.workflow_id,
    status: isWorkflowRunStatus(snap.status) ? snap.status : "running",
    name: snap.name ?? null,
    description: snap.description ?? null,
    script: snap.script ?? null,
    plannedPhases,
    phases: plannedPhases.map((p) => ({ title: p.title, detail: p.detail, agents: [] })),
    resultText: snap.result_text ?? null,
    totalTokens: snap.total_tokens ?? null,
    agentCount: snap.agent_count ?? null,
    startedAt,
    durationMs: snap.duration_ms ?? null,
    approvalRequestId: null,
  };
}

/** Merge a wire snapshot into a view: non-null fields win, status stays
 *  monotonic. Planned phases are only ever set once (the first snapshot
 *  that carries them) so a subagent already attributed to a phase before
 *  the phases list is known is preserved rather than dropped. */
export function mergeWorkflowSnapshot(
  item: WorkflowRunItem,
  snap: WorkflowSnapshot,
): WorkflowRunItem {
  const next: WorkflowRunItem = { ...item };
  if (isWorkflowRunStatus(snap.status)) {
    next.status = mergeWorkflowStatus(item.status, snap.status);
  }
  if (snap.name != null) next.name = snap.name;
  if (snap.description != null) next.description = snap.description;
  if (snap.script != null) next.script = snap.script;
  if (snap.phases != null && snap.phases.length > 0 && item.plannedPhases.length === 0) {
    const plannedPhases = snap.phases.map((p) => ({ title: p.title, detail: p.detail ?? null }));
    next.plannedPhases = plannedPhases;
    next.phases = plannedPhases.map((p) => {
      const existing = item.phases.find((ph) => ph.title === p.title);
      return { title: p.title, detail: p.detail, agents: existing?.agents ?? [] };
    });
  }
  if (snap.result_text != null) next.resultText = snap.result_text;
  if (snap.total_tokens != null) next.totalTokens = snap.total_tokens;
  if (snap.agent_count != null) next.agentCount = snap.agent_count;
  if (snap.duration_ms != null) next.durationMs = snap.duration_ms;
  return next;
}

// ── Whole-thread lookups ──

export function workflowRunItems(messages: ChatViewItem[]): WorkflowRunItem[] {
  return messages.filter((m): m is WorkflowRunItem => m.kind === "workflow_run");
}

/** Latest workflow run still gating on approval or in flight, or `null`. */
export function activeWorkflowRun(messages: ChatViewItem[]): WorkflowRunItem | null {
  const items = workflowRunItems(messages);
  for (let i = items.length - 1; i >= 0; i--) {
    if (items[i].status === "running" || items[i].status === "pending_approval") {
      return items[i];
    }
  }
  return null;
}

/** Most recently started workflow run, regardless of status, or `null`. */
export function latestWorkflowRun(messages: ChatViewItem[]): WorkflowRunItem | null {
  const items = workflowRunItems(messages);
  return items.length > 0 ? items[items.length - 1] : null;
}

/** Count of currently-running agents across every workflow run in the
 *  thread — the workflow-side counterpart of `countRunningSubagents`.
 *  (The pane sub-header pill both used to feed was removed in favor of
 *  the docked `SubagentActivityBar`, which covers plain subagent runs;
 *  workflow surfaces track their own status via `WorkflowRunCard` /
 *  the Orchestration panel.) */
export function countRunningWorkflowAgents(messages: ChatViewItem[]): number {
  let n = 0;
  for (const item of workflowRunItems(messages)) {
    for (const phase of item.phases) {
      for (const agent of phase.agents) {
        if (agent.status === "running") n += 1;
      }
    }
  }
  return n;
}

// ── Phase / run derived stats ──

export interface WorkflowPhaseStats {
  total: number;
  running: number;
  done: number;
  failed: number;
  tokens: number;
  elapsedMs: number;
}

/** Roll up one phase's agents. `now` defaults to `Date.now()` but accepts
 *  an override for deterministic tests, same discipline as
 *  `subagentElapsedMs`. Elapsed is the span between the earliest agent's
 *  start and the latest agent's (observed-or-current) end. */
export function workflowPhaseStats(
  phase: WorkflowPhaseView,
  now: number = Date.now(),
): WorkflowPhaseStats {
  let running = 0;
  let done = 0;
  let failed = 0;
  let tokens = 0;
  let minStart: number | null = null;
  let maxEnd: number | null = null;
  for (const agent of phase.agents) {
    if (agent.status === "running" || agent.status === "pending") running += 1;
    else if (agent.status === "completed") done += 1;
    else if (agent.status === "failed" || agent.status === "stopped") failed += 1;
    if (agent.totalTokens != null) tokens += agent.totalTokens;
    if (agent.startedAt != null) {
      minStart = minStart == null ? agent.startedAt : Math.min(minStart, agent.startedAt);
      const elapsed = subagentElapsedMs(agent, now);
      if (elapsed != null) {
        const end = agent.startedAt + elapsed;
        maxEnd = maxEnd == null ? end : Math.max(maxEnd, end);
      }
    }
  }
  const elapsedMs = minStart != null && maxEnd != null ? Math.max(0, maxEnd - minStart) : 0;
  return { total: phase.agents.length, running, done, failed, tokens, elapsedMs };
}

/** Derived per-phase status: agents running → `running`; every agent
 *  terminal → `done` (or `failed` when any of them failed); no agents yet
 *  while the run is still going → `pending`; no agents ever attributed
 *  and the run has already ended → `done`/`failed` alongside the run
 *  (an empty planned phase the workflow skipped). */
export function workflowPhaseStatus(
  phase: WorkflowPhaseView,
  runStatus: WorkflowRunStatus,
): "pending" | "running" | "done" | "failed" {
  const hasRunning = phase.agents.some((a) => a.status === "running" || a.status === "pending");
  if (hasRunning) return "running";
  if (phase.agents.length > 0) {
    const allTerminal = phase.agents.every(
      (a) => a.status === "completed" || a.status === "failed" || a.status === "stopped",
    );
    if (allTerminal) {
      const hasFailed = phase.agents.some((a) => a.status === "failed");
      return hasFailed ? "failed" : "done";
    }
    return "pending";
  }
  if (runStatus === "completed" || runStatus === "failed" || runStatus === "stopped") {
    return runStatus === "failed" ? "failed" : "done";
  }
  return "pending";
}

export interface WorkflowRunStats {
  agents: number;
  tokens: number;
  elapsedMs: number;
  phasesDone: number;
  phasesTotal: number;
  currentPhaseIndex: number;
}

/** Roll up a whole workflow run. `elapsedMs` uses the provider-reported
 *  `durationMs` once the run is terminal, else `now - startedAt` (inject
 *  `now` for deterministic tests). `currentPhaseIndex` is the first
 *  `running` phase, or the count of tracked phases when none are (the
 *  run hasn't started its next phase yet, or has finished). */
export function workflowRunStats(
  item: WorkflowRunItem,
  now: number = Date.now(),
): WorkflowRunStats {
  const allAgents = item.phases.flatMap((p) => p.agents);
  const tokens =
    item.totalTokens ?? allAgents.reduce((sum, a) => sum + (a.totalTokens ?? 0), 0);
  const terminal =
    item.status === "completed" || item.status === "failed" || item.status === "stopped";
  const elapsedMs = terminal
    ? item.durationMs ?? Math.max(0, now - item.startedAt)
    : Math.max(0, now - item.startedAt);
  const phasesTotal = Math.max(item.plannedPhases.length, item.phases.length);
  const statuses = item.phases.map((p) => workflowPhaseStatus(p, item.status));
  const phasesDone = statuses.filter((s) => s === "done" || s === "failed").length;
  const runningIndex = statuses.findIndex((s) => s === "running");
  const currentPhaseIndex = runningIndex >= 0 ? runningIndex : statuses.length;
  return {
    agents: allAgents.length,
    tokens,
    elapsedMs,
    phasesDone,
    phasesTotal,
    currentPhaseIndex,
  };
}

// ── Findings badge ──

export interface FindingBadge {
  label: string;
  tone: "green" | "red" | "muted";
}

/** Best-effort "N issues" / "clean" badge parsed out of a subagent's
 *  JSON result text. Looks for a `findings` / `bugs` / `issues` array;
 *  `null` when the result isn't JSON or carries none of those keys, so
 *  callers can render nothing rather than a misleading badge. */
export function subagentFindingBadge(view: SubagentView): FindingBadge | null {
  if (!view.resultText) return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(view.resultText);
  } catch {
    return null;
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return null;
  const obj = parsed as Record<string, unknown>;
  const arr = obj.findings ?? obj.bugs ?? obj.issues;
  if (!Array.isArray(arr)) return null;
  if (arr.length === 0) return { label: "clean", tone: "green" };
  return { label: `${arr.length} issue${arr.length === 1 ? "" : "s"}`, tone: "red" };
}
