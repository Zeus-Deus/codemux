import { create } from "zustand";
import { dbGetAllSettings, dbSetSetting } from "@/tauri/commands";
import { useSyncedSettingsStore } from "./synced-settings-store";

/** Machine-local settings only. Per-user settings live in synced-settings-store. */
export const SETTINGS_DEFAULTS: Record<string, string> = {
  "terminal.color_theme": "app",
  "terminal.font_family": "'JetBrains Mono Variable', monospace",
  ai_commit_message_enabled: "true",
  ai_commit_message_model: "",
  ai_resolver_enabled: "false",
  ai_resolver_cli: "",
  ai_resolver_model: "",
  ai_resolver_strategy: "smart_merge",
  auto_mcp_config: "true",
};

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
