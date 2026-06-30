import { create } from "zustand";
import { dbGetAllSettings, dbSetSetting } from "@/tauri/commands";
import { useSyncedSettingsStore } from "./synced-settings-store";

/** Machine-local settings only. Per-user settings live in synced-settings-store. */
export const SETTINGS_DEFAULTS: Record<string, string> = {
  "terminal.color_theme": "app",
  "terminal.font_family": "'JetBrains Mono Variable', monospace",
  ai_commit_message_enabled: "true",
  ai_commit_message_cli: "",
  ai_commit_message_model: "",
  ai_resolver_enabled: "false",
  ai_resolver_cli: "",
  ai_resolver_model: "",
  ai_resolver_strategy: "smart_merge",
  auto_mcp_config: "true",
  // How much detail each project-sidebar workspace row shows:
  //   "clean"    — icon + name + live status only (default — calmest)
  //   "branch"   — also the git branch name
  //   "detailed" — also ahead/behind + diff stats
  "sidebar.workspace_detail": "clean",
  // Color palette variant — "cool" (neutral graphite, default) or "warm".
  // Applied via a `.theme-warm` class on the root that overrides the
  // surface tokens (see globals.css). Cool = the default token map.
  "appearance.palette": "cool",
  // Spacing density — "comfortable" (default) or "compact". Scales card
  // padding, grid gaps, and group rhythm via the root `data-density` attr.
  "appearance.density": "comfortable",
};

/** Sidebar workspace-row detail level. */
export type SidebarWorkspaceDetail = "clean" | "branch" | "detailed";

/** Color palette variant. */
export type AppearancePalette = "cool" | "warm";

/** Spacing density mode. */
export type AppearanceDensity = "comfortable" | "compact";

interface SettingsState {
  loaded: boolean;
  settings: Record<string, string>;
}

interface SettingsActions {
  load: () => Promise<void>;
  get: (key: string) => string;
  set: (key: string, value: string) => void;
}

type SettingsStore = SettingsState & SettingsActions;

export const useSettingsStore = create<SettingsStore>()((setState, getState) => ({
  loaded: false,
  settings: {},

  load: async () => {
    try {
      const all = await dbGetAllSettings();
      setState({ settings: all, loaded: true });
    } catch {
      setState({ loaded: true });
    }
  },

  get: (key: string) => {
    return getState().settings[key] ?? SETTINGS_DEFAULTS[key] ?? "";
  },

  set: (key: string, value: string) => {
    setState((s) => ({
      settings: { ...s.settings, [key]: value },
    }));
    dbSetSetting(key, value).catch(console.error);
  },
}));

// ── Machine-local imperative getters ──

function raw(key: string): string {
  return useSettingsStore.getState().settings[key] ?? SETTINGS_DEFAULTS[key] ?? "";
}

export function getTerminalColorTheme(): string {
  return raw("terminal.color_theme");
}

export function getTerminalFontFamily(): string {
  return raw("terminal.font_family");
}

// ── Per-user imperative getters (redirect to synced store for backward compat) ──

export function getTerminalFontSize(): number {
  return useSyncedSettingsStore.getState().settings.appearance.terminal_font_size;
}

export function getTerminalCursorStyle(): string {
  return useSyncedSettingsStore.getState().settings.terminal.cursor_style;
}

export function getDefaultEditor(): string {
  return useSyncedSettingsStore.getState().settings.editor.default_ide ?? "";
}

export function getDefaultBaseBranch(): string {
  return useSyncedSettingsStore.getState().settings.git.default_base_branch;
}

// ── Machine-local React hook selectors ──

export const selectTerminalColorTheme = (s: SettingsStore): string =>
  s.settings["terminal.color_theme"] ?? SETTINGS_DEFAULTS["terminal.color_theme"]!;

export const selectTerminalFontFamily = (s: SettingsStore): string =>
  s.settings["terminal.font_family"] ?? SETTINGS_DEFAULTS["terminal.font_family"]!;

export const selectSidebarWorkspaceDetail = (
  s: SettingsStore,
): SidebarWorkspaceDetail =>
  (s.settings["sidebar.workspace_detail"] ??
    SETTINGS_DEFAULTS["sidebar.workspace_detail"]!) as SidebarWorkspaceDetail;

export const selectPalette = (s: SettingsStore): AppearancePalette =>
  (s.settings["appearance.palette"] ??
    SETTINGS_DEFAULTS["appearance.palette"]!) as AppearancePalette;

export const selectDensity = (s: SettingsStore): AppearanceDensity =>
  (s.settings["appearance.density"] ??
    SETTINGS_DEFAULTS["appearance.density"]!) as AppearanceDensity;
