import { afterEach, describe, expect, test } from "bun:test";
import { mkdtemp, rm, readdir } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import type { AgentOptions, SDKAgent, SDKMessage, Run, RunResult, RunOperation, SDKCustomTool, SDKArtifact, AgentUsage } from "@cursor/sdk";
import { JsonlLocalAgentStore } from "@cursor/sdk/bundled";
import { CursorRuntime, type CursorSdk } from "../src/cursor";
import { parseInitialize, type Initialize, type Json } from "../src/contract";

const dirs: string[] = [];
afterEach(async () => { for (const path of dirs.splice(0)) await rm(path, { recursive: true, force: true }); });
async function input(): Promise<Initialize> {
  const stateDir = await mkdtemp(join(tmpdir(), "codemux-cursor-test-")); dirs.push(stateDir);
  return { provider: "cursor", cwd: stateDir, model: "selected-model", effort: null,
    options: { stateDir }, tools: [{ name: "workflow_result", description: "Report", inputSchema: { type: "object" } }] };
}

function deferred() { let release: () => void = () => {}; const promise = new Promise<void>((resolve) => { release = resolve; }); return { promise, release }; }

class FakeRun implements Run {
  readonly id = "run"; readonly agentId = "sdk-fresh"; readonly status = "finished";
  cancelled = false;
  readonly gate = deferred();
  constructor(private readonly tool: SDKCustomTool | undefined) {}
  supports(_operation: RunOperation): boolean { return true; }
  unsupportedReason(_operation: RunOperation): undefined { return undefined; }
  async *stream(): AsyncGenerator<SDKMessage> {
    await this.gate.promise;
    if (!this.cancelled && this.tool) await this.tool.execute({ value: "valid" }, { sessionId: "forged-other-session", toolCallId: "forged-call" });
    yield { type: "assistant", agent_id: this.agentId, run_id: this.id, message: { role: "assistant", content: [{ type: "text", text: "done" }] } };
  }
  async wait(): Promise<RunResult> { return { id: this.id, status: this.cancelled ? "cancelled" : "finished", durationMs: 1 }; }
  async cancel(): Promise<void> { this.cancelled = true; this.gate.release(); }
  async conversation(): Promise<[]> { return []; }
  onDidChangeStatus(): () => void { return () => {}; }
}

class FakeAgent implements SDKAgent {
  readonly agentId = "sdk-fresh"; readonly model = undefined;
  disposed = false; run: FakeRun | undefined;
  readonly sendGate = deferred();
  blockSend = false;
  constructor(private readonly options: AgentOptions) {}
  async send(): Promise<Run> {
    if (this.blockSend) await this.sendGate.promise;
    return this.run = new FakeRun(this.options.local?.customTools?.workflow_result);
  }
  close(): void { this.disposed = true; }
  async reload(): Promise<void> {}
  async [Symbol.asyncDispose](): Promise<void> { this.disposed = true; await this.run?.cancel(); this.sendGate.release(); }
  async listArtifacts(): Promise<SDKArtifact[]> { return []; }
  async downloadArtifact(): Promise<Buffer> { return Buffer.alloc(0); }
  async getUsage(): Promise<AgentUsage> { throw new Error("Usage is unavailable"); }
}

function harness(loggedIn = true) {
  let agent: FakeAgent | undefined;
  let options: AgentOptions | undefined;
  const calls: Array<{ method: string; params: Json }> = [];
  const notifications: Array<{ method: string; params: Json }> = [];
  const completed = deferred();
  const sdk: CursorSdk = {
    create: async (value) => { options = value; return agent = new FakeAgent(value); },
    authStatus: async () => loggedIn ? { status: "logged-in", backendUrl: "https://api2.cursor.sh" } : { status: "logged-out" },
    // This file store is local and never sends inference requests.
    createStore: (path) => new JsonlLocalAgentStore(path),
  };
  const runtime = new CursorRuntime(sdk, {
    request: async (method, params) => { calls.push({ method, params }); return { accepted: true }; },
    notify: (method, params) => { notifications.push({ method, params }); if (method === "complete") completed.release(); },
  }, () => false);
  return { runtime, calls, notifications, completed: completed.promise, get agent() { return agent; }, get options() { return options; } };
}

async function settle(): Promise<void> { for (let i = 0; i < 5; i++) await Promise.resolve(); }

describe("Cursor native SDK managed attempts", () => {
  test("admits only owned callbacks and disables all ambient sources and native fanout", async () => {
    const h = harness(); const request = await input();
    const ready = await h.runtime.initialize(request);
    expect(ready.tools).toEqual(["workflow_result"]);
    expect(h.options?.tools).toEqual(["mcp"]);
    expect(h.options?.disallowedTools).toEqual(["task"]);
    expect(h.options?.mcpServers).toEqual({});
    expect(h.options?.agents).toEqual({});
    expect(h.options?.local?.settingSources).toEqual([]);
    expect(h.options?.local?.enableAgentRetries).toBe(false);
    expect(h.options?.model).toEqual({ id: "selected-model" });
    expect(Object.keys(h.options?.local?.customTools ?? {})).toEqual(["workflow_result"]);
    await h.runtime.startTurn({ turnId: "host-turn", prompt: "Use managed tools" });
    await settle(); h.agent?.run?.gate.release(); await h.completed;
    expect(h.calls).toEqual([{ method: "managed/tool_call", params: { name: "workflow_result", arguments: { value: "valid" } } }]);
    const complete = h.notifications.find((item) => item.method === "complete");
    expect(complete?.params).toEqual({ turnId: "host-turn", status: "success", usage: { durationMs: 1, numTurns: 0 } });
    await expect(h.runtime.initialize(request)).rejects.toThrow("cannot be reused");
    await h.runtime.dispose();
    expect(h.agent?.disposed).toBe(true);
    // Only the parent removes state after verified process-group shutdown.
    expect(await readdir(request.cwd)).toContain("cursor-store");
  });

  test("does not silently reuse CLI auth or discard an effort selection", async () => {
    const loggedOut = harness(false);
    await expect(loggedOut.runtime.initialize(await input())).rejects.toThrow("CURSOR_API_KEY");
    expect(loggedOut.options).toBeUndefined();
    const unsupported = harness(); const request = await input(); request.effort = "high";
    await expect(unsupported.runtime.initialize(request)).rejects.toThrow("standalone effort is unsupported");
    expect(unsupported.options).toBeUndefined();
  });

  test("rejects relative attempt storage before creating an SDK agent", async () => {
    const h = harness(); const request = await input();
    request.options = { stateDir: "relative-state" };
    await expect(h.runtime.initialize(request)).rejects.toThrow("host-owned state directory");
    expect(h.options).toBeUndefined();
    expect(await readdir(request.cwd)).toEqual([]);
  });

  test("claims a turn before asynchronous send and cancels a run acquired later", async () => {
    const h = harness(); await h.runtime.initialize(await input());
    if (!h.agent) throw new Error("Missing fake agent"); h.agent.blockSend = true;
    await h.runtime.startTurn({ turnId: "host-turn", prompt: "Slow admission" });
    await expect(h.runtime.startTurn({ turnId: "duplicate", prompt: "Duplicate" })).rejects.toThrow("not ready");
    await h.runtime.cancel(); h.agent.sendGate.release(); await settle();
    expect(h.agent.run?.cancelled).toBe(true);
    expect(h.calls).toEqual([]);
    await h.runtime.dispose();
  });

  test("callbacks fail closed outside an active turn and after revocation", async () => {
    const h = harness(); await h.runtime.initialize(await input());
    const callback = h.options?.local?.customTools?.workflow_result;
    if (!callback) throw new Error("Missing callback");
    await expect(callback.execute({}, {})).rejects.toThrow("no longer authorized");
    await h.runtime.startTurn({ turnId: "host-turn", prompt: "Work" }); await settle();
    await h.runtime.cancel();
    await expect(callback.execute({}, { sessionId: "other" })).rejects.toThrow("no longer authorized");
    await h.runtime.dispose();
    expect(h.calls).toEqual([]);
  });

  test("rejects native names, duplicate tools and malformed schemas at the transport boundary", async () => {
    const request = await input();
    expect(() => parseInitialize({ ...request, tools: [{ ...request.tools[0], name: "shell" }] })).toThrow("tool definition");
    expect(() => parseInitialize({ ...request, tools: [...request.tools, ...request.tools] })).toThrow("tool definition");
    expect(() => parseInitialize({ ...request, tools: [{ ...request.tools[0], inputSchema: { type: "string" } }] })).toThrow("tool definition");
  });
});
