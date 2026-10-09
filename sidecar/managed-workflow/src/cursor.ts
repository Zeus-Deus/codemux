import { mkdir } from "node:fs/promises";
import { isAbsolute, join } from "node:path";
import type { AgentOptions, SDKAgent, Run, SDKCustomTool, SDKJsonValue, SdkAuthStatus } from "@cursor/sdk";
import type { Host, Initialize, Json, Ready, Runtime } from "./contract";

export const CURSOR_ADAPTER_VERSION = "cursor-sdk-1.0.37";

/** The injectable facade exercises the actual admission options without paid calls. */
export interface CursorSdk {
  create(options: AgentOptions): Promise<SDKAgent>;
  authStatus(): Promise<SdkAuthStatus>;
  createStore(path: string): NonNullable<NonNullable<AgentOptions["local"]>["store"]>;
}

interface ActiveRun { id: string; run?: Run; cancel: boolean; done?: Promise<void> }

export class CursorRuntime implements Runtime {
  private state: "new" | "initializing" | "ready" | "failed" | "closed" = "new";
  private agent: SDKAgent | undefined;
  private storeDir: string | undefined;
  private active: ActiveRun | undefined;

  constructor(private readonly sdk: CursorSdk, private readonly host: Host,
    private readonly apiKeyPresent: () => boolean = () => Boolean(process.env.CURSOR_API_KEY?.trim())) {}

  async initialize(input: Initialize): Promise<Ready> {
    if (this.state !== "new" || input.provider !== "cursor") throw new Error("Cursor attempt cannot be reused");
    this.state = "initializing";
    try {
      if (input.effort !== null) {
        // CLI suffixes do not define SDK model parameters. Never silently drop a selection.
        throw new Error("Cursor SDK workflow effort must be selected through an SDK model variant; standalone effort is unsupported");
      }
      if (!this.apiKeyPresent() && (await this.sdk.authStatus()).status !== "logged-in") {
        throw new Error("Cursor workflows require CURSOR_API_KEY or an existing Cursor SDK login. Ordinary Cursor CLI login remains unchanged.");
      }
      const stateDir = input.options?.stateDir;
      if (typeof stateDir !== "string" || !isAbsolute(stateDir)) throw new Error("Cursor attempt requires a host-owned state directory");
      this.storeDir = join(stateDir, "cursor-store");
      await mkdir(this.storeDir, { mode: 0o700 });
      const customTools: Record<string, SDKCustomTool> = {};
      for (const tool of input.tools) {
        customTools[tool.name] = {
          description: tool.description,
          inputSchema: tool.inputSchema,
          execute: async (args) => {
            if (this.state !== "ready" || !this.active || this.active.cancel) throw new Error("Cursor attempt is no longer authorized");
            // SDK session IDs and model arguments never select the caller's authority.
            return await this.host.request("managed/tool_call", { name: tool.name, arguments: args }) as SDKJsonValue;
          },
        };
      }
      const options: AgentOptions = {
        ...(input.model === null ? {} : { model: { id: input.model } }),
        tools: ["mcp"],
        disallowedTools: ["task"],
        mcpServers: {},
        agents: {},
        local: {
          cwd: input.cwd,
          store: this.sdk.createStore(this.storeDir),
          settingSources: [],
          customTools,
          enableAgentRetries: false,
        },
      };
      this.agent = await this.sdk.create(options);
      if (this.isClosed()) {
        await this.agent[Symbol.asyncDispose]();
        throw new Error("Cursor attempt closed during initialization");
      }
      this.state = "ready";
      return {
        protocolVersion: 1, provider: "cursor", adapterVersion: CURSOR_ADAPTER_VERSION,
        sessionId: this.agent.agentId, tools: input.tools.map((tool) => tool.name),
        isolation: { nativeTools: [], nativeFanout: false, ambientConfig: false },
      };
    } catch (error) {
      if (!this.isClosed()) this.state = "failed";
      throw error;
    }
  }

  async startTurn(input: { turnId: string; prompt: string }): Promise<void> {
    if (this.state !== "ready" || !this.agent || this.active) throw new Error("Cursor attempt is not ready for a turn");
    if (!input.turnId || !input.prompt) throw new Error("Cursor turn requires an id and prompt");
    const active: ActiveRun = { id: input.turnId, cancel: false };
    this.active = active;
    active.done = this.driveTurn(active, input.prompt);
  }

  private async driveTurn(active: ActiveRun, prompt: string): Promise<void> {
    let numTurns = 0;
    const startedAt = performance.now();
    try {
      const agent = this.agent;
      if (!agent) throw new Error("Cursor agent is unavailable");
      const run = await agent.send(prompt);
      active.run = run;
      if (active.cancel || this.state !== "ready") await run.cancel();
      for await (const message of run.stream()) {
        if (active.cancel || this.state !== "ready") continue;
        if (message.type === "assistant") {
          for (const block of message.message.content) {
            if (block.type === "text") this.host.notify("text", { turnId: active.id, text: block.text });
          }
        } else if (message.type === "usage") numTurns++;
      }
      const result = await run.wait();
      if (this.state === "ready") this.host.notify("complete", {
        turnId: active.id,
        status: active.cancel || result.status === "cancelled" ? "cancelled" : result.status === "finished" ? "success" : "error",
        ...(result.error === undefined ? {} : { message: result.error.message }),
        // Token counts cannot establish a Cursor bill: plan, discounts and fees vary.
        // Omitted cost remains unknown instead of an invented $0 or rate estimate.
        usage: {
          durationMs: Math.max(0, Math.floor(result.durationMs ?? performance.now() - startedAt)), numTurns,
          ...(result.usage === undefined ? {} : { tokens: { ...result.usage } }),
          ...(result.model === undefined ? {} : { model: result.model.id }),
        },
      });
    } catch (error) {
      if (this.state === "ready") this.host.notify("complete", {
        turnId: active.id, status: "error", message: error instanceof Error ? error.message : "Cursor turn failed",
      });
    } finally {
      if (this.active === active) this.active = undefined;
    }
  }

  async cancel(): Promise<void> {
    const active = this.active;
    if (!active) return;
    active.cancel = true;
    await active.run?.cancel();
  }

  async dispose(): Promise<void> {
    if (this.state === "closed") return;
    this.state = "closed";
    await this.cancel();
    await this.agent?.[Symbol.asyncDispose]();
    await this.active?.done;
  }

  private isClosed(): boolean { return this.state === "closed"; }
}
