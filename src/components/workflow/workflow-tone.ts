import { statusTone, type SubagentToneClasses } from "@/lib/agent-chat/subagents";
import type { SubagentStatus, WorkflowRunStatus } from "@/lib/agent-chat/types";

export type { SubagentToneClasses };

/**
 * Tone helpers for the Orchestration panel. All of them funnel through
 * `statusTone` (design tokens: running = status-working/amber,
 * completed = status-open/green, failed = status-attention/red,
 * stopped/pending = muted) so the panel's colors stay identical to the
 * rest of the subagent UI — no hardcoded colors, per
 * docs/reference/DESIGN-SYSTEM.md.
 */

/** Tone for the run-level status pill. `WorkflowRunStatus` is a superset
 *  of `SubagentStatus` (adds `pending_approval`), so it can't be passed
 *  to `statusTone` directly. */
export function workflowRunTone(status: WorkflowRunStatus): SubagentToneClasses {
  switch (status) {
    case "pending_approval":
    case "running":
      return statusTone("running");
    case "completed":
      return statusTone("completed");
    case "failed":
      return statusTone("failed");
    case "stopped":
      return statusTone("stopped");
  }
}

/** Tone for a derived phase status (`workflowPhaseStatus`'s
 *  "pending" | "running" | "done" | "failed"). */
export function workflowPhaseTone(
  status: "pending" | "running" | "done" | "failed",
): SubagentToneClasses {
  switch (status) {
    case "pending":
      return statusTone("stopped");
    case "running":
      return statusTone("running");
    case "done":
      return statusTone("completed");
    case "failed":
      return statusTone("failed");
  }
}

/** Tone for one agent row's status glyph. A `pending` subagent (not yet
 *  started — the design's "queued" hollow-dot row) reads as muted rather
 *  than amber-running, so it doesn't visually compete with agents
 *  actually in flight. */
export function workflowAgentTone(status: SubagentStatus): SubagentToneClasses {
  if (status === "pending") return statusTone("stopped");
  return statusTone(status);
}

/** Tone for a `subagentFindingBadge` result ("green" | "red" | "muted"),
 *  used to tint the agent-detail header strip and Result card. */
export function findingTone(tone: "green" | "red" | "muted"): SubagentToneClasses {
  switch (tone) {
    case "green":
      return statusTone("completed");
    case "red":
      return statusTone("failed");
    case "muted":
      return statusTone("stopped");
  }
}
