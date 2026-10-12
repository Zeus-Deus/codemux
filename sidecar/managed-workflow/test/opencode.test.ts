import { afterEach, describe, expect, test } from "bun:test";
import { mkdtemp, rm, writeFile, chmod } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Host, Initialize, Json } from "../src/contract";
import { OpenCodeRuntime, OPENCODE_NATIVE_VERSION, nativeTokens, privateMcp, sessionPermissions, managedConfig, nativeDeps } from "../src/opencode";

const resources: Array<() => Promise<void> | void> = [];
afterEach(async () => { for (const close of resources.splice(0).reverse()) await close(); });
const tool = { name: "workflow_submit", description: "Submit a checked result", inputSchema: { type: "object" } };

async function fixture(change?: (config: Record<string, unknown>) => void, options: { children?: boolean; missingCost?: boolean; rejectAuth?: boolean; pauseUsage?: boolean } = {}) {
  const stateDir = await mkdtemp(join(tmpdir(), "codemux-opencode-test-"));
  resources.push(() => rm(stateDir, { recursive: true, force: true }));
  const calls: Json[] = []; const notifications: Array<{ method: string; params: Json }> = [];
  let finished: ((value: Json) => void) | undefined;
  const complete = new Promise<Json>((resolve) => { finished = resolve; });
  const host: Host = { request: async (method, params) => { expect(method).toBe("managed/tool_call"); calls.push(params); return { accepted: true }; },
    notify: (method, params) => { notifications.push({ method, params }); if (method === "complete") finished?.(params); } };
  let prompts = 0; let stopped = false; let env: NodeJS.ProcessEnv = {};
  let releaseUsage: (() => void) | undefined; let observeUsage: (() => void) | undefined;
  const usageStarted = new Promise<void>((resolve) => { observeUsage = resolve; });
  const usageGate = new Promise<void>((resolve) => { releaseUsage = resolve; });
  const input: Initialize = { provider: "opencode", cwd: stateDir, model: "openai/test-model", effort: null, tools: [tool], options: { stateDir, nativeBinary: "/fake/opencode" } };
  const runtime = new OpenCodeRuntime(host, { async launch(_binary, _cwd, nativeEnv) {
    env = nativeEnv;
    const config = JSON.parse(nativeEnv.OPENCODE_CONFIG_CONTENT!) as Record<string, unknown>;
    const mcp = (config.mcp as { codemux: { url: string; headers: Record<string, string> } }).codemux;
    change?.(config);
    const server = Bun.serve({ hostname: "127.0.0.1", port: 0, async fetch(request) {
      expect(request.headers.get("authorization")).toBe(`Basic ${Buffer.from("opencode:secret").toString("base64")}`);
      const path = new URL(request.url).pathname;
      if (path === "/global/health") return Response.json({ healthy: true, version: OPENCODE_NATIVE_VERSION });
      if (path === "/config") return Response.json(config);
      if (path === "/agent") return Response.json([{ name: "codemux-workflow", mode: "primary", permission: [...sessionPermissions([tool]), { permission: "external_directory", pattern: `${stateDir}/data/opencode/tool-output/*`, action: "allow" }] }]);
      if (path === "/mcp") {
        const list = await (await fetch(mcp.url, { method: "POST", headers: mcp.headers,
          body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/list" }) })).json() as { result: { tools: unknown[] } };
        expect(list.result.tools).toEqual([tool]);
        return Response.json({ codemux: { status: "connected" } });
      }
      if (path === "/provider") return Response.json({ connected: options.rejectAuth ? [] : ["openai"], all: [{ id: "openai", models: { "test-model": { variants: { high: {} } } } }] });
      if (path === "/session") {
        const body = await request.json() as { permission: Json[] };
        return Response.json({ id: "ses_test", permission: body.permission });
      }
      if (path === "/session/ses_test/children") return Response.json(options.children ? [{ id: "ses_nativechild" }] : []);
      if (path === "/session/ses_test/abort") return Response.json(true);
      if (path === "/session/ses_test/message" && request.method === "GET") {
        observeUsage?.(); if (options.pauseUsage) await usageGate;
        return Response.json([{ info: { id: `msg_${prompts}`, role: "assistant", ...(options.missingCost ? {} : { cost: 0.02 }) } }]);
      }
      if (path === "/session/ses_test/message") {
        prompts++;
        const body = await request.json() as { agent: string; model: { providerID: string; modelID: string }; parts: unknown[] };
        expect(body.agent).toBe("codemux-workflow"); expect(body.model).toEqual({ providerID: "openai", modelID: "test-model" });
        await fetch(mcp.url, { method: "POST", headers: mcp.headers, body: JSON.stringify({ jsonrpc: "2.0", id: 2,
          method: "tools/call", params: { name: "workflow_submit", arguments: { result: 42, caller_attempt: "forged" } } }) });
        return Response.json({ info: { role: "assistant", id: `msg_${prompts}` }, parts: [{ type: "text", text: "Submitted" }] });
      }
      return new Response(null, { status: 404 });
    } });
    return { url: `http://127.0.0.1:${server.port}`, password: "secret", dead: () => stopped,
      async stop() { stopped = true; server.stop(true); } };
  } });
  resources.push(() => runtime.dispose());
  return { runtime, input, calls, notifications, complete, usageStarted, releaseUsage: () => releaseUsage?.(), prompts: () => prompts, stopped: () => stopped, env: () => env };
}

async function v2Fixture(options: { change?: (entries: unknown[], session: Record<string, unknown>) => void; pausedWait?: boolean; outcome?: string; children?: boolean; badAck?: boolean; stale?: boolean; badCursor?: boolean; pauseUsage?: boolean; variant?: string; coldPlugins?: boolean; badPlugin?: boolean; defaultVariant?: boolean; pauseCancelWait?: boolean; pauseAdmission?: boolean; nativeTool?: boolean; badWait?: boolean; extraAgent?: boolean } = {}) {
  const stateDir = await mkdtemp(join(tmpdir(), "codemux-opencode-v2-test-"));
  resources.push(() => rm(stateDir, { recursive: true, force: true }));
  const notifications: Array<{ method: string; params: Json }> = [];
  let resolveComplete: (value: Json) => void = () => {};
  let textCallback: (() => void) | undefined;
  const completions: Json[] = []; const completionWaiters = new Map<number, (value: Json) => void>();
  const complete = new Promise<Json>((resolve) => { resolveComplete = resolve; });
  const host: Host = { request: async () => { throw new Error("No host calls in readiness"); }, notify: (method, params) => {
    notifications.push({ method, params }); if (method === "text") textCallback?.(); if (method === "complete") { completions.push(params); resolveComplete(params); completionWaiters.get(completions.length)?.(params); }
  } };
  const input: Initialize = { provider: "opencode", cwd: stateDir, model: "openai/test-model", effort: options.variant ?? null, tools: [tool], options: { stateDir, nativeBinary: "/fake/v2" } };
  let env: NodeJS.ProcessEnv = {}; let stopped = false; let prompts = 0; let interrupts = 0; let waitCalls = 0;
  let releaseWait: () => void = () => {}; let sawWait: () => void = () => {};
  const waiting = new Promise<void>((resolve) => { sawWait = resolve; });
  const waitGate = new Promise<void>((resolve) => { releaseWait = resolve; });
  let releaseUsage: () => void = () => {}; let sawUsage: () => void = () => {};
  const usageStarted = new Promise<void>((resolve) => { sawUsage = resolve; });
  const usageGate = new Promise<void>((resolve) => { releaseUsage = resolve; });
  let releaseCancel: () => void = () => {}; let sawCancel: () => void = () => {};
  const cancelWaiting = new Promise<void>((resolve) => { sawCancel = resolve; });
  const cancelGate = new Promise<void>((resolve) => { releaseCancel = resolve; });
  let releaseAdmission: () => void = () => {}; let sawAdmission: () => void = () => {};
  const admissionStarted = new Promise<void>((resolve) => { sawAdmission = resolve; });
  const admissionGate = new Promise<void>((resolve) => { releaseAdmission = resolve; });
  let interruptedBeforeAdmission = false;
  const paths: string[] = []; let pluginReads = 0; let promptID = "";
  const runtime = new OpenCodeRuntime(host, { version: async () => "2.0.27", async launch(_binary, _cwd, nativeEnv) {
    env = nativeEnv;
    const config = JSON.parse(nativeEnv.OPENCODE_CONFIG_CONTENT!);
    const entries: unknown[] = [{ type: "directory", path: join(stateDir, "config/opencode") }, { type: "document", info: config }];
    const session: Record<string, unknown> = { id: "ses_v2test", location: { directory: stateDir }, agent: "codemux-workflow", model: { providerID: "openai", id: "test-model", ...(input.effort ? { variant: input.effort } : options.defaultVariant ? { variant: "default" } : {}) }, permissions: sessionPermissions([tool], "2.0.27") };
    options.change?.(entries, session);
    const server = Bun.serve({ hostname: "127.0.0.1", port: 0, async fetch(request) {
      expect(request.headers.get("authorization")).toBe(`Basic ${Buffer.from("opencode:secret").toString("base64")}`);
      const url = new URL(request.url); const path = url.pathname; paths.push(request.method + " " + path);
      const envelope = (data: unknown) => Response.json({ location: { directory: stateDir }, data });
      if (path === "/api/info") return Response.json({ version: "2.0.27", pid: 123 });
      if (path === "/api/config") return Response.json(entries);
      if (path === "/api/plugin") {
        pluginReads++;
        return envelope(options.coldPlugins && pluginReads === 1 ? [] : ["opencode.config.agent", "opencode.config.mcp", "opencode.config.policy", "opencode.config.provider"].map((id) => ({ id, source: { type: "builtin" }, state: { status: options.badPlugin ? "failed" : "active" } })));
      }
      if (path === "/api/agent") return envelope([{ id: "codemux-workflow", mode: "primary", permissions: sessionPermissions([tool], "2.0.27") }, ...(options.extraAgent ? [{ id: "ambient", mode: "primary", permissions: [] }] : [])]);
      if (path === "/api/mcp") return envelope([{ name: "codemux", status: { status: "connected" } }]);
      if (path === "/api/provider") return envelope([{ id: "openai", activation: "auto", package: "@opencode/ai/providers/openai" }]);
      if (path === "/api/model") return envelope([{ providerID: "openai", id: "test-model", enabled: true, variants: [{ id: "high" }] }]);
      if (path === "/api/session" && request.method === "POST") {
        expect(await request.json()).toMatchObject({ location: { directory: stateDir }, model: { providerID: "openai", id: "test-model" }, agent: "codemux-workflow", permissions: sessionPermissions([tool], "2.0.27") });
        return Response.json({ data: session });
      }
      if (path === "/api/session/ses_v2test") return Response.json({ data: { ...session, ...(prompts ? { outcome: options.outcome ?? "succeeded", time: { idle: 1 } } : {}) } });
      if (path === "/api/session") return Response.json({ data: options.children ? [{ id: "ses_child" }] : [], cursor: {} });
      if (path === "/api/session/ses_v2test/prompt") {
        const body = await request.json() as { id: string; text: string };
        sawAdmission(); if (options.pauseAdmission) await admissionGate;
        prompts++; promptID = body.id; expect(body.text).toBe("Test");
        return Response.json({ data: { id: options.badAck ? "msg_foreign" : body.id, sessionID: "ses_v2test", type: "user", payload: { text: body.text } } });
      }
      if (path === "/api/experimental/session/ses_v2test/wait") {
        expect(request.method).toBe("POST"); waitCalls++; sawWait(); if (interrupts && options.pauseCancelWait) { sawCancel(); await cancelGate; } else if (options.pausedWait) await waitGate;
        return options.badWait ? Response.json({}) : new Response(null, { status: 204 });
      }
      if (path === "/api/session/ses_v2test/interrupt") {
        expect(request.method).toBe("POST"); expect(url.searchParams.get("resume")).toBe("false"); interruptedBeforeAdmission ||= prompts === 0; interrupts++; releaseWait();
        return Response.json({ interrupted: prompts > 0 });
      }
      if (path === "/api/session/ses_v2test/message") {
        const currentPrompt = paths.filter((p) => p.endsWith("/prompt")).length;
        if (!currentPrompt) return Response.json({ data: options.stale ? [{ id: "msg_old", type: "assistant", content: [{ type: "text", text: "Old" }], tokens: { input: 999, output: 999, reasoning: 0, cache: { read: 0, write: 0 } } }] : [], cursor: {} });
        sawUsage(); if (options.pauseUsage) await usageGate;
        if (url.searchParams.has("cursor")) return Response.json({ data: [{ id: `msg_idle${prompts}`, type: "idle", outcome: options.outcome ?? "succeeded" }], cursor: options.badCursor ? { next: "next" } : {} });
        return Response.json({ data: [{ id: promptID, type: "user", text: "Test" }, { id: `msg_assistant${prompts}`, type: "assistant", content: [{ type: "text", text: "Submitted" }, ...(options.nativeTool ? [{ type: "tool", name: "shell", state: { status: "completed" } }] : [])], tokens: { input: 2, output: 3, reasoning: 4, cache: { read: 5, write: 6 } } }], cursor: { next: "next" } });
      }
      return new Response(null, { status: 404 });
    } });
    return { url: `http://127.0.0.1:${server.port}`, password: "secret", dead: () => stopped, async stop() { stopped = true; server.stop(true); } };
  } });
  resources.push(() => runtime.dispose());
  return { runtime, input, notifications, completions, complete, onText: (callback: () => void) => { textCallback = callback; }, completion: (count: number) => completions[count - 1] ? Promise.resolve(completions[count - 1]!) : new Promise<Json>((resolve) => { completionWaiters.set(count, resolve); }), paths, admissionStarted, releaseAdmission: () => releaseAdmission(), interruptedBeforeAdmission: () => interruptedBeforeAdmission, cancelWaiting, releaseCancel: () => releaseCancel(), waiting, releaseWait: () => releaseWait(), usageStarted, releaseUsage: () => releaseUsage(), env: () => env, stopped: () => stopped, prompts: () => prompts, interrupts: () => interrupts, waitCalls: () => waitCalls };
}

describe("OpenCode managed native HTTP boundary (no inference)", () => {
  test.skipIf(process.platform !== "linux")("native v2 version banner is exact and the owned server reports reaped after stop", async () => {
    const dir = await mkdtemp(join(tmpdir(), "codemux-native-owner-"));
    resources.push(() => rm(dir, { recursive: true, force: true }));
    const binary = join(dir, "native-fixture");
    await writeFile(binary, `#!${process.execPath}\nif (process.argv.includes("--version")) { console.log("opencode v2.0.27"); process.exit(0); }\nconst s = Bun.serve({hostname:"127.0.0.1",port:0,fetch:()=>new Response(null)});\nconsole.log("server listening on http://127.0.0.1:" + s.port);\n`);
    await chmod(binary, 0o700);
    const env = { PATH: "/usr/bin:/bin", HOME: dir, USERPROFILE: dir, TMPDIR: dir, TEMP: dir, TMP: dir };
    expect(await nativeDeps.version!(binary, dir, env)).toBe("2.0.27");
    const native = await nativeDeps.launch(binary, dir, env); resources.push(() => native.stop());
    expect(native.dead()).toBe(false);
    await native.stop(); expect(native.dead()).toBe(true);
    await writeFile(binary, `#!${process.execPath}\nconsole.log("opencode v2.0.28");\n`);
    await expect(nativeDeps.version!(binary, dir, env)).rejects.toThrow("reviewed");
  });
  test("v2 awaits cold policy/config plugin activation before using available auto providers", async () => {
    const f = await v2Fixture({ coldPlugins: true });
    await f.runtime.initialize(f.input);
    expect(f.paths.filter((p) => p === "GET /api/plugin").length).toBe(2);
    expect(managedConfig(f.input, "http://127.0.0.1:1/mcp", "synthetic", "2.0.27").plugins).toContain("-opencode.browser");
  });
  test("v2 verifies the native catalog default variant without accepting an arbitrary substitution", async () => {
    const f = await v2Fixture({ defaultVariant: true });
    expect(await f.runtime.initialize(f.input)).toMatchObject({ sessionId: "ses_v2test" });
  });
  test("v2 prompt admission waits for 204 quiescence and reconciles only this turn's paginated history", async () => {
    const f = await v2Fixture({ pausedWait: true, stale: true });
    await f.runtime.initialize(f.input); await f.runtime.startTurn({ turnId: "v2-turn", prompt: "Test" });
    await Promise.race([f.waiting, f.complete.then(() => { throw new Error("Prompt ACK must not settle the turn"); })]);
    expect(f.completions).toEqual([]); expect(f.notifications).toEqual([]);
    f.releaseWait();
    expect(await f.complete).toMatchObject({ status: "success", turnId: "v2-turn", usage: { numTurns: 1, tokens: { inputTokens: 2, outputTokens: 7, reasoningTokens: 4, cacheReadTokens: 5, cacheWriteTokens: 6 } } });
    expect(f.notifications.filter((n) => n.method === "text")).toEqual([{ method: "text", params: { turnId: "v2-turn", text: "Submitted" } }]);
    expect(f.paths).toContain("POST /api/experimental/session/ses_v2test/wait");
  });
  test("v2 cancellation cannot overtake unconfirmed prompt admission", async () => {
    const f = await v2Fixture({ pauseAdmission: true, outcome: "interrupted" });
    await f.runtime.initialize(f.input);
    await f.runtime.startTurn({ turnId: "admission-cancel", prompt: "Test" });
    await f.admissionStarted;
    const cancel = f.runtime.cancel();
    try {
      const early = await Promise.race([
        cancel.then(() => true),
        new Promise<false>((resolve) => setTimeout(() => resolve(false), 100)),
      ]);
      expect(early).toBe(false);
      expect(f.interrupts()).toBe(0);
      expect(f.completions).toEqual([]);
    } finally { f.releaseAdmission(); await cancel; }
    expect(f.interruptedBeforeAdmission()).toBe(false);
    expect(await f.complete).toMatchObject({ status: "cancelled", turnId: "admission-cancel" });
    expect(f.notifications.filter((n) => n.method === "text")).toEqual([]);
  });

  test("v2 cancellation retires the owner when pending admission fails verification", async () => {
    const f = await v2Fixture({ pauseAdmission: true, badAck: true });
    await f.runtime.initialize(f.input);
    await f.runtime.startTurn({ turnId: "bad-admission-cancel", prompt: "Test" });
    await f.admissionStarted;
    const cancel = f.runtime.cancel();
    const result = cancel.then(() => "resolved", () => "rejected");
    f.releaseAdmission();
    expect(await result).toBe("rejected");
    expect(f.stopped()).toBe(true);
    expect(f.interrupts()).toBe(0);
  });

  test("v2 interruption retains turn ownership until acknowledged native quiescence", async () => {
    const f = await v2Fixture({ pausedWait: true, pauseCancelWait: true, outcome: "interrupted" });
    await f.runtime.initialize(f.input); await f.runtime.startTurn({ turnId: "v2-cancel", prompt: "Test" }); await f.waiting;
    const cancel = f.runtime.cancel();
    await Promise.race([f.cancelWaiting, cancel.then(() => { throw new Error("Cancel must await native quiescence"); })]);
    expect(f.completions).toEqual([]);
    await expect(f.runtime.startTurn({ turnId: "overlap", prompt: "Test" })).rejects.toThrow("unavailable");
    f.releaseCancel(); await cancel;
    expect(await f.complete).toMatchObject({ status: "cancelled", turnId: "v2-cancel" });
    expect(f.interrupts()).toBe(1); expect(f.waitCalls()).toBe(2);
    expect(f.notifications.filter((n) => n.method === "text")).toEqual([]);
  });
  test.each(["badAck", "badWait", "badCursor", "children", "nativeTool", "outcome"] as const)("v2 retires the owned runtime on %s before reporting error", async (key) => {
    const f = await v2Fixture(key === "outcome" ? { outcome: "failed" } : { [key]: true });
    await f.runtime.initialize(f.input); await f.runtime.startTurn({ turnId: "unsafe-turn", prompt: "Test" });
    expect(await f.complete).toMatchObject({ status: "error" });
    expect(f.stopped()).toBe(true);
    expect(f.notifications.filter((n) => n.method === "text")).toEqual([]);
    await expect(f.runtime.startTurn({ turnId: "after-error", prompt: "Test" })).rejects.toThrow("unavailable");
  });
  test("readiness metadata declares both exact reviewed native pins while retaining the v1 adapter", async () => {
    const child = Bun.spawn([process.execPath, join(import.meta.dir, "../src/main.ts"), "--check"], { stdout: "pipe", stderr: "pipe" });
    resources.push(async () => { child.kill("SIGKILL"); await child.exited; });
    const text = await new Response(child.stdout).text();
    expect(await child.exited).toBe(0);
    expect(JSON.parse(text)).toMatchObject({ openCodeAdapterVersion: "opencode-managed-v1", openCodeNativeVersion: "1.18.35", openCodeNativeVersions: ["1.18.35", "2.0.27"] });
  });
  test("v2 rejects an extra resolved agent despite a valid managed one", async () => {
    const f = await v2Fixture({ extraAgent: true });
    await expect(f.runtime.initialize(f.input)).rejects.toThrow("managed allowlist");
    expect(f.stopped()).toBe(true); expect(f.prompts()).toBe(0);
  });
  test("v2 rejects noncanonical source paths beneath the owned config root", async () => {
    const f = await v2Fixture({ change: (entries) => { const directory = entries[0] as { path: string }; (entries[1] as { path: string }).path = directory.path + "/../../ambient.json"; } });
    await expect(f.runtime.initialize(f.input)).rejects.toThrow("isolated managed configuration");
    expect(f.stopped()).toBe(true); expect(f.prompts()).toBe(0);
  });
  test("v2 cancel during final history accounting cannot emit text or become success", async () => {
    const f = await v2Fixture({ pauseUsage: true });
    await f.runtime.initialize(f.input); await f.runtime.startTurn({ turnId: "accounting-cancel", prompt: "Test" }); await f.usageStarted;
    await f.runtime.cancel(); f.releaseUsage();
    expect(await f.complete).toMatchObject({ status: "cancelled", turnId: "accounting-cancel" });
    expect(f.notifications.filter((n) => n.method === "text")).toEqual([]);
  });
  test("v2 usage and text deduplicate old messages across consecutive turns", async () => {
    const f = await v2Fixture(); await f.runtime.initialize(f.input);
    for (let index = 1; index <= 2; index++) {
      await f.runtime.startTurn({ turnId: `turn-${index}`, prompt: "Test" });
      expect(await f.completion(index)).toMatchObject({ status: "success", usage: { numTurns: 1, tokens: { inputTokens: 2, outputTokens: 7 } } });
    }
    expect(f.notifications.filter((n) => n.method === "text")).toHaveLength(2);
    expect(JSON.stringify(f.completions)).not.toContain("totalCostUsd");
  });
  test.each(["permissions", "mcp", "plugins", "instructions", "experimental", "agents", "skills"])("v2 rejects ambient %s in source documents before creating a session", async (key) => {
    const f = await v2Fixture({ change: (entries) => { (entries[1] as { info: Record<string, unknown> }).info[key] = {}; } });
    await expect(f.runtime.initialize(f.input)).rejects.toThrow("isolated managed configuration");
    expect(f.stopped()).toBe(true); expect(f.prompts()).toBe(0); expect(f.paths).not.toContain("POST /api/session");
  });
  test.each(["permissions", "agent", "model", "location"])("v2 rejects substituted final session %s before prompts", async (key) => {
    const f = await v2Fixture({ change: (_entries, session) => { session[key] = {}; } });
    await expect(f.runtime.initialize(f.input)).rejects.toThrow("exact final session");
    expect(f.stopped()).toBe(true); expect(f.prompts()).toBe(0);
  });
  test("v2 does not accept an arbitrary default variant substitution", async () => {
    const f = await v2Fixture({ change: (_entries, session) => { (session.model as Record<string, unknown>).variant = "foreign"; } });
    await expect(f.runtime.initialize(f.input)).rejects.toThrow("exact final session");
  });
  test("v2 rejects unavailable effort rather than forwarding an unadvertised variant", async () => {
    const f = await v2Fixture({ variant: "foreign" });
    await expect(f.runtime.initialize(f.input)).rejects.toThrow("effort variant is unavailable");
    expect(f.prompts()).toBe(0);
  });
  test("v2 failed built-in activation is not retried or tolerated", async () => {
    const f = await v2Fixture({ badPlugin: true });
    await expect(f.runtime.initialize(f.input)).rejects.toThrow("plugin isolation failed");
    expect(f.stopped()).toBe(true); expect(f.prompts()).toBe(0);
  });
  test("v2 cancellation during terminal accounting wins before the complete notification", async () => {
    const f = await v2Fixture(); let cancel: Promise<void> | undefined;
    f.onText(() => queueMicrotask(() => { cancel = f.runtime.cancel(); void cancel.catch(() => {}); }));
    await f.runtime.initialize(f.input); await f.runtime.startTurn({ turnId: "late-cancel", prompt: "Test" });
    expect(await f.complete).toMatchObject({ status: "cancelled" });
    await cancel; expect(f.interrupts()).toBe(1);
  });
  test("v2 config selection uses config model, not the session ref id field", async () => {
    const f = await fixture();
    expect(managedConfig(f.input, "http://127.0.0.1:1/mcp", "synthetic", "2.0.27").model).toEqual({ providerID: "openai", model: "test-model" });
  });
  test("v2 readiness verifies native catalogs and exact persisted session before prompts", async () => {
    const f = await v2Fixture({ variant: "high" });
    expect(await f.runtime.initialize(f.input)).toMatchObject({ sessionId: "ses_v2test", tools: [tool.name] });
    expect(f.prompts()).toBe(0);
    expect(f.paths).toContain("GET /api/session/ses_v2test");
    expect(f.env()).toMatchObject({ HOME: join(f.input.cwd, "home"), USERPROFILE: join(f.input.cwd, "home"), TMP: join(f.input.cwd, "tmp"), TEMP: join(f.input.cwd, "tmp"), OPENCODE_DISABLE_FILEWATCHER: "true" });
  });
  test("v2 config uses native rules and disables MCP code mode without relaxing native denies", async () => {
    const f = await fixture();
    const config = managedConfig(f.input, "http://127.0.0.1:1/mcp", "synthetic", "2.0.27");
    expect(config.permissions).toEqual([
      { action: "*", resource: "*", effect: "deny" },
      { action: "external_directory", resource: "*", effect: "deny" },
      { action: "codemux_workflow_submit", resource: "*", effect: "allow" },
    ]);
    expect(config.model).toEqual({ providerID: "openai", model: "test-model" });
    expect(config.mcp).toMatchObject({ servers: { codemux: { oauth: false, disabled: false, codemode: false } } });
    expect(config).toMatchObject({ update: "disable", snapshots: false, compaction: { auto: false }, skills: [] });
    expect(config.plugins).toContain("-opencode.tool.subagent");
    expect(config).not.toHaveProperty("permission");
    expect(config.compaction).not.toHaveProperty("prune");
    expect(() => managedConfig(f.input, "http://127.0.0.1:1/mcp", "synthetic", "2.0.28")).toThrow("reviewed");
  });
  test("confirms exact session rules and private tools before allowing a turn", async () => {
    const f = await fixture();
    const ready = await f.runtime.initialize(f.input);
    expect(ready.tools).toEqual(["workflow_submit"]); expect(ready.isolation.nativeFanout).toBe(false);
    expect(f.prompts()).toBe(0);
    expect(f.env().OPENCODE_DISABLE_PROJECT_CONFIG).toBe("true");
    expect(f.env().OPENCODE_TEST_HOME).toBe(join(String(f.input.options!.stateDir), "home"));
    expect(f.env().OPENCODE_DISABLE_DEFAULT_PLUGINS).toBe("true");
    await f.runtime.startTurn({ turnId: "turn-1", prompt: "Test" });
    const result = await f.complete;
    expect(result).toMatchObject({ turnId: "turn-1", status: "success", usage: { numTurns: 1 } });
    expect(JSON.stringify(result)).not.toContain("totalCostUsd");
    expect(f.calls).toEqual([{ name: "workflow_submit", arguments: { result: 42, caller_attempt: "forged" } }]);
    expect(f.notifications[0]).toEqual({ method: "text", params: { turnId: "turn-1", text: "Submitted" } });
  });

  test.each(["permission", "mcp", "plugin", "instructions"])("rejects ambient %s configuration before inference", async (key) => {
    const f = await fixture((config) => { config[key] = key === "plugin" || key === "instructions" ? ["ambient"] : {}; });
    await expect(f.runtime.initialize(f.input)).rejects.toThrow("isolated managed configuration");
    expect(f.prompts()).toBe(0); expect(f.stopped()).toBe(true);
  });

  test("rejects unavailable native authentication before prompting", async () => {
    const f = await fixture(undefined, { rejectAuth: true });
    await expect(f.runtime.initialize(f.input)).rejects.toThrow("environment API key"); expect(f.prompts()).toBe(0);
  });

  test("unmanaged native children fail the attempt", async () => {
    const f = await fixture(undefined, { children: true });
    await f.runtime.initialize(f.input); await f.runtime.startTurn({ turnId: "child-turn", prompt: "Test" });
    expect(await f.complete).toMatchObject({ status: "error" });
  });

  test("acknowledged cancel during final transcript accounting cannot become success", async () => {
    const f = await fixture(undefined, { pauseUsage: true });
    await f.runtime.initialize(f.input);
    await f.runtime.startTurn({ turnId: "cancelled-turn", prompt: "Test" });
    await f.usageStarted;
    await f.runtime.cancel(); f.releaseUsage();
    expect(await f.complete).toMatchObject({ status: "cancelled", turnId: "cancelled-turn" });
  });

  test("missing cost stays unknown, and native rate estimates are never bills", async () => {
    const f = await fixture(undefined, { missingCost: true });
    await f.runtime.initialize(f.input); await f.runtime.startTurn({ turnId: "cost-turn", prompt: "Test" });
    const result = await f.complete;
    expect(result).toMatchObject({ status: "success", usage: { numTurns: 1 } });
    expect(JSON.stringify(result)).not.toContain("totalCostUsd");
  });

  test("native token buckets normalize reasoning without inventing missing counts", () => {
    expect(nativeTokens({ input: 2, output: 3, reasoning: 4, cache: { read: 5, write: 6 } })).toEqual({
      inputTokens: 2, outputTokens: 7, reasoningTokens: 4, cacheReadTokens: 5, cacheWriteTokens: 6,
    });
    expect(nativeTokens({ input: 2, output: 3 })).toBeUndefined();
    expect(nativeTokens({ input: -1, output: 3, reasoning: 0, cache: { read: 5, write: 6 } })).toBeUndefined();
  });

  test("private MCP refuses forged tools, missing authentication and oversized calls", async () => {
    const calls: Json[] = [];
    const mcp = privateMcp({ request: async (_method, params) => { calls.push(params); return {}; }, notify: () => {} }, [tool]);
    resources.push(() => mcp.stop());
    const body = JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/call", params: { name: "bash", arguments: {} } });
    expect((await fetch(mcp.url, { method: "POST", body })).status).toBe(401);
    const forged = await (await fetch(mcp.url, { method: "POST", headers: { Authorization: `Bearer ${mcp.token}` }, body })).json();
    expect(forged).toMatchObject({ error: { code: -32602 } });
    let oversizedResponse: Response | undefined;
    try {
      oversizedResponse = await fetch(mcp.url, { method: "POST", headers: { Authorization: `Bearer ${mcp.token}` },
        body: "x".repeat(2 * 1024 * 1024 + 1) });
    } catch (error) {
      // The pinned Windows runtime can reset an oversized POST before its 413
      // reaches fetch. Require that exact observed error and a healthy endpoint.
      if (process.platform !== "win32" || Bun.version !== "1.3.12"
          || !(error instanceof Error) || !("code" in error) || error.code !== "ECONNRESET") throw error;
    }
    if (oversizedResponse) expect(oversizedResponse.status).toBe(413);
    expect(calls).toEqual([]);
    const healthy = await fetch(mcp.url, { method: "POST", headers: { Authorization: `Bearer ${mcp.token}` },
      body: JSON.stringify({ jsonrpc: "2.0", id: 2, method: "tools/list" }) });
    expect(healthy.status).toBe(200);
    expect(await healthy.json()).toEqual({ jsonrpc: "2.0", id: 2, result: { tools: [tool] } });
    expect(calls).toEqual([]);
  });
});
