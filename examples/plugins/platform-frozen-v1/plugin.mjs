// Frozen isolated API v1 artifact. Do not rebuild against the current author SDK.
export async function activate(ctx) {
  let activationDenied = false;
  try { await ctx.storage.get("outside-invocation"); } catch { activationDenied = true; }
  ctx.handle("action:snapshot", async (_input, invocation) => {
    const count = (await ctx.storage.get("frozen-count")) ?? 0;
    await ctx.storage.set("frozen-count", count + 1);
    return { data: { apiVersion: ctx.apiVersion, capabilities: [...ctx.capabilities], count: count + 1,
      activationDenied, caller: invocation.caller, context: invocation.context, operationId: invocation.operationId },
      card: { viewId: "snapshot", summary: "Frozen v1 snapshot remains available as historical text.",
        data: { count: count + 1 }, installationId: "forged", generation: "forged", dataOwner: "forged" },
      intents: [{ type: "future.intent" }, { type: "view.open", viewId: "snapshot" }] };
  });
  ctx.handle("action:wait", async (_input, invocation) => {
    await ctx.storage.set("entered", true);
    await new Promise(resolve => invocation.signal.addEventListener("abort", resolve, { once: true }));
    return { message: "A cancelled invocation must not return this result." };
  });
}
