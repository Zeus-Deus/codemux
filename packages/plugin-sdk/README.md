# @codemux/plugin-sdk

The dependency-free TypeScript contract for **isolated API v1** plugins. This is separate from the existing React renderer API `@codemux/sdk`. Existing React and MCP plugins keep their existing loaders.

Import author types while developing; the worker supplies the live context. There is no browser/Tauri import and no runtime bootstrap dependency:

```ts
import type { PluginModule } from '@codemux/plugin-sdk';
export const activate: PluginModule['activate'] = ctx => {
  ctx.handle('action:inspect', async (_input, invocation) => {
    invocation.signal.throwIfAborted();
    if (invocation.caller.kind !== 'agent' || !invocation.caller.threadId)
      throw new Error('A verified agent thread is required.');
    const thread = await ctx.threads.metadata({threadId: invocation.caller.threadId});
    return {message: thread.title ?? 'Untitled thread'};
  });
};
```

Compile TypeScript before packaging. A plugin directory contains `codemux-plugin.json`, a JavaScript `.mjs`/`.js` entry exporting `activate(ctx)`, and its bundled or relative dependencies and HTML assets. Installation never runs npm/install/build hooks. The checked-in examples require no SDK installation or build.

`capabilities` and `requiredCapabilities` are required feature requests. Unsupported API versions, required capabilities or `requires` clauses make an installation unavailable without importing it. Unknown optional capabilities remain inspectable but are absent from the live **supported and granted** `ctx.capabilities`. Test that array before using optional APIs. A method's presence alone proves neither capability nor project authority. `workflow.query`, metadata/model listings and thread reads require their relevant grants; thread mutations require explicit project grants and the caller's current native permission ceiling.

The current host gates `projects.list`, `workspaces.list`, `threads.list` and `models.list` with both `workflow.query` and `threads.read`. Metadata/transcript reads require `threads.read`; storage and declared settings require their own capabilities. Create/send require their respective grants, current project scope and native caller authority; inheriting a source model also requires readable verified source metadata. A worktree adds `worktrees.create`. Cards/views and change signals require their declared UI capabilities. Grants are checked again during host calls, so a snapshot in activation is discovery rather than permanent authority.

Register handlers synchronously during activation (or before `activate` resolves). **Host calls work only inside a live invocation**, including tree and committed-event handlers. Activation, deactivation, timers and escaped continuations have no host authority. Await host calls before returning. `ctx.signal` is worker lifetime; `invocation.signal` ends on cancellation, deadline, return or revocation. Cancellation revokes outstanding authority; it cannot undo an effect the core already admitted. Private `ctx.dataDirectory` and JSON storage survive updates/restarts and removal unless the person explicitly deletes plugin data. Treat unknown stored versions as unavailable and retain the bytes for repair.

`invocation.caller` is frozen, host-verified identity. `context` is the requested target and authorizes nothing. An agent's `caller.threadId` exists only after native verification; an outside client cannot obtain one by naming a target. Human controls must require `caller.kind === 'client'`; an input payload cannot promote an agent to a person. `subagentOf` and `lineage` are host-derived ancestry hints, not new authority. An unresolved lineage must not be guessed into an owner.

Mutation identity includes installation, stable host invocation `operationId`, authenticated principal, and your stable `idempotencyKey`, independently of code generation. Changed inputs for that identity conflict. Different operation IDs or sessions do not prove a replay. Persist the immutable request **before** dispatch. After a lost reply retain pending/uncertain state and inspect; never issue a new create to discover whether the first succeeded. `threads.create/send` return native snake_case `PluginOperationReceipt`. `accepted`/`running` mean pending, `succeeded` acknowledges the native command, and `needs_attention` or an unknown state requires inspection. None proves the agent finished its work. Read a known receipt using `workflow.query({query:'operation', operationId: receipt.operation_id})`; the host checks current grants and installation/principal ownership. Within the original stable invocation operation, `workflow.query({query:'effect', idempotencyKey: originalKey})` reads that mutation's receipt; a missing receipt throws `not_found` and is not proof that retrying is safe. Read `query:'thread-status'` or bounded transcript pages to inspect subsequent work. A receipt unavailable to the current session remains uncertain.

Model selections use canonical `{instanceId,model,options:[{id,value}]}`. Create also accepts legacy option maps. Discover `models.list` for actual models, options and `permissionModes`; do not invent provider settings. Use `nativePermissionMode` for an advertised native policy. Portable `runtimeMode` has only reviewed mappings; `auto` is explicitly unsupported. A worktree choice requires its grant, a base ref, and optional branch/startFromOrigin. Local refs use the local commit; `startFromOrigin:true` explicitly fetches origin and requires success. Unknown worktree outcomes are retained for inspection without duplicate creation.

Committed `run.finalized` delivery is at least once. Persist `deliveryId` and stable follow-up intent, tolerate duplicate delivery after restart, and retain a bounded journal without silently evicting unresolved effects. Only `effectsAllowed:true` supplies verified input provenance for automation; completion alone does not. Automatic effects also require opt-in settings, capabilities and project authority. The host excludes plugin-caused runs and caps per-delivery effects. Disabled/incompatible gaps require the person's replay/skip choice.

A card returned by an author contains only `viewId`, historical plain-text `summary` (1,000 characters) and small JSON reference `data` (2,048 UTF-8 bytes). The host stamps `installationId`, `generation` and `dataOwner`; they are not author input. Retain summary text in history so disabled, missing, broken or incompatible plugins do not erase the proposal. Live state belongs in private storage. Native trees return project/thread IDs so the client renders real rows; page metadata and bound scans/output rather than reading every transcript.

HTML views use injected `window.codemux` (`PluginViewBridge`), with a compatibility `window.t3` alias. Check `features` and optional `onChanged`/`intent`/`resize`; refresh remains a fallback. View invokes can reach only that view's handlers. The bridge, context, card data and local intents do not confer backend authority. Views are single HTML documents with inline scripts/styles; network loading is blocked. The isolated worker is a separate process, **not** an OS filesystem/network sandbox.

From the repository root:

```sh
node node_modules/typescript/bin/tsc -p packages/plugin-sdk/tsconfig.json
node --test packages/plugin-sdk/test/*.test.mjs examples/plugins/platform-*/test/*.test.mjs
CODEMUX_PLUGIN_HOST_TEST_BINARY=./src-tauri/binaries/codemux-plugin-host-x86_64-unknown-linux-gnu node --test examples/plugins/platform-*/test/*.test.mjs
```

The worker tests use persisted deterministic host fixtures; native integration tests separately verify grants, receipts, committed provenance and host stamps. `platform-frozen-v1/plugin.mjs` is hash-pinned release input and never rebuilt against this package. To prepare this package for publication, run its build explicitly, then `npm pack --ignore-scripts`; no publication occurs automatically.
