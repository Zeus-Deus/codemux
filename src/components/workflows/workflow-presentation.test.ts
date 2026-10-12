import { describe, expect, it } from "vitest";
import { DEFAULT_WORKFLOW_LIMITS, type WorkflowAttempt, type WorkflowRunSnapshot, type WorkflowTaskSnapshot } from "@/tauri/workflows";
import { checkpointGroups, currentFileChanges, isSnapshotArtifact, taskRouteLabel, taskStatusLabel, taskVerification, workflowCapacity } from "./workflow-presentation";

const usage = { input_tokens: 0, output_tokens: 0, total_tokens: 0, reserved_tokens: 0, estimated_tokens: 0, tokens_unknown: false, cost_usd: 0, cost_unknown: false };

function attempt(status: WorkflowAttempt["status"] = "succeeded", generation = 1): WorkflowAttempt {
  return { id: `attempt-${generation}`, generation, operation_id: "operation", status, route_id: "claude-writer", started_at_ms: 1, finished_at_ms: status === "succeeded" ? 2 : null, cancel_requested: false, reserved_tokens: 0, external_ref: null, output: { ok: true }, error: null, usage, artifacts: [] };
}
function task(id: string, status: WorkflowTaskSnapshot["status"] = "succeeded"): WorkflowTaskSnapshot {
  const current = status === "queued" || status === "blocked" ? null : attempt(status);
  return { retired: false, spec: { id, title: id, prompt: "Synthetic task", dependencies: [], route_id: "claude-writer", access: "read_only", scope: [], required: true }, generation: 1, status, depth: 0, parent_task_id: null, current_attempt: current, attempts: current ? [current] : [], result: current?.output ?? null, error: null, waiting_for: [], messages: [] };
}
function run(tasks: WorkflowTaskSnapshot[]): WorkflowRunSnapshot {
  return { id: "run", status: "running", cancel_requested: false, pause_requested: false, error: null, revision: 1, created_at_ms: 1, updated_at_ms: 2, spec: { workspace_id: "workspace", title: "Synthetic workflow", goal: "Inspect", mode: "live", allow_writes: true, routes: [{ id: "claude-writer", provider: "claude", model: "configured-model", effort: "high" }, { id: "codex-reviewer", provider: "codex" }], limits: DEFAULT_WORKFLOW_LIMITS, tasks: [] }, tasks, usage, resolved_limits: { ...DEFAULT_WORKFLOW_LIMITS, concurrency: 4 } };
}
function artifact(attemptId = "attempt-1") {
  return { kind: "workspace_snapshot", run_id: "run", attempt_id: attemptId, baseline_digest: "baseline", digest: "sealed", changed_paths: ["src/recovery.ts"] };
}
function writer(id = "writer") {
  const item = task(id);
  item.spec.access = "write";
  item.current_attempt!.artifacts = [artifact()];
  return item;
}
const ids = (tasks: WorkflowTaskSnapshot[]) => tasks.map((item) => item.spec.id);

describe("workflow checkpoint presentation", () => {
  it("groups attention, in-flight, file review and settled work in stable snapshot order", () => {
    const optionalCancelled = task("optional-cancelled", "cancelled"); optionalCancelled.spec.required = false;
    const retired = task("retired", "failed"); retired.retired = true;
    const snapshot = run([task("queued", "queued"), writer(), task("failed", "failed"), task("done"), task("waiting", "waiting"), task("blocked", "blocked"), task("unknown", "unknown"), task("required-cancelled", "cancelled"), optionalCancelled, retired, task("stopping", "stopping")]);
    const groups = checkpointGroups(snapshot);
    expect(ids(groups.needsYou)).toEqual(["failed", "blocked", "unknown", "required-cancelled"]);
    expect(ids(groups.inFlight)).toEqual(["queued", "waiting", "stopping"]);
    expect(ids(groups.readyToReview)).toEqual(["writer"]);
    expect(ids(groups.settled)).toEqual(["done", "optional-cancelled", "retired"]);
  });

  it.each(["cancel_requested", "cancelled"])("leaves cancelled run history inspectable and unknown holds actionable (%s)", (cancellation) => {
    const unknown = task("unknown", "unknown"); unknown.retired = true;
    const snapshot = run([task("failed", "failed"), task("blocked", "blocked"), task("cancelled", "cancelled"), unknown, writer()]);
    if (cancellation === "cancel_requested") snapshot.cancel_requested = true;
    else snapshot.status = "cancelled";
    const groups = checkpointGroups(snapshot);
    expect(ids(groups.needsYou)).toEqual(["unknown"]);
    expect(groups.readyToReview).toEqual([]);
    expect(ids(groups.settled)).toEqual(["failed", "blocked", "cancelled", "writer"]);
  });

  it("keeps current file evidence settled when writes or live execution are unavailable", () => {
    const snapshot = run([writer()]);
    snapshot.spec.allow_writes = false;
    expect(checkpointGroups(snapshot).readyToReview).toEqual([]);
    expect(ids(checkpointGroups(snapshot).settled)).toEqual(["writer"]);
    snapshot.spec.allow_writes = true;
    snapshot.spec.mode = "dry_run";
    expect(checkpointGroups(snapshot).readyToReview).toEqual([]);
  });

  it("promotes only the current successful write generation with matching manifest identity", () => {
    const item = writer();
    const snapshot = run([item]);
    expect(currentFileChanges(snapshot)).toHaveLength(1);
    const old = attempt("succeeded", 0); old.artifacts = [artifact(old.id)];
    item.attempts.unshift(old);
    expect(currentFileChanges(snapshot)).toHaveLength(1);
    item.current_attempt = null;
    expect(currentFileChanges(snapshot)).toHaveLength(1);
    item.generation = 2;
    expect(currentFileChanges(snapshot)).toEqual([]);
    item.generation = 1;
    item.retired = true;
    expect(currentFileChanges(snapshot)).toEqual([]);
  });

  it.each([
    { ...artifact(), run_id: "another-run" },
    { ...artifact(), attempt_id: "another-attempt" },
    { ...artifact(), digest: "" },
    { ...artifact(), baseline_digest: "" },
    { ...artifact(), changed_paths: ["../escape"] },
    { ...artifact(), changed_paths: ["/absolute"] },
    { ...artifact(), changed_paths: [".git/config"] },
    { ...artifact(), changed_paths: ["src/recovery.ts", "src/recovery.ts"] },
    { ...artifact(), changed_paths: [42] },
    { ...artifact(), changed_paths: [] },
  ])("does not promote invalid or empty file change metadata %#", (manifest) => {
    const item = writer(); item.current_attempt!.artifacts = [manifest];
    expect(currentFileChanges(run([item]))).toEqual([]);
  });

  it("retains historical manifests without promoting them after failed repair", () => {
    const item = writer();
    const previous = item.current_attempt!;
    item.generation = 2;
    item.status = "failed";
    item.current_attempt = attempt("failed", 2);
    item.attempts.push(item.current_attempt);
    expect(currentFileChanges(run([item]))).toEqual([]);
    expect(previous.artifacts).toEqual([artifact()]);
  });

  it("requires displayable manifest fields without claiming bytes have been verified", () => {
    expect(isSnapshotArtifact(artifact())).toBe(true);
    expect(isSnapshotArtifact({ ...artifact(), changed_paths: [] })).toBe(true);
    expect(isSnapshotArtifact({ ...artifact(), kind: "dry_run_report" })).toBe(false);
    expect(isSnapshotArtifact(null)).toBe(false);
  });

  it("counts only admitted attempts, with waiting parents yielding and unknown capacity retained", () => {
    const dispatching = task("dispatching", "running"); dispatching.current_attempt!.status = "dispatching";
    const snapshot = run([dispatching, task("running", "running"), task("stopping", "stopping"), task("unknown", "unknown"), task("waiting", "waiting"), task("queued", "queued"), task("done")]);
    expect(workflowCapacity(snapshot)).toEqual({ held: 4, active: 3, unknown: 1, unconfirmed: 0, ceiling: 4, markers: ["unknown", "active", "active", "active"], overflow: 0 });
    snapshot.cancel_requested = true;
    expect(workflowCapacity(snapshot).held).toBe(4);
  });

  it("reports absent or stale admission metadata as unconfirmed without free-slot claims", () => {
    const missing = task("missing", "running"); missing.current_attempt = null;
    const stale = task("stale", "unknown"); stale.generation = 2;
    const waiting = task("waiting", "waiting"); waiting.current_attempt = null;
    expect(workflowCapacity(run([missing, stale, waiting]))).toEqual({ held: 0, active: 0, unknown: 0, unconfirmed: 2, ceiling: 4, markers: ["unconfirmed", "unconfirmed"], overflow: 0 });
  });

  it("bounds capacity markers without losing the ceiling or overflow count", () => {
    const snapshot = run([]); snapshot.resolved_limits.concurrency = 128;
    expect(workflowCapacity(snapshot)).toMatchObject({ held: 0, ceiling: 128, markers: Array(8).fill("free"), overflow: 120 });
  });

  it("shows the current routed provider rather than a historical route or invented model", () => {
    const item = task("task");
    expect(taskRouteLabel(run([item]), item)).toBe("Claude · configured-model · high effort");
    item.current_attempt!.route_id = "codex-reviewer";
    expect(taskRouteLabel(run([item]), item)).toBe("Codex");
    item.generation = 2;
    expect(taskRouteLabel(run([item]), item)).toBe("Claude · configured-model · high effort");
    item.spec.route_id = "removed-route";
    expect(taskRouteLabel(run([item]), item)).toBe("Route unavailable");
  });

  it("reports schema acceptance only for a successful current generation with a declared schema", () => {
    const item = writer();
    expect(taskVerification(run([item]), item)).toEqual({ resultAccepted: true, schemaAccepted: false, retainedFiles: 1, summary: "Result accepted. No output schema was declared." });
    item.spec.output_schema = { type: "object" };
    expect(taskVerification(run([item]), item).schemaAccepted).toBe(true);
    item.spec.output_schema = false;
    expect(taskVerification(run([item]), item).schemaAccepted).toBe(true);
    item.generation = 2;
    expect(taskVerification(run([item]), item)).toMatchObject({ resultAccepted: false, schemaAccepted: false, retainedFiles: 0 });
    expect(taskVerification(run([item]), item).summary).not.toMatch(/test|build|hash/i);
  });

  it("uses actionable status text for yielded, blocked and unverified tasks", () => {
    expect(taskStatusLabel(task("waiting", "waiting"))).toBe("Waiting for tasks");
    expect(taskStatusLabel(task("blocked", "blocked"))).toBe("Dependency blocked");
    const item = task("unknown", "unknown"); item.retired = true;
    expect(taskStatusLabel(item)).toBe("Needs confirmation");
  });

  it("preserves accepted result evidence when a completed task is retired", () => {
    const item = writer();
    item.spec.output_schema = { type: "object" };
    item.retired = true;
    expect(taskVerification(run([item]), item)).toMatchObject({ resultAccepted: true, schemaAccepted: true, retainedFiles: 0 });
    expect(checkpointGroups(run([item])).readyToReview).toEqual([]);
  });
});
