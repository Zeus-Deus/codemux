export const WORKFLOW_STARTER = `await workflow.phase("Inspect");
const areas = ["architecture", "tests", "integration"];
const findings = await parallel(areas, (area, index) =>
  agent(goal + "\\nInspect " + area + ". Return concrete findings.", {
    id: "inspect-" + index,
    title: "Inspect " + area,
    route_id: routes[index % routes.length].id,
    access: "read_only"
  })
);
await workflow.phase("Synthesize");
return await agent("Summarize findings and propose the next steps: " +
  JSON.stringify(findings), {
    id: "synthesis",
    title: "Synthesize findings",
    route_id: routes[0].id,
    access: "read_only"
  });`;
