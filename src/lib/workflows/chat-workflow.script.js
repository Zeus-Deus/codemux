// The `/workflow` program: an async function body run by the CodeMux workflow
// engine (QuickJS) with goal, args, routes, allow_writes, agent, parallel and
// workflow in scope. Shared by the app and the live canary test.
const labels = (args && args.route_labels) || {};
const routeIds = routes.map((route) => route.id);
const maxWorkers = Math.max(1, Math.min(Number(args && args.max_workers) || 4, 12));
const canWrite = allow_writes === true;
const summarySchema = { type: "object", additionalProperties: false, required: ["summary"], properties: { summary: { type: "string" } } };
const catalog = routes.map((route, index) =>
  "- " + route.id + ": " + (labels[route.id] || route.provider) + (index === 0 ? " (the chat's own model, strongest default)" : "")
).join("\n");

await workflow.phase("Plan");
const plan = await agent([
  "You are the planner of a CodeMux workflow. Inspect the workspace as needed, then split the goal into independent subtasks that separate agents can do in parallel.",
  "",
  "Rules:",
  "- Return between 1 and " + maxWorkers + " subtasks. Use fewer for small goals; never pad the list.",
  "- Each subtask prompt must be self-contained: the worker sees only the goal and that prompt.",
  "- Choose a route_id for every subtask. Give narrow, mechanical work to lighter models and hard reasoning to the strongest.",
  canWrite
    ? "- Set edits to true only for subtasks that must change files. Changed files stay isolated until the user reviews them."
    : "- This run is read-only. Set edits to false; workers cannot change files.",
  "",
  "Routes:",
  catalog
].join("\n"), {
  id: "plan",
  title: "Plan the work",
  route_id: routeIds[0],
  access: "read_only",
  schema: {
    type: "object",
    additionalProperties: false,
    required: ["tasks"],
    properties: {
      tasks: {
        type: "array",
        minItems: 1,
        maxItems: maxWorkers,
        items: {
          type: "object",
          additionalProperties: false,
          required: ["title", "prompt", "route_id", "edits"],
          properties: {
            title: { type: "string", minLength: 1, maxLength: 120 },
            prompt: { type: "string", minLength: 1 },
            route_id: { type: "string", enum: routeIds },
            edits: { type: "boolean" }
          }
        }
      }
    }
  }
});
if (plan.status !== "succeeded" || !plan.output || !Array.isArray(plan.output.tasks)) {
  throw new Error("Planning failed" + (plan.error ? ": " + plan.error : "."));
}
const subtasks = plan.output.tasks.slice(0, maxWorkers);

await workflow.phase("Work");
const results = await parallel(subtasks, (task, index) => {
  const write = canWrite && task.edits === true;
  return agent(String(task.prompt) + "\n\nSubmit a concise summary of what you found or changed, citing file paths where relevant.", {
    id: "work-" + (index + 1),
    title: String(task.title || "Subtask " + (index + 1)).slice(0, 120),
    route_id: routeIds.indexOf(task.route_id) >= 0 ? task.route_id : routeIds[0],
    access: write ? "write" : "read_only",
    scope: write ? ["."] : [],
    required: false,
    schema: summarySchema
  });
});

await workflow.phase("Report");
const findings = results.map((result, index) => ({
  subtask: subtasks[index].title,
  status: result.status,
  summary: result.output && typeof result.output.summary === "string" ? result.output.summary.slice(0, 6000) : null,
  error: result.error ? String(result.error).slice(0, 1000) : null
}));
const report = await agent(
  "Subtask results (JSON):\n" + JSON.stringify(findings) +
  "\n\nWrite the final report for the user in Markdown. Lead with the answer, then the key findings in order of importance, then concrete next steps. Mention any subtask that failed.",
  { id: "report", title: "Write the report", route_id: routeIds[0], access: "read_only", schema: summarySchema }
);
if (report.status !== "succeeded" || !report.output) {
  throw new Error("The report could not be written" + (report.error ? ": " + report.error : "."));
}
return { summary: report.output.summary };
