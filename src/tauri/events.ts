import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type { UnlistenFn };
import type {
  AppStateSnapshot,
  AuthStatePayload,
  OrchestratorTriggerResult,
  PresetStoreSnapshot,
  TerminalStatusPayload,
  ThemeColors,
  UserSettings,
} from "./types";

export type EventCallback<T> = (payload: T) => void;

export const onAppStateChanged = (cb: EventCallback<AppStateSnapshot>): Promise<UnlistenFn> =>
  listen<AppStateSnapshot>("app-state-changed", (e) => cb(e.payload));

export const onPresetsChanged = (cb: EventCallback<PresetStoreSnapshot>): Promise<UnlistenFn> =>
  listen<PresetStoreSnapshot>("presets-changed", (e) => cb(e.payload));

export const onThemeChanged = (cb: EventCallback<ThemeColors>): Promise<UnlistenFn> =>
  listen<ThemeColors>("theme-changed", (e) => cb(e.payload));

export const onTerminalStatus = (cb: EventCallback<TerminalStatusPayload>): Promise<UnlistenFn> =>
  listen<TerminalStatusPayload>("terminal-status", (e) => cb(e.payload));

export interface WorkspaceSetupProgress {
  workspace_id: string;
  command: string;
  index: number;
  total: number;
}

export const onWorkspaceSetupProgress = (
  cb: EventCallback<WorkspaceSetupProgress>,
): Promise<UnlistenFn> =>
  listen<WorkspaceSetupProgress>("workspace-setup-progress", (e) => cb(e.payload));

export interface WorkspaceSetupComplete {
  workspace_id: string;
}

export const onWorkspaceSetupComplete = (
  cb: EventCallback<WorkspaceSetupComplete>,
): Promise<UnlistenFn> =>
  listen<WorkspaceSetupComplete>("workspace-setup-complete", (e) => cb(e.payload));

export interface WorkspaceSetupFailed {
  workspace_id: string;
  command: string;
  stdout: string;
  stderr: string;
  exit_code: number | null;
}

export const onWorkspaceSetupFailed = (
  cb: EventCallback<WorkspaceSetupFailed>,
): Promise<UnlistenFn> =>
  listen<WorkspaceSetupFailed>("workspace-setup-failed", (e) => cb(e.payload));

export interface WorktreeIncludesApplied {
  workspace_id: string;
  source: "file" | "setting" | "defaults";
  copied: string[];
}

export const onWorktreeIncludesApplied = (
  cb: EventCallback<WorktreeIncludesApplied>,
): Promise<UnlistenFn> =>
  listen<WorktreeIncludesApplied>("worktree-includes-applied", (e) => cb(e.payload));

export const onOpenflowCycle = (
  cb: EventCallback<OrchestratorTriggerResult>,
): Promise<UnlistenFn> =>
  listen<OrchestratorTriggerResult>("openflow-cycle", (e) => cb(e.payload));

export const onSettingsSynced = (
  cb: EventCallback<UserSettings>,
): Promise<UnlistenFn> =>
  listen<UserSettings>("settings-synced", (e) => cb(e.payload));

export const onAuthStateChanged = (
  cb: EventCallback<AuthStatePayload>,
): Promise<UnlistenFn> =>
  listen<AuthStatePayload>("auth-state-changed", (e) => cb(e.payload));
