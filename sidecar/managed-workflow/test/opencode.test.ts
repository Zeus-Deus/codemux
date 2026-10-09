import { afterEach, describe, expect, test } from "bun:test";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Host, Initialize, Json } from "../src/contract";
import { OpenCodeRuntime, OPENCODE_NATIVE_VERSION, nativeTokens, privateMcp, sessionPermissions } from "../src/opencode";

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

describe("OpenCode managed native HTTP boundary (no inference)", () => {
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
