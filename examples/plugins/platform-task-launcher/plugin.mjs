import * as NodeCrypto from "node:crypto";
import { LIMITS, choices, fields, proposal, safeId, state, text, journalReceipt, model } from "./validation.mjs";

const KEY = "plans.v1";
const UNCERTAIN =
  "Launch outcome is uncertain. Inspect threads before starting this work elsewhere.";
const ABANDONED =
  "Launch acknowledged and retired. A thread may already exist; this does not cancel or delete it. Proposing a new plan may duplicate that work.";
const NOT_DISPATCHED = "Launch was not dispatched. Review the task before starting it.";
const choiceFields = [
  "taskId",
  "modelSelection",
  "runtimeMode",
  "nativePermissionMode",
  "interactionMode",
  "workspaceStrategy",
];

export function activate(ctx) {
  const locks = new Map();
  const starting = new Set();
  const serial = async (key, operation) => {
    const previous = locks.get(key) ?? Promise.resolve();
    let release;
    const gate = new Promise((resolve) => {
      release = resolve;
    });
    const tail = previous.then(() => gate);
    locks.set(key, tail);
    await previous;
    try {
      return await operation();
    } finally {
      release();
      if (locks.get(key) === tail) locks.delete(key);
    }
  };
  const load = async () => state(await ctx.storage.get(KEY));
  const update = (operation) =>
    serial("storage", async () => {
      const current = await load();
      const result = await operation(current);
      if (Buffer.byteLength(JSON.stringify(current)) > LIMITS.stateBytes)
        throw new Error("Plan storage is full. Inspect existing plans before proposing more.");
      await ctx.storage.set(KEY, current);
      return result;
    });
  const human = (invocation) => {
    if (invocation.caller.kind !== "client")
      throw new Error("Only a person can use task launch controls.");
  };
  const sourceFor = async (invocation, { allowCancelled = false } = {}) => {
    if (allowCancelled) ctx.signal?.throwIfAborted();
    else invocation.signal.throwIfAborted();
    const threadId =
      invocation.caller.kind === "agent"
        ? invocation.caller.threadId
        : invocation.caller.kind === "client"
          ? invocation.context?.threadId
          : undefined;
    if (!threadId) throw new Error("A host-verified thread is required.");
    if (invocation.context?.threadId && invocation.context.threadId !== threadId)
      throw new Error("The target context does not match the verified thread.");
    const source = await ctx.threads.metadata({ threadId });
    if (
      source.id !== threadId ||
      (invocation.context?.projectId && invocation.context.projectId !== source.projectId)
    )
      throw new Error("The project context does not match the verified thread.");
    return source;
  };
  const authorize = async (invocation, plan, options) => {
    const source = await sourceFor(invocation, options);
    if (plan && (plan.sourceThreadId !== source.id || plan.projectId !== source.projectId))
      throw new Error("This plan does not belong to the current thread.");
    return source;
  };
  const invalidate = async () => {
    if (!ctx.capabilities.includes("ui.changes")) return;
    try {
      await ctx.ui.invalidate(["view:launcher", "view:tasks", "status:tasks"]);
    } catch {
      ctx.log("warn", "Task state changed; refresh the view if change delivery is unavailable.");
    }
  };
  const select = async (input, invocation) => {
    const source = await authorize(invocation);
    const current = await load();
    const plan =
      input.planId === undefined
        ? current.plans.find(
            (p) => p.sourceThreadId === source.id && p.projectId === source.projectId,
          )
        : current.plans.find((p) => p.id === safeId(input.planId));
    if (input.planId !== undefined && !plan)
      throw new Error("This plan is unavailable or was removed from bounded storage.");
    await authorize(invocation, plan);
    return plan ?? null;
  };
  const present = async (plan, invocation) => {
    await authorize(invocation, plan);
    let visible = null;
    if (plan) {
      const tasks = [];
      for (const task of plan.tasks) {
        const journal = task.launch;
        const state = starting.has(task.id)
          ? "starting"
          : journal.state === "pending"
            ? (journal.receipt && ["accepted", "running"].includes(journal.receipt.state) ? "pending" : "uncertain")
            : journal.state;
        let launch = { state };
        if (journal.receipt) launch.receipt = journal.receipt;
        if (state === "pending") launch.diagnostic = "Native command accepted; waiting for a receipt. Agent work is not complete.";
        if (state === "failed") launch.diagnostic = "Native operation failed. Inspect its receipt; this task is not redispatched.";
        if (state === "uncertain") launch.diagnostic = UNCERTAIN;
        if (state === "abandoned") launch.diagnostic = ABANDONED;
        if (journal.request) launch.choices = choices(journal.request, plan.defaults, true);
        if (journal.state === "accepted") {
          try {
            const target = await ctx.threads.metadata({ threadId: journal.threadId });
            if (target.id !== journal.threadId || target.projectId !== plan.projectId)
              throw new Error("unavailable");
            launch.threadId = target.id;
          } catch {
            launch.diagnostic =
              "Accepted thread is unavailable under current access. Inspect threads.";
          }
        }
        tasks.push({ id: task.id, title: task.title, prompt: task.prompt, launch });
      }
      visible = {
        version: 1,
        id: plan.id,
        title: plan.title,
        sourceThreadId: plan.sourceThreadId,
        projectId: plan.projectId,
        createdAt: plan.createdAt,
        defaults: plan.defaults,
        tasks,
      };
    }
    await authorize(invocation, plan);
    return {
      plan: visible,
      canStart: invocation.caller.kind === "client" && ctx.capabilities.includes("threads.create"),
      canWorktree: ctx.capabilities.includes("worktrees.create"),
    };
  };
  const card = (plan) => {
    const summary = `Proposed ${plan.tasks.length} tasks. Review before starting.`;
    return { message: summary, card: { viewId: "launcher", summary, data: { planId: plan.id } } };
  };
  ctx.handle("action:propose", async (input, invocation) => {
    if (invocation.caller.kind !== "agent" || !invocation.caller.threadId)
      throw new Error("A host-verified agent thread must propose tasks.");
    const proposed = proposal(input);
    const source = await authorize(invocation);
    // Legacy/unconfigured native threads may have null model identity. Validate before
    // persisting, so one unavailable source cannot poison the versioned plan store.
    const defaultModel = model(source.modelSelection);
    const plan = await update(async (current) => {
      await authorize(invocation);
      const next = {
        version: 1,
        id: NodeCrypto.randomUUID(),
        title: proposed.title,
        sourceThreadId: source.id,
        projectId: source.projectId,
        createdAt: new Date().toISOString(),
        defaults: {
          modelSelection: defaultModel,
          runtimeMode: source.runtimeMode,
          interactionMode: source.interactionMode,
        },
        tasks: proposed.tasks.map((t) => ({
          ...t,
          id: NodeCrypto.randomUUID(),
          launch: { state: "proposed" },
        })),
      };
      if (Buffer.byteLength(JSON.stringify(next)) > LIMITS.planBytes)
        throw new Error("The immutable plan is too large (24 KiB maximum).");
      if (current.plans.length >= LIMITS.plans) {
        const evict = current.plans.findLastIndex((p) =>
          p.tasks.every((t) => !["pending", "uncertain"].includes(t.launch.state)),
        );
        if (evict === -1)
          throw new Error(
            "Plan storage is full of unresolved launches. Inspect threads before proposing more.",
          );
        current.plans.splice(evict, 1);
      }
      current.plans.unshift(next);
      return next;
    });
    await invalidate();
    await authorize(invocation, plan);
    return card(plan);
  });
  ctx.handle("action:open", async (_input, invocation) => {
    human(invocation);
    await authorize(invocation);
    return { intents: [{ type: "view.open", viewId: "tasks" }] };
  });
  const read = async (input, invocation) => {
    human(invocation);
    fields(input ?? {}, ["planId"]);
    return { data: await present(await select(input ?? {}, invocation), invocation) };
  };
  const models = async (input, invocation) => {
    human(invocation);
    fields(input ?? {}, ["cursor"]);
    const cursor = input?.cursor;
    if (cursor !== undefined && (!Number.isSafeInteger(cursor) || cursor < 0))
      throw new Error("Invalid model page cursor.");
    const source = await authorize(invocation);
    const page = await ctx.models.list({
      projectId: source.projectId,
      ...(cursor === undefined ? {} : { cursor }),
      limit: 50,
    });
    await authorize(invocation);
    return { data: page };
  };
  const start = async (input, invocation, all) => {
    human(invocation);
    fields(input, all ? ["planId", "choices"] : ["planId", ...choiceFields]);
    const planId = safeId(input.planId);
    if (!ctx.capabilities.includes("threads.create"))
      throw new Error("Thread creation is not granted.");
    return serial(`plan-${planId}`, async () => {
      let plan = await select({ planId }, invocation);
      const selected = all
        ? input.choices
        : [
            Object.fromEntries(
              choiceFields
                .filter((key) => input[key] !== undefined)
                .map((key) => [key, input[key]]),
            ),
          ];
      if (!Array.isArray(selected) || selected.length < 1 || selected.length > LIMITS.tasks)
        throw new Error("Choose between one and six tasks.");
      const seen = new Set();
      const requests = selected.map((item) => {
        fields(item, choiceFields);
        const task = plan.tasks.find((t) => t.id === safeId(item.taskId));
        if (!task || seen.has(task.id))
          throw new Error("Each selected task must belong to this plan exactly once.");
        seen.add(task.id);
        return {
          taskId: task.id,
          request: {
            projectId: plan.projectId,
            title: task.title,
            text: task.prompt,
            idempotencyKey: `launch-${task.id}`,
            ...choices(item, plan.defaults, ctx.capabilities.includes("worktrees.create")),
          },
        };
      });
      const failures = new Map();
      for (const { taskId, request } of requests) {
        invocation.signal.throwIfAborted();
        await authorize(invocation, plan);
        const task = plan.tasks.find((t) => t.id === taskId);
        // Receipt identity also includes a host-private principal. Even equal operationId
        // in a later invocation cannot prove that principal, so never redispatch a journal.
        if (task.launch.state !== "proposed") continue;
        const operationId = text(invocation.operationId, 128, "operation id");
        starting.add(taskId);
        let dispatched = false;
        try {
          await update(async (current) => {
            await authorize(invocation, plan);
            const stored = current.plans.find((p) => p.id === planId);
            if (!stored) throw new Error("Plan unavailable.");
            stored.tasks.find((t) => t.id === taskId).launch = {
              state: "pending",
              operationId,
              request,
              diagnostic: UNCERTAIN,
            };
          });
          await invalidate();
          invocation.signal.throwIfAborted();
          await authorize(invocation, plan);
          invocation.signal.throwIfAborted();
          dispatched = true;
          const result = await ctx.threads.create(structuredClone(request));
          await update((current) => {
            const stored = current.plans.find((p) => p.id === planId).tasks.find((t) => t.id === taskId);
            stored.launch = journalReceipt(result, { operationId, request });
          });
        } catch {
          failures.set(taskId, UNCERTAIN);
          try {
            await update(async (current) => {
              const stored = current.plans
                .find((p) => p.id === planId)
                .tasks.find((t) => t.id === taskId);
              if (!dispatched) {
                // Only our own call boundary proves no dispatch. A cancelled local signal may
                // permit recovery, but the public host must still authorize metadata and storage.
                await authorize(invocation, plan, { allowCancelled: true });
                if (
                  stored.launch.state !== "proposed" &&
                  !(stored.launch.state === "pending" && stored.launch.operationId === operationId)
                )
                  throw new Error("The task journal changed before recovery.");
                stored.launch = { state: "proposed" };
              } else {
                // A thrown create may follow a committed effect, regardless of error.reason.
                stored.launch = { state: "uncertain", operationId, request, diagnostic: UNCERTAIN };
              }
            });
            if (!dispatched) failures.set(taskId, NOT_DISPATCHED);
          } catch {
            ctx.log(
              "warn",
              "Launch recovery could not be saved; inspect the recorded state before continuing.",
            );
          }
        } finally {
          starting.delete(taskId);
          await invalidate();
        }
        plan = await select({ planId }, invocation);
      }
      plan = await select({ planId }, invocation);
      const data = await present(plan, invocation);
      data.results = requests.map(({ taskId }) => {
        const launch = data.plan.tasks.find((t) => t.id === taskId).launch;
        return {
          taskId,
          ...launch,
          ...(failures.has(taskId) ? { diagnostic: failures.get(taskId) } : {}),
        };
      });
      return { data };
    });
  };
  const inspect = async (input, invocation) => {
    human(invocation);
    fields(input, ["planId", "taskId"]);
    const planId = safeId(input.planId), taskId = safeId(input.taskId);
    return serial(`plan-${planId}`, async () => {
      const plan = await select({ planId }, invocation);
      const task = plan?.tasks.find(t => t.id === taskId);
      if (!task) throw new Error("Task unavailable.");
      if (task.launch.receipt && ["pending", "uncertain"].includes(task.launch.state)) {
        await authorize(invocation, plan);
        try {
          // This is a read of native receipt identity, never a replay of threads.create.
          const result = await ctx.workflow.query({ query: "operation", operationId: task.launch.receipt.operation_id });
          await update(async current => {
            await authorize(invocation, plan);
            const stored = current.plans.find(p => p.id === planId).tasks.find(t => t.id === taskId);
            if (result.operation_id !== stored.launch.receipt.operation_id) throw new Error("Receipt identity changed.");
            stored.launch = journalReceipt(result, stored.launch);
          });
        } catch {
          ctx.log("warn", "Receipt unavailable; inspect native threads. This task will not be launched again.");
        }
      }
      return { data: await present(await select({ planId }, invocation), invocation) };
    });
  };
  const acknowledge = async (input, invocation) => {
    human(invocation);
    fields(input, ["planId", "taskId", "confirmUncertain"]);
    const planId = safeId(input.planId);
    const taskId = safeId(input.taskId);
    if (input.confirmUncertain !== true)
      throw new Error("Explicitly acknowledge that the task's thread may already exist.");
    await select({ planId }, invocation);
    if (starting.has(taskId)) throw new Error("This task is still starting. Wait for its outcome.");
    return serial(`plan-${planId}`, async () => {
      const plan = await select({ planId }, invocation);
      const task = plan.tasks.find((t) => t.id === taskId);
      if (!task) throw new Error("This task does not belong to the selected plan.");
      if (starting.has(taskId))
        throw new Error("This task is still starting. Wait for its outcome.");
      if (!["pending", "uncertain", "abandoned"].includes(task.launch.state))
        throw new Error("Only an uncertain launch can be acknowledged.");
      if (task.launch.state !== "abandoned") {
        const acknowledgementOperationId = text(invocation.operationId, 128, "operation id");
        await update(async (current) => {
          await authorize(invocation, plan);
          const stored = current.plans
            .find((p) => p.id === planId)
            .tasks.find((t) => t.id === taskId);
          stored.launch = {
            ...stored.launch,
            state: "abandoned",
            diagnostic: stored.launch.diagnostic ?? UNCERTAIN,
            acknowledgedAt: new Date().toISOString(),
            acknowledgementOperationId,
          };
        });
        await invalidate();
      }
      return { data: await present(await select({ planId }, invocation), invocation) };
    });
  };
  for (const view of ["launcher", "tasks"]) {
    ctx.handle(`view:${view}:read`, read);
    ctx.handle(`view:${view}:inspect`, inspect);
    ctx.handle(`view:${view}:acknowledge`, acknowledge);
    ctx.handle(`view:${view}:models`, models);
    ctx.handle(`view:${view}:start`, (input, invocation) => start(input, invocation, false));
    ctx.handle(`view:${view}:startAll`, (input, invocation) => start(input, invocation, true));
  }
  ctx.handle("status:tasks", async (_input, invocation) => {
    human(invocation);
    await authorize(invocation);
    return {
      data: {
        label: "Tasks",
        tone: "neutral",
        detail: "Review task proposals and recorded launch outcomes.",
      },
    };
  });
}
