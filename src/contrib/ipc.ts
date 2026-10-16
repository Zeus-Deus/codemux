/** Typed wrappers for the native `plugins_*` commands (src-tauri/src/plugins). */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface InstallMarker {
  source: "catalog" | "git";
  repo: string;
  sha: string;
  catalogId?: string;
  tier: "official" | "unverified";
  installedAt: string;
  subdir?: string;
  name?: string;
  description?: string;
}

export interface DiskPluginEntry {
  id: string;
  file: string;
  dir: string;
  marker: InstallMarker | null;
  error: string | null;
}

export interface PluginsRoot {
  path: string;
  disabledByEnv: boolean;
}

export const pluginsRoot = () => invoke<PluginsRoot>("plugins_root");
export const pluginsList = () => invoke<DiskPluginEntry[]>("plugins_list");
export const pluginsReadSource = (id: string) => invoke<string>("plugins_read_source", { id });
export const pluginsWatch = () => invoke<void>("plugins_watch");
export const pluginsRevealRoot = () => invoke<void>("plugins_reveal_root");

export const onPluginsChanged = (cb: (id: string | null) => void): Promise<UnlistenFn> =>
  listen<{ id: string | null }>("plugins_changed", (e) => cb(e.payload?.id ?? null));

export interface SafeModeStatus {
  paused: boolean;
  reason: string | null;
  disabledByEnv: boolean;
}

export const pluginsGetDecisions = () => invoke<Record<string, boolean>>("plugins_get_decisions");
export const pluginsSetEnabled = (id: string, on: boolean) =>
  invoke<void>("plugins_set_enabled", { id, on });
export const pluginsSafeModeStatus = () => invoke<SafeModeStatus>("plugins_safe_mode_status");
export const pluginsStartupCheckpoint = (phase: "loading" | "ok") =>
  invoke<void>("plugins_startup_checkpoint", { phase });
export const pluginsResume = () => invoke<void>("plugins_resume");
