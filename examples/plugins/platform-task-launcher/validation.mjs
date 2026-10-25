export const LIMITS = Object.freeze({ tasks: 6, plans: 12, planBytes: 24576, stateBytes: 850000 });
const record = (value) => value !== null && typeof value === "object" && !Array.isArray(value);
const fail = (message) => {
  throw new Error(message);
};
export function fields(value, allowed) {
  if (!record(value) || Object.keys(value).some((key) => !allowed.includes(key)))
    fail("Unsupported input field or invalid object.");
  return value;
}
export function text(value, max, name) {
  if (typeof value !== "string" || !value.trim() || Buffer.byteLength(value, "utf8") > max)
    fail(`A bounded ${name} is required (maximum ${max} UTF-8 bytes).`);
  return value.trim();
}
export function safeId(value) {
  if (typeof value !== "string" || !/^[a-zA-Z0-9][a-zA-Z0-9._-]{0,127}$/.test(value))
    fail("A valid plan or task identifier is required.");
  return value;
}
export function model(value) {
  fields(value, ["instanceId", "model", "options"]);
  const result = {
    instanceId: text(value.instanceId, 256, "provider instance"),
    model: text(value.model, 256, "model"),
  };
  if (value.options !== undefined) {
    const options = Array.isArray(value.options) ? value.options
      : record(value.options) ? Object.entries(value.options).map(([id, value]) => ({id, value})) : null;
    if (!options || options.length > 32) fail("Model options must be bounded selections.");
    const ids = new Set();
    result.options = options.map(option => {
      fields(option, ["id", "value"]);
      const id = text(option.id, 128, "option id");
      if (ids.has(id)) fail("Duplicate model option.");
      ids.add(id);
      if (typeof option.value !== "boolean" && (typeof option.value !== "string" || !option.value || option.value.length > 256))
        fail("Invalid native model option value.");
      return { id, value: option.value };
    });
  }
  if (Buffer.byteLength(JSON.stringify(result)) > 4096) fail("Model selection is too large.");
  return result;
}
export function choices(input, defaults, canWorktree) {
  const runtimeMode = input.runtimeMode ?? defaults.runtimeMode;
  const nativePermissionMode = input.nativePermissionMode ?? defaults.nativePermissionMode;
  if (nativePermissionMode !== undefined) text(nativePermissionMode, 128, "native permission mode");
  if (!["approval-required", "auto-accept-edits", "full-access"].includes(runtimeMode) && !nativePermissionMode)
    fail("Unsupported runtime mode.");
  const interactionMode = input.interactionMode ?? defaults.interactionMode;
  if (!["default", "plan"].includes(interactionMode)) fail("Unsupported interaction mode.");
  const workspace = input.workspaceStrategy ?? { type: "root" };
  if (!record(workspace)) fail("A workspace strategy is required.");
  let workspaceStrategy;
  if (workspace.type === "root") {
    fields(workspace, ["type"]);
    workspaceStrategy = { type: "root" };
  } else if (workspace.type === "worktree") {
    if (!canWorktree) fail("Worktree creation is not granted.");
    fields(workspace, ["type", "baseRef", "branch", "startFromOrigin"]);
    workspaceStrategy = { type: "worktree", baseRef: text(workspace.baseRef, 256, "base ref") };
    if (workspace.branch !== undefined)
      workspaceStrategy.branch = text(workspace.branch, 256, "branch");
    if (workspace.startFromOrigin !== undefined) {
      if (typeof workspace.startFromOrigin !== "boolean") fail("Invalid origin choice.");
      workspaceStrategy.startFromOrigin = workspace.startFromOrigin;
    }
  } else fail("Unsupported workspace strategy.");
  return {
    modelSelection: model(input.modelSelection ?? defaults.modelSelection),
    ...(runtimeMode ? { runtimeMode } : {}),
    ...(nativePermissionMode ? { nativePermissionMode } : {}),
    interactionMode,
    workspaceStrategy,
  };
}
export function proposal(input) {
  fields(input, ["title", "tasks"]);
  if (!Array.isArray(input.tasks) || input.tasks.length < 1 || input.tasks.length > LIMITS.tasks)
    fail("Propose between one and six tasks.");
  const result = {
    title: input.title === undefined ? "Proposed tasks" : text(input.title, 120, "plan title"),
    tasks: input.tasks.map((task) => {
      fields(task, ["title", "prompt"]);
      return {
        title: text(task.title, 100, "task title"),
        prompt: text(task.prompt, 2000, "prompt"),
      };
    }),
  };
  if (Buffer.byteLength(JSON.stringify(result)) > LIMITS.planBytes)
    fail("The plan is too large (24 KiB maximum).");
  return result;
}
export function state(value) {
  if (value === null) return { version: 1, plans: [] };
  if (
    !record(value) ||
    value.version !== 1 ||
    !Array.isArray(value.plans) ||
    value.plans.length > LIMITS.plans ||
    Buffer.byteLength(JSON.stringify(value)) > LIMITS.stateBytes
  )
    fail("Unsupported or damaged stored plan format/version.");
  fields(value, ["version", "plans"]);
  const seen = new Set();
  for (const plan of value.plans) {
    if (
      !record(plan) ||
      plan.version !== 1 ||
      !Array.isArray(plan.tasks) ||
      seen.has(safeId(plan.id))
    )
      fail("Unsupported or damaged stored plan format/version.");
    fields(plan, [
      "version",
      "id",
      "title",
      "sourceThreadId",
      "projectId",
      "createdAt",
      "defaults",
      "tasks",
    ]);
    seen.add(plan.id);
    text(plan.sourceThreadId, 512, "source thread");
    text(plan.projectId, 512, "project");
    text(plan.createdAt, 40, "proposal time");
    proposal({
      title: plan.title,
      tasks: plan.tasks.map((t) => ({ title: t.title, prompt: t.prompt })),
    });
    fields(plan.defaults, ["modelSelection", "runtimeMode", "nativePermissionMode", "interactionMode"]);
    choices({}, plan.defaults, true);
    const tasks = new Set();
    for (const task of plan.tasks) {
      fields(task, ["id", "title", "prompt", "launch"]);
      if (
        tasks.has(safeId(task.id)) ||
        !record(task.launch) ||
        !["proposed", "pending", "uncertain", "accepted", "failed", "abandoned"].includes(task.launch.state)
      )
        fail("Damaged task journal.");
      tasks.add(task.id);
      if (
        task.launch.state === "proposed" &&
        Object.keys(task.launch).some((key) => key !== "state")
      )
        fail("A proposed task cannot carry a dispatch journal.");
      fields(task.launch, [
        "state",
        "operationId",
        "request",
        "threadId",
        "receipt",
        "diagnostic",
        "acknowledgedAt",
        "acknowledgementOperationId",
      ]);
      if (task.launch.diagnostic !== undefined)
        text(task.launch.diagnostic, 300, "launch diagnostic");
      if (task.launch.state === "abandoned") {
        text(task.launch.acknowledgedAt, 40, "acknowledgement time");
        text(task.launch.acknowledgementOperationId, 128, "acknowledgement operation id");
      } else if (
        task.launch.acknowledgedAt !== undefined ||
        task.launch.acknowledgementOperationId !== undefined
      ) {
        fail("Damaged task acknowledgement journal.");
      }
      if (task.launch.state !== "proposed") {
        text(task.launch.operationId, 128, "operation id");
        const request = task.launch.request;
        if (
          !record(request) ||
          request.projectId !== plan.projectId ||
          request.title !== task.title ||
          request.text !== task.prompt ||
          request.idempotencyKey !== `launch-${task.id}`
        )
          fail("Damaged frozen launch request.");
        fields(request, [
          "projectId",
          "title",
          "text",
          "idempotencyKey",
          "modelSelection",
          "runtimeMode",
          "nativePermissionMode",
          "interactionMode",
          "workspaceStrategy",
        ]);
        choices(request, plan.defaults, true);
      }
      if (task.launch.receipt !== undefined) receipt(task.launch.receipt);
      if (task.launch.state === "accepted") {
        text(task.launch.threadId, 512, "accepted thread");
        if (task.launch.receipt?.state !== "succeeded" || task.launch.receipt.thread_id !== task.launch.threadId)
          fail("Damaged accepted receipt journal.");
      }
    }
  }
  return value;
}

/** Native operation success acknowledges a command; agent work may still be running. */
export function receipt(value) {
  if (!record(value) || Buffer.byteLength(JSON.stringify(value)) > 16384)
    fail("Invalid or oversized native operation receipt.");
  text(value.operation_id, 256, "native operation id");
  text(value.state, 128, "native operation state");
  if (value.thread_id !== null && value.thread_id !== undefined) text(value.thread_id, 512, "native thread id");
  return structuredClone(value);
}
export function journalReceipt(value, previous) {
  const accepted = receipt(value);
  return {
    ...previous,
    state: accepted.state === "succeeded" && accepted.thread_id ? "accepted"
      : ["accepted", "running"].includes(accepted.state) ? "pending"
      : accepted.state === "failed" ? "failed" : "uncertain",
    receipt: accepted,
    ...(accepted.thread_id ? { threadId: accepted.thread_id } : {}),
  };
}
