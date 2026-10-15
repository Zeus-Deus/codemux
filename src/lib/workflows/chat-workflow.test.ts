import { describe, expect, it } from "vitest";
import { buildChatWorkflowLaunch, chatWorkflowPhase, CHAT_WORKFLOW_SCRIPT } from "./chat-workflow";
import { parseWorkflowSettings } from "./workflow-settings";

interface FakeTask { id: string; title: string; route_id: string; access: string; scope: string[]; required?: boolean; output_schema: unknown; prompt: string }
type Outcome = { status: string; output: unknown; error: string | null };

/** Run the `/workflow` program the way the engine does: an async function
 *  body with `agent`, `parallel`, `workflow`, `args`, `goal`, `routes` and
 *  `allow_writes` in scope. `respond` plays every agent. */
async function runScript({ respond, routes = [{ id: "claude-opus", provider: "claude" }, { id: "claude-haiku", provider: "claude" }], allowWrites = false, maxWorkers = 4 }: {
  respond: (task: FakeTask) => Outcome;
  routes?: { id: string; provider: string }[];
  allowWrites?: boolean;
  maxWorkers?: number;
}) {
  const tasks: FakeTask[] = [];
  const phases: string[] = [];
  const agent = async (prompt: string, options: Record<string, unknown>) => {
    const task = { ...options, prompt, output_schema: options.output_schema ?? options.schema } as FakeTask;
    tasks.push(task);
    return { id: task.id, ...respond(task) };
  };
  const parallel = <T, R>(items: T[], fn: (item: T, index: number) => Promise<R>) => Promise.all(items.map(fn));
  const workflow = { phase: async (name: string) => { phases.push(name); } };
  const AsyncFunction = Object.getPrototypeOf(async () => {}).constructor as new (...args: string[]) => (...values: unknown[]) => Promise<unknown>;
  const program = new AsyncFunction("agent", "parallel", "workflow", "args", "goal", "routes", "allow_writes", CHAT_WORKFLOW_SCRIPT);
  const result = await program(agent, parallel, workflow, { max_workers: maxWorkers, route_labels: { "claude-opus": "Claude Opus 5.5" } }, "Review auth", routes, allowWrites);
  return { result, tasks, phases };
}

const plan = (subtasks: { title: string; route_id: string; edits?: boolean }[]): Outcome => ({ status: "succeeded", error: null, output: { tasks: subtasks.map((task) => ({ prompt: `Do ${task.title}`, edits: false, ...task })) } });
const summary = (text: string): Outcome => ({ status: "succeeded", error: null, output: { summary: text } });

describe("CHAT_WORKFLOW_SCRIPT", () => {
  it("plans, fans the subtasks out in parallel on their chosen routes, then writes one report", async () => {
    const { result, tasks, phases } = await runScript({
      respond: (task) => task.id === "plan"
        ? plan([{ title: "Sessions", route_id: "claude-opus" }, { title: "Tests", route_id: "claude-haiku" }])
        : summary(task.id === "report" ? "Final report" : `Found in ${task.title}`),
    });
    expect(phases).toEqual(["Plan", "Work", "Report"]);
    expect(tasks.map((task) => [task.id, task.route_id, task.access])).toEqual([
      ["plan", "claude-opus", "read_only"],
      ["work-1", "claude-opus", "read_only"],
      ["work-2", "claude-haiku", "read_only"],
      ["report", "claude-opus", "read_only"],
    ]);
    // The planner sees the route catalog and the schema limits its choices.
    expect(tasks[0].prompt).toContain("claude-opus: Claude Opus 5.5 (the chat's own model");
    expect(tasks[0].output_schema).toMatchObject({ properties: { tasks: { maxItems: 4, items: { properties: { route_id: { enum: ["claude-opus", "claude-haiku"] } } } } } });
    // Workers are optional so one failure still reaches the report.
    expect(tasks[1].required).toBe(false);
    expect(tasks[3].prompt).toContain("Found in Sessions");
    expect(result).toEqual({ summary: "Final report" });
  });

  it("caps subtasks at the run size and falls back to the main route for unknown routes", async () => {
    const { tasks } = await runScript({
      maxWorkers: 2,
      respond: (task) => task.id === "plan"
        ? plan([{ title: "A", route_id: "not-a-route" }, { title: "B", route_id: "claude-haiku" }, { title: "C", route_id: "claude-haiku" }])
        : summary("ok"),
    });
    expect(tasks.filter((task) => task.id.startsWith("work-")).map((task) => task.route_id)).toEqual(["claude-opus", "claude-haiku"]);
  });

  it("only grants write access when the run allows file changes and the planner asks for edits", async () => {
    const respond = (task: FakeTask) => task.id === "plan"
      ? plan([{ title: "Fix", route_id: "claude-opus", edits: true }, { title: "Read", route_id: "claude-opus", edits: false }])
      : summary("ok");
    const readOnly = await runScript({ respond });
    expect(readOnly.tasks.filter((task) => task.id.startsWith("work-")).map((task) => task.access)).toEqual(["read_only", "read_only"]);
    const writable = await runScript({ respond, allowWrites: true });
    expect(writable.tasks.filter((task) => task.id.startsWith("work-")).map((task) => [task.access, task.scope])).toEqual([["write", ["."]], ["read_only", []]]);
  });

  it("reports subtask failures to the report agent and fails the run when planning fails", async () => {
    const { tasks } = await runScript({
      respond: (task) => task.id === "plan" ? plan([{ title: "A", route_id: "claude-opus" }])
        : task.id === "work-1" ? { status: "failed", output: null, error: "provider crashed" } : summary("report"),
    });
    expect(tasks[tasks.length - 1]?.prompt).toContain("provider crashed");
    await expect(runScript({ respond: () => ({ status: "failed", output: null, error: "no model" }) })).rejects.toThrow("Planning failed: no model");
  });
});

describe("buildChatWorkflowLaunch", () => {
  it("builds a live run from Settings with the planner script and route labels", () => {
    const settings = parseWorkflowSettings({ "workflows.size": "large", "workflows.file_changes": "allow" });
    const launch = buildChatWorkflowLaunch({
      workspaceId: "ws-1",
      goal: "Review the auth module\nand its tests",
      settings,
      plan: { routes: [{ id: "codex", provider: "codex", model: null, effort: null }], labels: { codex: "Codex (default model)" }, problem: null },
    });
    expect(launch.source).toBe(CHAT_WORKFLOW_SCRIPT);
    expect(launch.args).toEqual({ max_workers: 10, route_labels: { codex: "Codex (default model)" } });
    expect(launch.spec).toMatchObject({ workspace_id: "ws-1", title: "Review the auth module", mode: "live", allow_writes: true, tasks: [], limits: { concurrency: 8, wall_time_ms: 3_600_000 } });
  });

  it("maps task ids to card phases", () => {
    expect(["plan", "work-3", "report", "inspect-0"].map(chatWorkflowPhase)).toEqual(["plan", "work", "report", null]);
  });
});
