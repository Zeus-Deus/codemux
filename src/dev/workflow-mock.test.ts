import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_WORKFLOW_LIMITS, type WorkflowCapability, type WorkflowRunSnapshot, type WorkflowRunSpec, type WorkflowRunSummary, type WorkflowScript, type WorkflowTaskSpec } from "@/tauri/workflows";
import { resetWorkflowMock, workflowMockHandlers } from "./workflow-mock";

const handlers = workflowMockHandlers();
function task(id: string, dependencies: string[] = [], prompt = "Inspect the fixture"): WorkflowTaskSpec {
  return { id, title: id, prompt, dependencies, route_id: "codex", access: "read_only", scope: [], required: true };
}
function spec(tasks: WorkflowTaskSpec[] = []): WorkflowRunSpec {
  return { workspace_id: "fixture-workspace", title: "Fixture", goal: "Review synthetic fixture", mode: "dry_run", allow_writes: false, routes: [{ id: "codex", provider: "codex" }], limits: { ...DEFAULT_WORKFLOW_LIMITS, concurrency: 1 }, tasks };
}
const get = (id: string) => handlers.workflow_get({ runId: id }) as WorkflowRunSnapshot;
const create = (runSpec: WorkflowRunSpec, idempotencyKey = "fixture-key") => handlers.workflow_create({ spec: runSpec, idempotencyKey }) as WorkflowRunSnapshot;

beforeEach(() => { vi.useFakeTimers(); resetWorkflowMock(); });
afterEach(() => { resetWorkflowMock(); vi.useRealTimers(); });

describe("token-free workflow preview", () => {
  it("exposes provider setup requirements while refusing live execution for every preview adapter", () => {
    const capabilities = handlers.workflow_capabilities({}) as WorkflowCapability[];
    expect(capabilities.map((item) => item.provider)).toEqual(["claude", "codex", "hermes", "opencode", "cursor", "grok"]);
    expect(capabilities.every((item) => !item.live && !item.write)).toBe(true);
    expect(capabilities.filter((item) => item.requires_model).map((item) => item.provider)).toEqual(["opencode"]);
    expect(capabilities.filter((item) => !item.effort_supported).map((item) => item.provider)).toEqual(["cursor"]);
  });

  it("admits one worker, honors dependencies, and settles with zero usage", () => {
    const run = create(spec([task("inspect"), task("synthesize", ["inspect"])]));
    vi.advanceTimersByTime(100);
    expect(get(run.id).tasks.map((item) => item.status)).toEqual(["running", "queued"]);
    vi.advanceTimersByTime(1500);
    expect(get(run.id).tasks.map((item) => item.status)).toEqual(["succeeded", "running"]);
    vi.advanceTimersByTime(1500);
    const finished = get(run.id);
    expect(finished.status).toBe("completed");
    expect(finished.usage.total_tokens).toBe(0);
    expect(finished.tasks[1].attempts[0].route_id).toBe("codex");
    expect(finished.tasks[1].result).toMatchObject({ dry_run: true });
  });

  it("pause drains admitted work and resume releases the next dependency", () => {
    const run = create(spec([task("first"), task("second", ["first"])]));
    vi.advanceTimersByTime(100);
    handlers.workflow_pause({ runId: run.id });
    vi.advanceTimersByTime(3000);
    expect(get(run.id).tasks.map((item) => item.status)).toEqual(["succeeded", "queued"]);
    expect(get(run.id).status).toBe("paused");
    handlers.workflow_resume({ runId: run.id });
    vi.advanceTimersByTime(100);
    expect(get(run.id).tasks[1].status).toBe("running");
  });

  it("retries settled failures with a fresh attempt and enforces the attempt budget", () => {
    const runSpec = spec([task("failure", [], "[mock:fail]")]);
    runSpec.limits.max_attempts = 1;
    const run = create(runSpec);
    vi.advanceTimersByTime(1700);
    expect(get(run.id).status).toBe("failed");
    handlers.workflow_retry({ runId: run.id, taskId: "failure" });
    vi.advanceTimersByTime(100);
    const retried = get(run.id).tasks[0];
    expect(retried.generation).toBe(1);
    expect(retried.error).toBe("Attempt budget exceeded.");
    expect(retried.attempts).toHaveLength(1);
  });

  it("cancels a run without treating cancelled tasks as succeeded", () => {
    const run = create(spec([task("a"), task("b")]));
    vi.advanceTimersByTime(100);
    handlers.workflow_cancel({ runId: run.id });
    vi.advanceTimersByTime(3000);
    expect(get(run.id).status).toBe("cancelled");
    expect(get(run.id).tasks.map((item) => item.status)).toEqual(["cancelled", "cancelled"]);
  });

  it.each([
    { command: "workflow_cancel_task", retired: false, outcome: "failed" },
    { command: "workflow_retire", retired: true, outcome: "completed" },
  ])("$command preserves its distinct required-task outcome until explicit retry", ({ command, retired, outcome }) => {
    const run = create(spec([task("required")]));
    handlers[command]({ runId: run.id, taskId: "required" });
    vi.advanceTimersByTime(100);
    expect(get(run.id).tasks[0]).toMatchObject({ status: "cancelled", retired });
    expect(get(run.id).status).toBe(outcome);
    handlers.workflow_retry({ runId: run.id, taskId: "required" });
    vi.advanceTimersByTime(1700);
    expect(get(run.id).tasks[0]).toMatchObject({ status: "succeeded", retired: false });
    expect(get(run.id).status).toBe("completed");
  });

  it("keeps declared dependents blocked when a prerequisite is retired", () => {
    const run = create(spec([task("obsolete"), task("dependent", ["obsolete"])]));
    handlers.workflow_retire({ runId: run.id, taskId: "obsolete" });
    vi.advanceTimersByTime(100);
    expect(get(run.id).tasks.map((item) => item.status)).toEqual(["cancelled", "blocked"]);
    expect(get(run.id).status).toBe("failed");
  });

  it("persists successful retirement while preserving its result and attempt audit", () => {
    const run = create(spec([task("done")]));
    vi.advanceTimersByTime(1700);
    const accepted = get(run.id).tasks[0];
    handlers.workflow_retire({ runId: run.id, taskId: "done" });
    const retired = get(run.id).tasks[0];
    expect(retired).toMatchObject({ status: "succeeded", retired: true, result: accepted.result, attempts: accepted.attempts });
    const stored = JSON.parse(localStorage.getItem("codemux:dev-workflows:v1")!);
    expect(stored.runs[0].tasks[0].retired).toBe(true);
  });

  it("deduplicates launch keys and saves reloadable reusable scripts", () => {
    const run = create(spec([task("a")]));
    expect(create(spec([task("a")])).id).toBe(run.id);
    handlers.workflow_script_save({ workspaceId: "fixture-workspace", name: "Review", source: "return await agent(goal);" });
    const saved = handlers.workflow_script_list({ workspaceId: "fixture-workspace" }) as WorkflowScript[];
    expect(saved).toHaveLength(1);
    expect(saved[0]).toMatchObject({ title: "Review", source: "return await agent(goal);" });
    const stored = JSON.parse(localStorage.getItem("codemux:dev-workflows:v1")!);
    expect(stored.runs[0].id).toBe(run.id);
    expect(stored.scripts[0].source).toBe(saved[0].source);
  });

  it("returns compact history summaries without task prompts, scripts, or results", () => {
    const runSpec = spec([task("inspect", [], "private task prompt")]);
    runSpec.script = { source: 'return "private script source";', args: { private: "script argument" }, api_version: 1 };
    const run = create(runSpec);
    vi.advanceTimersByTime(1700);
    const full = get(run.id);
    expect(full.tasks[0].result).not.toBeNull();
    expect(full.spec.script?.source).toContain("private script source");
    const listed = handlers.workflow_list({ workspaceId: "fixture-workspace" }) as WorkflowRunSummary[];
    expect(listed).toEqual([{ id: full.id, status: full.status, revision: full.revision, created_at_ms: full.created_at_ms, updated_at_ms: full.updated_at_ms, spec: { workspace_id: "fixture-workspace", title: "Fixture", mode: "dry_run" } }]);
    const serialized = JSON.stringify(listed);
    for (const excluded of ["tasks", "source", "prompt", "result", "private task prompt", "private script source", "script argument", "Review synthetic fixture"]) expect(serialized).not.toContain(excluded);
  });

  it("rejects invalid routes and graph budgets instead of returning shape-safe success", () => {
    expect(() => create({ ...spec(), routes: [] })).toThrow("Choose at least one provider route.");
    const runSpec = spec([task("a"), task("b")]);
    runSpec.limits.max_tasks = 1;
    expect(() => create(runSpec)).toThrow("Task budget exceeded.");
  });

  it("resolves a deliberate larger worker ceiling against the default global cap", () => {
    const runSpec = spec([task("bounded")]);
    runSpec.limits.concurrency = 256;
    const run = create(runSpec);
    expect(run.spec.limits.concurrency).toBe(256);
    expect(run.resolved_limits.concurrency).toBe(8);
  });
});
