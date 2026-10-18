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

// ── catalog and install ───────────────────────────────────────────────

export interface CatalogPlugin {
  id: string;
  name: string;
  description: string;
  tier: "official" | "unverified";
  repo: string;
  sha: string;
  subdir?: string;
  minCodemux?: string;
  screenshots: string[];
}

export interface CatalogView {
  catalog: {
    schemaVersion: number;
    revision: number;
    plugins: CatalogPlugin[];
    blocked: { id?: string; sha?: string; reason: string }[];
  };
  fetchedAt: number | null;
  error: string | null;
}

export type InstallSource =
  | { kind: "git"; url: string; gitRef?: string | null }
  | { kind: "catalog"; id: string }
  | { kind: "update"; id: string };

export interface InstallPreview {
  id: string;
  name: string;
  description: string | null;
  version: string | null;
  source: "git" | "catalog";
  repo: string;
  sha: string;
  subdir: string | null;
  tier: "official" | "unverified";
  /** The installed commit this replaces (update / reinstall). */
  replaces: string | null;
}

export const pluginsCatalog = (refresh = false) => invoke<CatalogView>("plugins_catalog", { refresh });
export const pluginsPreviewInstall = (source: InstallSource) =>
  invoke<InstallPreview>("plugins_preview_install", { source });
export const pluginsInstall = (source: InstallSource, expectedSha: string, enable: boolean) =>
  invoke<InstallPreview>("plugins_install", { source, expectedSha, enable });
export const pluginsRemove = (id: string) => invoke<void>("plugins_remove", { id });

// ── host.chat: status and compaction ──────────────────────────────────

export interface ThreadStatus {
  provider: string;
  /** A turn, pending approval, queue dispatch or compaction is in flight. */
  busy: boolean;
  /** Follow-ups waiting behind it; null when the provider can't tell. */
  queued: number | null;
  /** A requested compaction is running. */
  compacting: boolean;
  status: "starting" | "ready" | "running" | "waiting_approval" | "error" | "closed";
}

export type CompactionRefusal =
  | "busy"
  | "queued"
  | "not_ready"
  | "closed"
  | "no_session"
  | "not_claude"
  | "unsupported"
  | "compaction_off"
  | "idle_compaction_off"
  | "failed";

export type CompactionRequestResult = { started: true } | { started: false; reason: CompactionRefusal };

export const pluginsChatStatus = (threadId: string) =>
  invoke<ThreadStatus | null>("plugins_chat_status", { threadId });
export const pluginsChatRequestCompaction = (threadId: string) =>
  invoke<CompactionRequestResult>("plugins_chat_request_compaction", { threadId });
