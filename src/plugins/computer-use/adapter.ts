import type { PluginContext, PluginMcpActivity, PluginMcpCallResult, PluginMcpStatus } from "@codemux/sdk";
import * as sdk from "@codemux/sdk";

export type PeekStatus = "live" | "connecting" | "stale" | "error";
export interface PeekBodyProps {
  workspaceId: string;
  size: { width: number; height: number };
  close(): void;
  onStatusChange(status: PeekStatus): void;
}

export interface PeekAdapter {
  area: string;
  open(workspaceId: string): boolean;
  close(): void;
}

export interface ProcessOptions {
  command: string;
  args?: string[];
  cwd?: string;
  env?: Record<string, string>;
  timeoutMs?: number;
  maxOutputBytes?: number;
  workspaceId?: string;
}

export interface ProcessResult {
  code: number | null;
  stdout: string;
  stderr: string;
  timedOut: boolean;
  truncated: boolean;
  cancelled: boolean;
}

export interface DriverOptions {
  id: string;
  name: string;
  scope: "plugin" | { workspaceIds: string[] };
  toolPrefix: string;
  toolDescriptions?: Record<string, { append?: string; replace?: string }>;
  shutdown: "immediate";
  transport: "stdio";
  command: string;
  args: string[];
  env: Record<string, string>;
}

export interface DriverConnection {
  call(tool: string, args?: Record<string, unknown>): Promise<PluginMcpCallResult>;
  status(): Promise<PluginMcpStatus | null>;
  onStatus(fn: (status: PluginMcpStatus | null) => void): () => void;
  onActivity(fn: (event: PluginMcpActivity) => void): () => void;
  stop(): Promise<void>;
}

export interface DesktopAdapter {
  which(command: string): Promise<string | null>;
  run(options: ProcessOptions): Promise<ProcessResult>;
  writeDataFile(name: string, contents: string): Promise<string>;
  removeDataFile(name: string): Promise<void>;
  register(options: DriverOptions): Promise<DriverConnection>;
}

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

export function peekAdapter(ctx: PluginContext): PeekAdapter | null {
  const exports: unknown = sdk;
  const host: unknown = ctx.host;
  if (!record(exports) || typeof exports.PEEK_AREA !== "string" || !record(host) || !record(host.peek)) return null;
  const peek = host.peek;
  if (typeof peek.open !== "function" || typeof peek.close !== "function") return null;
  const open = peek.open.bind(peek);
  const close = peek.close.bind(peek);
  const id = `${ctx.id}:live`;
  return {
    area: exports.PEEK_AREA,
    open: (workspaceId) => open(id, { workspaceId }) === true,
    close: () => { close(id); },
  };
}

// Keep the pending desktop/file and plugin-only MCP APIs at one checked boundary.
export function desktopAdapter(ctx: PluginContext): DesktopAdapter | null {
  const value: unknown = ctx;
  if (!record(value) || !record(value.host)) return null;
  const process = value.host.process;
  const fs = value.host.fs;
  const register = value.registerMcpServer;
  if (
    !record(process) || typeof process.which !== "function" || typeof process.run !== "function" ||
    !record(fs) || typeof fs.writeDataFile !== "function" || typeof fs.removeDataFile !== "function" ||
    typeof register !== "function"
  ) return null;
  const which = process.which.bind(process);
  const run = process.run.bind(process);
  const write = fs.writeDataFile.bind(fs);
  const remove = fs.removeDataFile.bind(fs);
  return {
    async which(command) {
      const result: unknown = await which(command);
      if (result !== null && typeof result !== "string") throw new Error("Invalid executable lookup response.");
      return result;
    },
    async run(options) {
      const result: unknown = await run(options);
      if (
        !record(result) || (result.code !== null && typeof result.code !== "number") ||
        typeof result.stdout !== "string" || typeof result.stderr !== "string" ||
        typeof result.timedOut !== "boolean" || typeof result.truncated !== "boolean" ||
        typeof result.cancelled !== "boolean"
      ) throw new Error("Invalid local process response.");
      return {
        code: result.code, stdout: result.stdout, stderr: result.stderr,
        timedOut: result.timedOut, truncated: result.truncated, cancelled: result.cancelled,
      };
    },
    async writeDataFile(name, contents) {
      const result: unknown = await write(name, contents);
      if (typeof result !== "string" || !result) throw new Error("The driver policy file could not be created.");
      return result;
    },
    async removeDataFile(name) { await remove(name); },
    async register(options) {
      const dispose: unknown = await register.call(ctx, options);
      if (typeof dispose !== "function") throw new Error("The driver registration returned no Stop control.");
      let stopping: Promise<void> | undefined;
      return {
        call: (tool, args) => ctx.host.mcp.callTool(options.id, tool, args),
        status: () => ctx.host.mcp.getStatus(options.id),
        onStatus: (fn) => ctx.host.mcp.onStatus(options.id, fn),
        onActivity: (fn) => ctx.host.mcp.onActivity(options.id, fn),
        stop() {
          stopping ??= (async () => {
            const receipt: unknown = dispose();
            if (!record(receipt) || typeof receipt.then !== "function") {
              throw new Error("This build cannot confirm that computer input has stopped.");
            }
            await receipt;
          })();
          return stopping;
        },
      };
    },
  };
}
