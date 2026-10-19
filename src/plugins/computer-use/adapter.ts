import type { PluginContext, PluginMcpActivity, PluginMcpCallResult, PluginMcpStatus } from "@codemux/sdk";
import * as sdk from "@codemux/sdk";
import { createElement, type ComponentType, type ReactNode } from "react";

export type PeekStatus = "live" | "connecting" | "stale" | "error";
export interface PeekBodyProps {
  workspaceId: string;
  size: { width: number; height: number };
  close(): void;
  onStatusChange(status: PeekStatus, label?: string): void;
}

export interface PeekAdapter {
  area: string;
  composerStatusArea: string;
  open(workspaceId: string): boolean;
  close(): void;
  chip(props: { label: string; icon?: ComponentType<{ className?: string }>; status: PeekStatus }): ReactNode;
}

interface PeekChipProps {
  peekId: string;
  label: string;
  icon?: ComponentType<{ className?: string }>;
  status?: PeekStatus;
}

function isPeekChip(value: unknown): value is ComponentType<PeekChipProps> {
  return typeof value === "function";
}

export interface ProcessOptions {
  command: string;
  args?: string[];
  cwd?: string;
  env?: Record<string, string | null>;
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

export interface OsInfo {
  platform: "linux" | "windows" | "macos";
  arch: "x86_64" | "aarch64";
  homeDir: string | null;
  configDir: string | null;
  dataDir: string | null;
  localDataDir: string | null;
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
  osInfo(): Promise<OsInfo>;
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
  if (!record(exports) || typeof exports.PEEK_AREA !== "string" || typeof exports.COMPOSER_STATUS_AREA !== "string" ||
    !isPeekChip(exports.PeekChip) || !record(host) || !record(host.peek)) return null;
  const Chip = exports.PeekChip;
  const peek = host.peek;
  if (typeof peek.open !== "function" || typeof peek.close !== "function") return null;
  const open = peek.open.bind(peek);
  const close = peek.close.bind(peek);
  const id = `${ctx.id}:live`;
  return {
    area: exports.PEEK_AREA,
    composerStatusArea: exports.COMPOSER_STATUS_AREA,
    open: (workspaceId) => open(id, { workspaceId }) === true,
    close: () => { close(id); },
    chip: (props) => createElement(Chip, { ...props, peekId: id }),
  };
}

// Keep the pending desktop/file APIs at one checked boundary.
export function desktopAdapter(ctx: PluginContext): DesktopAdapter | null {
  const value: unknown = ctx;
  if (!record(value) || !record(value.host)) return null;
  const process = value.host.process;
  const fs = value.host.fs;
  const os = value.host.os;
  if (
    !record(process) || typeof process.which !== "function" || typeof process.run !== "function" ||
    !record(fs) || typeof fs.writeDataFile !== "function" || typeof fs.removeDataFile !== "function" ||
    !record(os) || typeof os.info !== "function"
  ) return null;
  const which = process.which.bind(process);
  const run = process.run.bind(process);
  const write = fs.writeDataFile.bind(fs);
  const remove = fs.removeDataFile.bind(fs);
  const info = os.info.bind(os);
  return {
    async which(command) {
      const result: unknown = await which(command);
      if (result !== null && typeof result !== "string") throw new Error("Invalid executable lookup response.");
      return result;
    },
    async osInfo() {
      const result: unknown = await info();
      if (!record(result) || (result.platform !== "linux" && result.platform !== "windows" && result.platform !== "macos") ||
        (result.arch !== "x86_64" && result.arch !== "aarch64") ||
        (result.homeDir !== null && typeof result.homeDir !== "string") ||
        (result.configDir !== null && typeof result.configDir !== "string") ||
        (result.dataDir !== null && typeof result.dataDir !== "string") ||
        (result.localDataDir !== null && typeof result.localDataDir !== "string")) {
        throw new Error("Could not read the local installation directories.");
      }
      return {
        platform: result.platform, arch: result.arch, homeDir: result.homeDir, configDir: result.configDir,
        dataDir: result.dataDir, localDataDir: result.localDataDir,
      };
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
      const dispose = await ctx.registerMcpServer(options);
      let stopping: Promise<void> | undefined;
      return {
        call: (tool, args) => ctx.host.mcp.callTool(options.id, tool, args),
        status: () => ctx.host.mcp.getStatus(options.id),
        onStatus: (fn) => ctx.host.mcp.onStatus(options.id, fn),
        onActivity: (fn) => ctx.host.mcp.onActivity(options.id, fn),
        stop() {
          stopping ??= dispose();
          return stopping;
        },
      };
    },
  };
}
