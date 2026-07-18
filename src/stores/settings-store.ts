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
  // Color palette variant — "cool" (neutral graphite, default) or "warm".
  // Applied via a `.theme-warm` class on the root that overrides the
  // surface tokens (see globals.css). Cool = the default token map.
  "appearance.palette": "cool",
  // Spacing density — "comfortable" (default) or "compact". Scales card
  // padding, grid gaps, and group rhythm via the root `data-density` attr.
  "appearance.density": "comfortable",
  // Sidebar grouping mode — "project" (default) keeps live agents in their
  // project group; "top" gathers every live agent into a LIVE section above
  // the project tree. See sidebar-workspace-list.tsx.
  "sidebar.live_agents": "project",
  // Working-indicator glyph shown while an agent is working — the animation
  // variant and its token color. Rendered by the WorkingIndicator component
  // in the sidebar row and rail flyout.
  "sidebar.working_indicator": "braille",
  "sidebar.working_indicator_color": "status-working",
};

/** Color palette variant. */
export type AppearancePalette = "cool" | "warm";

/** Spacing density mode. */
export type AppearanceDensity = "comfortable" | "compact";

/** Sidebar grouping mode for live agents. */
export type SidebarLiveAgents = "project" | "top";

/** Working-indicator animation variant. */
export type WorkingIndicatorVariant =
  | "braille"
  | "ring"
  | "blink"
  | "sweep"
  | "typing";

/** Working-indicator color token. No red — that's reserved for the
 *  needs-you dot. */
export type WorkingIndicatorColor =
  | "status-working"
  | "foreground"
  | "accent-ember"
  | "status-open"
  | "status-remote"
  | "accent-violet";

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

export const selectPalette = (s: SettingsStore): AppearancePalette =>
  (s.settings["appearance.palette"] ??
    SETTINGS_DEFAULTS["appearance.palette"]!) as AppearancePalette;

export const selectDensity = (s: SettingsStore): AppearanceDensity =>
  (s.settings["appearance.density"] ??
    SETTINGS_DEFAULTS["appearance.density"]!) as AppearanceDensity;

export const selectSidebarLiveAgents = (s: SettingsStore): SidebarLiveAgents =>
  (s.settings["sidebar.live_agents"] ??
    SETTINGS_DEFAULTS["sidebar.live_agents"]!) as SidebarLiveAgents;

export const selectWorkingIndicator = (
  s: SettingsStore,
): WorkingIndicatorVariant =>
  (s.settings["sidebar.working_indicator"] ??
    SETTINGS_DEFAULTS["sidebar.working_indicator"]!) as WorkingIndicatorVariant;

export const selectWorkingIndicatorColor = (
  s: SettingsStore,
): WorkingIndicatorColor =>
  (s.settings["sidebar.working_indicator_color"] ??
    SETTINGS_DEFAULTS[
      "sidebar.working_indicator_color"
    ]!) as WorkingIndicatorColor;
