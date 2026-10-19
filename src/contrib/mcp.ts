import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { McpServerRuntime, McpTool } from "@/tauri/commands";
import { useMcpStore } from "@/stores/mcp-store";
import { type Owner } from "./lifetime";

export type PluginMcpScope = "all" | "plugin" | { workspaceIds: string[] };
export type PluginMcpServer = {
  id: string;
  name: string;
  /** 1–32 ASCII letters, digits, underscores or dashes; defaults to plugin id.
   * May repeat across registrations with disjoint workspace scopes. */
  toolPrefix?: string;
  /** Applied to raw tool names for every provider and the live catalog.
   * 128 entries, 2000 UTF-8 bytes per text, 16 KiB total. */
  toolDescriptions?: Record<string, { append?: string; replace?: string }>;
  scope: PluginMcpScope;
  /** Immediate skips EOF grace: Linux kills the owned process group;
   * other platforms terminate the owned child. Default: graceful. */
  shutdown?: "graceful" | "immediate";
} & (
  | { transport: "stdio"; command: string; args?: string[]; env?: Record<string, string>; cwd?: string }
  | { transport: "http"; url: string; headers?: Record<string, string> }
);
export type PluginMcpTool = McpTool;
export type PluginMcpStatus = McpServerRuntime;
export interface PluginMcpContent {
  type: string;
  text?: string;
  data?: string;
  mimeType?: string;
  [key: string]: unknown;
}
export interface PluginMcpCallResult {
  content: PluginMcpContent[];
  isError?: boolean;
  structuredContent?: unknown;
  [key: string]: unknown;
}
export interface PluginMcpActivity {
  phase: "start" | "end";
  callId: string;
  toolName: string;
  prefixedName: string;
  workspaceId: string | null;
  threadId?: string;
  isError?: boolean;
  durationMs?: number;
}
const registrations = new WeakMap<Owner, Map<string, string>>();
const serverId = (pluginId: string, id: string) => `plugin:${pluginId}:${id}`;
function tokenFor(owner: Owner | null, id: string): { pluginId: string; localId: string; token: string } {
  const token = owner && !owner.ended ? registrations.get(owner)?.get(id) : null;
  if (!owner || !token) throw new Error("Plugin MCP registration is no longer active; use ctx.host.mcp");
  return { pluginId: owner.pluginId, localId: id, token };
}
export async function registerMcpServer(owner: Owner, server: PluginMcpServer): Promise<() => Promise<void>> {
  if (owner.ended) throw new Error("Plugin has been disposed");
  // The persisted switch is stable across incarnations, including registrations
  // that arrive before Settings or the first chat has mounted.
  await useMcpStore.getState().syncToBackend();
  if (owner.ended) throw new Error("Plugin has been disposed");
  const token = crypto.randomUUID();
  await invoke("plugins_mcp_register", { pluginId: owner.pluginId, token, server });
  const map = registrations.get(owner) ?? new Map<string, string>();
  registrations.set(owner, map);
  map.set(server.id, token);
  let stopped: Promise<void> | undefined;
  const stop = () => stopped ??= (async () => {
    await invoke("plugins_mcp_unregister", { pluginId: owner.pluginId, localId: server.id, token });
    if (map.get(server.id) === token) map.delete(server.id);
  })();
  const release = owner.track(() => { void stop().catch((error) => console.error(`[plugins] ${owner.pluginId}: MCP cleanup failed`, error)); });
  const dispose = () => { release(); return stop(); };
  if (owner.ended) await dispose();
  return dispose;
}

export function createMcpHost(ownerNow: () => Owner | null, subscription: <A extends unknown[]>(fn: (...args: A) => void, attach: (run: (...args: A) => void) => () => void) => () => void) {
  return {
    listTools: (id: string) => invoke<PluginMcpTool[]>("plugins_mcp_list_tools", tokenFor(ownerNow(), id)),
    callTool: (id: string, toolName: string, args: Record<string, unknown> = {}) => invoke<PluginMcpCallResult>("plugins_mcp_call_tool", { ...tokenFor(ownerNow(), id), toolName, arguments: args }),
    getStatus: (id: string) => invoke<PluginMcpStatus | null>("plugins_mcp_status", tokenFor(ownerNow(), id)),
    onStatus: (id: string, fn: (status: PluginMcpStatus | null) => void) => {
      const owner = ownerNow();
      const auth = tokenFor(owner, id);
      return subscription(fn, (run) => {
        let ended = false;
        let unlisten: (() => void) | undefined;
        void listen<McpServerRuntime>("mcp-status-changed", (event) => {
          if (event.payload.id === serverId(auth.pluginId, id)) {
            void invoke<PluginMcpStatus | null>("plugins_mcp_status", auth).then((value) => { if (!ended) run(value); }).catch(() => { if (!ended) run(null); });
          }
        }).then((stop) => { if (ended) stop(); else unlisten = stop; });
        return () => { ended = true; unlisten?.(); };
      });
    },
    onActivity: (id: string, fn: (event: PluginMcpActivity) => void) => {
      const owner = ownerNow();
      const auth = tokenFor(owner, id);
      return subscription(fn, (run) => {
        let ended = false;
        let unlisten: (() => void) | undefined;
        void listen<PluginMcpActivity & { serverId: string }>("plugin-mcp-activity", (event) => {
          if (!ended && event.payload.serverId === serverId(auth.pluginId, id)) run(event.payload);
        }).then((stop) => { if (ended) stop(); else unlisten = stop; });
        return () => { ended = true; unlisten?.(); };
      });
    },
  };
}
