import type { AgentChatProviderKind } from "@/tauri/types";
import type { WorkflowAttempt, WorkflowRunSnapshot, WorkflowRunStatus, WorkflowTaskSnapshot } from "@/tauri/workflows";

const PROVIDER_LABELS: Record<AgentChatProviderKind, string> = {
  claude: "Claude", codex: "Codex", hermes: "Hermes", opencode: "OpenCode", cursor: "Cursor", grok: "Grok",
};

export interface SnapshotArtifact {
  kind: "workspace_snapshot";
  run_id: string;
  attempt_id: string;
  baseline_digest: string;
  digest: string;
  changed_paths: string[];
}

/** Checks displayable metadata; the host validates the retained bytes at preview and apply. */
export function isSnapshotArtifact(value: unknown): value is SnapshotArtifact {
  if (!value || typeof value !== "object") return false;
  if (!("kind" in value) || value.kind !== "workspace_snapshot") return false;
  if (!("run_id" in value) || typeof value.run_id !== "string" || !value.run_id) return false;
  if (!("attempt_id" in value) || typeof value.attempt_id !== "string" || !value.attempt_id) return false;
  if (!("baseline_digest" in value) || typeof value.baseline_digest !== "string" || !value.baseline_digest) return false;
  if (!("digest" in value) || typeof value.digest !== "string" || !value.digest) return false;
  if (!("changed_paths" in value) || !Array.isArray(value.changed_paths)) return false;
  const paths = value.changed_paths;
  return paths.every((path: unknown) => typeof path === "string" && path.length > 0 && !path.includes("\0") && path.split("/").every((part) => part !== "" && part !== "." && part !== ".." && part !== ".git"))
    && new Set(paths).size === paths.length;
}

export function currentTaskAttempt(task: WorkflowTaskSnapshot): WorkflowAttempt | null {
  // Older snapshots omit current_attempt after settlement. Never fall back past a replacement generation.
  const attempt = task.current_attempt ?? task.attempts[task.attempts.length - 1];
  return attempt?.generation === task.generation ? attempt : null;
}

export interface CurrentFileChange {
  task: WorkflowTaskSnapshot;
  attempt: WorkflowAttempt;
  artifact: SnapshotArtifact;
}

function currentTaskFileChanges(runId: string, task: WorkflowTaskSnapshot): CurrentFileChange[] {
  const attempt = currentTaskAttempt(task);
  if (task.retired || task.status !== "succeeded" || task.spec.access !== "write" || attempt?.status !== "succeeded") return [];
  return attempt.artifacts.flatMap((artifact) => isSnapshotArtifact(artifact)
    && artifact.run_id === runId && artifact.attempt_id === attempt.id && artifact.changed_paths.length > 0
    ? [{ task, attempt, artifact }]
    : []);
}

export function currentFileChanges(run: WorkflowRunSnapshot): CurrentFileChange[] {
  return run.tasks.flatMap((task) => currentTaskFileChanges(run.id, task));
}

export interface CheckpointGroups {
  needsYou: WorkflowTaskSnapshot[];
  inFlight: WorkflowTaskSnapshot[];
  readyToReview: WorkflowTaskSnapshot[];
  settled: WorkflowTaskSnapshot[];
}

export function checkpointGroups(run: WorkflowRunSnapshot): CheckpointGroups {
  const groups: CheckpointGroups = { needsYou: [], inFlight: [], readyToReview: [], settled: [] };
  const cancelled = run.cancel_requested || run.status === "cancelled";
  const reviewIds = new Set(currentFileChanges(run).map(({ task }) => task.spec.id));
  for (const task of run.tasks) {
    // Unverified execution remains actionable, including after cancellation or retirement.
    if (task.status === "unknown") groups.needsYou.push(task);
    else if (task.retired) groups.settled.push(task);
    else if (!cancelled && (task.status === "failed" || task.status === "blocked" || (task.status === "cancelled" && task.spec.required))) groups.needsYou.push(task);
    else if (["running", "waiting", "queued", "stopping"].includes(task.status)) groups.inFlight.push(task);
    else if (!cancelled && run.spec.mode === "live" && run.spec.allow_writes && reviewIds.has(task.spec.id)) groups.readyToReview.push(task);
    else groups.settled.push(task);
  }
  return groups;
}

export type CapacityMarker = "active" | "unknown" | "unconfirmed" | "free";
export interface WorkflowCapacity {
  held: number;
  active: number;
  unknown: number;
  unconfirmed: number;
  ceiling: number;
  markers: CapacityMarker[];
  overflow: number;
}

export function workflowCapacity(run: WorkflowRunSnapshot): WorkflowCapacity {
  let active = 0;
  let unknown = 0;
  let unconfirmed = 0;
  for (const task of run.tasks) {
    const attempt = task.current_attempt;
    if (!attempt || attempt.generation !== task.generation) {
      if (["running", "stopping", "unknown"].includes(task.status)) unconfirmed += 1;
      continue;
    }
    if (attempt.status === "unknown") unknown += 1;
    else if (["dispatching", "running", "stopping"].includes(attempt.status)) active += 1;
  }
  const held = active + unknown;
  const ceiling = Math.max(0, Math.floor(run.resolved_limits.concurrency));
  // Missing admission metadata cannot prove that a remaining slot is free.
  const represented = unconfirmed ? held + unconfirmed : Math.max(held, ceiling);
  const count = Math.min(8, represented);
  const markers: CapacityMarker[] = Array.from({ length: count }, (_, index) => index < unknown ? "unknown" : index < held ? "active" : unconfirmed ? "unconfirmed" : "free");
  return { held, active, unknown, unconfirmed, ceiling, markers, overflow: Math.max(0, represented - count) };
}

export function taskRouteLabel(run: WorkflowRunSnapshot, task: WorkflowTaskSnapshot): string {
  const routeId = currentTaskAttempt(task)?.route_id ?? task.spec.route_id ?? run.spec.routes[0]?.id;
  const route = run.spec.routes.find((entry) => entry.id === routeId);
  if (!route) return "Route unavailable";
  return [PROVIDER_LABELS[route.provider], route.model, route.effort ? `${route.effort} effort` : null].filter(Boolean).join(" · ");
}

export function runStatusLabel(status: WorkflowRunStatus): string {
  const labels: Record<WorkflowRunStatus, string> = {
    running: "Running", paused: "Paused", completed: "Completed", failed: "Failed",
    cancelled: "Cancelled", stopping: "Stopping", unknown: "Needs confirmation",
  };
  return labels[status];
}

export function taskStatusLabel(task: WorkflowTaskSnapshot): string {
  if (task.status === "unknown") return "Needs confirmation";
  if (task.retired) return "Retired";
  const labels: Record<WorkflowTaskSnapshot["status"], string> = {
    queued: "Queued", running: "Running", waiting: "Waiting for tasks", succeeded: "Completed", failed: "Failed",
    cancelled: "Cancelled", blocked: "Dependency blocked", unknown: "Needs confirmation", stopping: "Stopping",
  };
  return labels[task.status];
}

export interface TaskVerification {
  resultAccepted: boolean;
  schemaAccepted: boolean;
  retainedFiles: number;
  summary: string;
}

export function taskVerification(run: WorkflowRunSnapshot, task: WorkflowTaskSnapshot): TaskVerification {
  const resultAccepted = task.status === "succeeded" && currentTaskAttempt(task)?.status === "succeeded";
  const schemaAccepted = resultAccepted && task.spec.output_schema !== undefined && task.spec.output_schema !== null;
  const retainedFiles = currentTaskFileChanges(run.id, task).reduce((count, entry) => count + entry.artifact.changed_paths.length, 0);
  const summary = schemaAccepted ? "Result accepted against its declared schema."
    : resultAccepted ? "Result accepted. No output schema was declared."
      : "No accepted result for this task generation.";
  return { resultAccepted, schemaAccepted, retainedFiles, summary };
}
