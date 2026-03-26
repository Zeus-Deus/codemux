import { create } from "zustand";
import { dbGetAllSettings, dbSetSetting } from "@/tauri/commands";

const DEFAULTS: Record<string, string> = {
  "terminal.font_size": "13",
  "terminal.cursor_style": "bar",
  "terminal.color_theme": "app",
  "terminal.font_family": "'JetBrains Mono Variable', monospace",
  "editor.default": "",
  "git.default_base_branch": "main",
  notification_sound_enabled: "true",
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
    return getState().settings[key] ?? DEFAULTS[key] ?? "";
  },

  set: (key: string, value: string) => {
    setState((s) => ({
      settings: { ...s.settings, [key]: value },
    }));
    dbSetSetting(key, value).catch(console.error);
  },
}));

// ── Typed selectors (computed from raw settings) ──

function raw(key: string): string {
  return useSettingsStore.getState().settings[key] ?? DEFAULTS[key] ?? "";
}

export function getTerminalFontSize(): number {
  return Number(raw("terminal.font_size")) || 13;
}

export function getTerminalCursorStyle(): string {
  return raw("terminal.cursor_style");
}

export function getTerminalColorTheme(): string {
  return raw("terminal.color_theme");
}

export function getTerminalFontFamily(): string {
  return raw("terminal.font_family");
}

export function getDefaultEditor(): string {
  return raw("editor.default");
}

export function getDefaultBaseBranch(): string {
  return raw("git.default_base_branch");
}

// ── React hook selectors (trigger re-renders) ──

export const selectTerminalFontSize = (s: SettingsStore): number =>
  Number(s.settings["terminal.font_size"] ?? DEFAULTS["terminal.font_size"]) || 13;

export const selectTerminalCursorStyle = (s: SettingsStore): string =>
  s.settings["terminal.cursor_style"] ?? DEFAULTS["terminal.cursor_style"]!;

export const selectTerminalColorTheme = (s: SettingsStore): string =>
  s.settings["terminal.color_theme"] ?? DEFAULTS["terminal.color_theme"]!;

export const selectTerminalFontFamily = (s: SettingsStore): string =>
  s.settings["terminal.font_family"] ?? DEFAULTS["terminal.font_family"]!;

export const selectDefaultEditor = (s: SettingsStore): string =>
  s.settings["editor.default"] ?? DEFAULTS["editor.default"]!;

export const selectDefaultBaseBranch = (s: SettingsStore): string =>
  s.settings["git.default_base_branch"] ?? DEFAULTS["git.default_base_branch"]!;
