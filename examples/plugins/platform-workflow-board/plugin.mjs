// Adapted from T3's workflow-board: a persisted observation journal precedes effects.
const KEY = "board.v1", LIMIT = 100, DELIVERIES = 2000;
const object = value => value !== null && typeof value === "object" && !Array.isArray(value);
export function activate(ctx) {
  let pending = Promise.resolve();
  const serial = operation => { const next = pending.then(operation); pending = next.catch(() => {}); return next; };
  const read = async () => {
    const saved = await ctx.storage.get(KEY);
    if (saved === null) return { version: 1, items: [], deliveries: [] };
    if (!object(saved) || saved.version !== 1 || !Array.isArray(saved.items) || !Array.isArray(saved.deliveries)
      || saved.items.length > LIMIT || saved.deliveries.length > DELIVERIES)
      throw new Error("Unsupported or damaged board data; retained for repair.");
    return saved;
  };
  const write = state => {
    if (Buffer.byteLength(JSON.stringify(state), "utf8") > 850000)
      throw new Error("Board storage is full; export/repair existing observations before adding more.");
    return ctx.storage.set(KEY, state);
  };
  const human = invocation => {
    if (invocation.caller.kind !== "client") throw new Error("Only a person can change workflow notes.");
    invocation.signal.throwIfAborted();
  };
  const invalidate = async () => {
    if (ctx.capabilities.includes("ui.changes")) {
      try { await ctx.ui.invalidate(["view:board"]); } catch { ctx.log("warn", "Board saved; refresh to read it."); }
    }
  };
  const present = (state, input = {}) => {
    const cursor = input.cursor ?? 0, limit = input.limit ?? 25;
    if (!Number.isSafeInteger(cursor) || cursor < 0 || !Number.isSafeInteger(limit) || limit < 1 || limit > 100)
      throw new Error("Invalid board page.");
    const items = []; let bytes = 0, next = cursor;
    // Leave room for the result envelope below the worker's 128,000-byte cap.
    for (; next < state.items.length && items.length < limit; next++) {
      const item = state.items[next], size = Buffer.byteLength(JSON.stringify(item), "utf8") + 1;
      if (bytes + size > 110000) break;
      items.push(item); bytes += size;
    }
    if (next === cursor && cursor < state.items.length) throw new Error("Stored board item is too large; retained for repair.");
    return { data: { items, observations: state.deliveries.length, totalItems: state.items.length,
      nextCursor: next < state.items.length ? next : null } };
  };
  const save = (text, invocation) => serial(async () => {
    human(invocation);
    if (typeof text !== "string" || !text.trim() || text.length > 8000) throw new Error("Enter a note of 1–8000 characters.");
    const state = await read();
    if (!state.items.some(item => item.id === invocation.operationId)) {
      state.items.unshift({ id: invocation.operationId, text: text.trim(), createdAt: new Date().toISOString(),
        ...(invocation.context?.threadId ? { threadId: invocation.context.threadId } : {}) });
      state.items = state.items.slice(0, LIMIT);
      await write(state); await invalidate();
    }
    return present(state);
  });
  ctx.handle("action:open-board", () => ({ intents: [{ type: "view.open", viewId: "board" }] }));
  ctx.handle("action:save-selection", (_input, invocation) => save(invocation.context?.selection, invocation));
  ctx.handle("view:board:add", (input, invocation) => save(input?.text, invocation));
  ctx.handle("view:board:list", input => serial(async () => present(await read(), input ?? {})));
  ctx.handle("view:board:remove", (input, invocation) => serial(async () => {
    human(invocation);
    if (typeof input?.id !== "string") throw new Error("A note id is required.");
    const state = await read(); state.items = state.items.filter(item => item.id !== input.id);
    await write(state); await invalidate(); return present(state);
  }));
  ctx.handle("view:board:review", async input => {
    const item = (await read()).items.find(item => item.id === input?.id);
    if (!item?.threadId) throw new Error("This note has no linked thread.");
    const thread = await ctx.threads.metadata({ threadId: item.threadId });
    return { intents: [{ type: "thread.reveal", threadId: thread.id }] };
  });
  if (ctx.capabilities.includes("events.run-finalized")) ctx.events.onRunFinalized((event, invocation) => serial(async () => {
    invocation.signal.throwIfAborted();
    if (!object(event) || typeof event.deliveryId !== "string" || typeof event.threadId !== "string") throw new Error("Invalid committed event.");
    const state = await read();
    let observation = state.deliveries.find(item => item.id === event.deliveryId);
    if (!observation) {
      // Refuse overflow rather than forget identities and repeat automation on replay.
      if (state.deliveries.length >= DELIVERIES) throw new Error("Observation journal is full; export/repair it before replaying events.");
      observation = { id: event.deliveryId, threadId: event.threadId, runId: event.runId, outcome: event.outcome };
      state.deliveries.push(observation);
      state.items.unshift({ id: `run-${event.deliveryId}`, threadId: event.threadId, runId: event.runId,
        text: event.summary || `Run ${event.outcome}.`, outcome: event.outcome, createdAt: new Date().toISOString() });
      state.items = state.items.slice(0, LIMIT);
      if (event.effectsAllowed === true && event.outcome === "completed" && ctx.capabilities.includes("threads.send")
        && (await ctx.settings.get("autoFollowup")) === true) {
        observation.followup = { state: "pending", request: { threadId: event.threadId,
          idempotencyKey: "completion-review", text: "Review the previous result for correctness. Report any issue concisely; do not edit files or request another follow-up." } };
      }
      await write(state);
    }
    if (observation.followup?.state === "pending" && event.effectsAllowed === true && ctx.capabilities.includes("threads.send")
      && (await ctx.settings.get("autoFollowup")) === true) {
      // A repeated durable delivery has the same host operation/principal. The host receipt
      // recovers a committed send after a crash; unlike a new UI invocation this is a safe replay.
      invocation.signal.throwIfAborted();
      const receipt = await ctx.threads.send(structuredClone(observation.followup.request));
      observation.followup = { ...observation.followup, state: "recorded", receipt };
      await write(state);
    }
    await invalidate();
    return present(state);
  }));
}
