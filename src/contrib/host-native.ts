/** Bounded desktop services owned by one plugin incarnation. */
import { Channel, invoke } from "@tauri-apps/api/core";
import type { Owner } from "./lifetime";
import { pluginPageEpoch } from "./mcp";
import { randomUUID } from "@/lib/uuid";

export interface PluginLocalContext {
  /** Optional open workspace to validate. Omit for machine-global operations.
   * Paths always refer to this machine; remote workspaces are refused. */
  workspaceId?: string;
}
export interface PluginProcessInput extends PluginLocalContext {
  command: string;
  args?: string[];
  cwd?: string;
  /** Inherit the app environment; override values or remove a key with null. */
  env?: Record<string, string | null>;
  /** Default 30 seconds; maximum 10 minutes. */
  timeoutMs?: number;
  /** Combined stdout/stderr bytes. Default 1 MiB; maximum 8 MiB. */
  maxOutputBytes?: number;
}
export interface PluginProcessResult {
  code: number | null;
  stdout: string;
  stderr: string;
  timedOut: boolean;
  /** Captured raw output exceeded maxOutputBytes (or a pipe read failed). */
  truncated: boolean;
  /** A descendant kept an inherited pipe open beyond the 100 ms drain window.
   * The host continues draining it without retaining more output. */
  pipesHeldOpen: boolean;
  cancelled: boolean;
}
export interface PluginDirectoryEntry {
  name: string;
  kind: "file" | "directory" | "symlink" | "other";
}
export interface PluginReadTextOptions extends PluginLocalContext {
  /** Refuse larger files. Default 1 MiB; maximum 8 MiB. */
  maxBytes?: number;
}
export interface PluginOsInfo {
  platform: "linux" | "windows" | "macos";
  arch: "x86_64" | "aarch64";
  homeDir: string | null;
  configDir: string | null;
  dataDir: string | null;
  localDataDir: string | null;
}
/** A helper program whose release Codemux pins (today only `cua-driver`). */
export interface PluginToolStatus {
  id: string;
  version: string;
  /** False when Codemux can't install it on this platform; see the reason. */
  supported: boolean;
  unsupportedReason: string | null;
  /** The Codemux-managed executable, when installed. */
  executable: string | null;
}
export type PluginToolProgress =
  | { stage: "downloading"; received: number; total: number }
  | { stage: "verifying" }
  | { stage: "validating" };
type Auth = { pluginId: string; token: string };
const owners = new WeakMap<Owner, Promise<Auth>>();
async function ownerAuth(owner: Owner | null): Promise<Auth> {
  if (!owner || owner.ended)
    throw new Error("Plugin host owner is no longer active; use ctx.host");
  let pending = owners.get(owner);
  if (!pending) {
    pending = (async () => {
      const pageEpoch = await pluginPageEpoch();
      if (owner.ended) throw new Error("Plugin has been disposed");
      const auth = { pluginId: owner.pluginId, token: randomUUID() };
      await invoke<void>("plugins_host_acquire", { ...auth, pageEpoch });
      let stopped: Promise<void> | undefined;
      const stop = () =>
        (stopped ??= invoke<void>("plugins_host_release", auth));
      // track runs immediately if the asynchronous acquire finished after unload.
      owner.track(() => {
        void stop().catch((error) =>
          console.error(
            `[plugins] ${owner.pluginId}: host cleanup failed`,
            error,
          ),
        );
      });
      if (owner.ended) {
        await stop();
        throw new Error("Plugin has been disposed");
      }
      return auth;
    })();
    owners.set(owner, pending);
  }
  let auth: Auth;
  try {
    auth = await pending;
  } catch (error) {
    // Retry transient native failures without clearing a newer acquisition.
    if (owners.get(owner) === pending) owners.delete(owner);
    throw error;
  }
  if (owner.ended) throw new Error("Plugin has been disposed");
  return auth;
}

type FsOperation =
  | { kind: "exists" | "listDir"; path: string }
  | { kind: "readText"; path: string; maxBytes?: number }
  | { kind: "dataDir" }
  | { kind: "writeDataFile"; name: string; contents: string }
  | { kind: "removeDataFile"; name: string };
export function createNativeHost(ownerNow: () => Owner | null) {
  const fs = async <T>(
    operation: FsOperation,
    options?: PluginLocalContext,
  ): Promise<T> =>
    invoke<T>("plugins_fs", {
      ...(await ownerAuth(ownerNow())),
      workspaceId: options?.workspaceId,
      operation,
    });
  return {
    os: {
      info: async (): Promise<PluginOsInfo> =>
        invoke("plugins_os_info", await ownerAuth(ownerNow())),
    },
    process: {
      run: async (input: PluginProcessInput): Promise<PluginProcessResult> =>
        invoke("plugins_process_run", {
          ...(await ownerAuth(ownerNow())),
          input,
        }),
      which: async (command: string): Promise<string | null> =>
        invoke("plugins_process_which", {
          ...(await ownerAuth(ownerNow())),
          command,
        }),
    },
    /** App-pinned helper programs. The release, checksum and files are fixed
     *  by Codemux; installs go to its own data folder, never PATH, and are
     *  validated before they replace an earlier managed copy. */
    tools: {
      status: async (tool: string): Promise<PluginToolStatus> =>
        invoke("plugins_tool_status", { ...(await ownerAuth(ownerNow())), tool }),
      /** Resolves with the installed executable path. */
      install: async (
        tool: string,
        onProgress?: (progress: PluginToolProgress) => void,
      ): Promise<string> => {
        const channel = new Channel<PluginToolProgress>();
        if (onProgress) channel.onmessage = onProgress;
        return invoke("plugins_tool_install", {
          ...(await ownerAuth(ownerNow())),
          tool,
          onProgress: channel,
        });
      },
    },
    fs: {
      exists: (path: string, options?: PluginLocalContext) =>
        fs<boolean>({ kind: "exists", path }, options),
      listDir: (path: string, options?: PluginLocalContext) =>
        fs<PluginDirectoryEntry[]>({ kind: "listDir", path }, options),
      readText: (path: string, options?: PluginReadTextOptions) =>
        fs<string>(
          { kind: "readText", path, maxBytes: options?.maxBytes },
          options,
        ),
      dataDir: (options?: PluginLocalContext) =>
        fs<string>({ kind: "dataDir" }, options),
      writeDataFile: (
        name: string,
        contents: string,
        options?: PluginLocalContext,
      ) => fs<string>({ kind: "writeDataFile", name, contents }, options),
      removeDataFile: (name: string, options?: PluginLocalContext) =>
        fs<void>({ kind: "removeDataFile", name }, options),
    },
  };
}
