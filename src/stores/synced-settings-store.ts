import { create } from "zustand";
import type { UserSettings } from "@/tauri/types";
import {
  getSyncedSettings,
  updateSyncedSettings,
  updateSetting as updateSettingCmd,
  resetSyncedSettings,
} from "@/tauri/commands";

const DEFAULT_SETTINGS: UserSettings = {
  appearance: { theme: "system", shell_font: null, terminal_font_size: 13 },
  editor: { default_ide: null },
  terminal: { scrollback_limit: 10_000, cursor_style: "bar" },
  git: { default_base_branch: "main" },
  keyboard: { shortcuts: {} },
  notifications: { sound_enabled: true, desktop_enabled: true },
};

export interface SyncedSettingsState {
  settings: UserSettings;
  isLoading: boolean;
  isSyncing: boolean;
}

interface SyncedSettingsActions {
  loadSettings: () => Promise<void>;
  updateSettings: (settings: UserSettings) => Promise<void>;
  updateSetting: (section: string, key: string, value: unknown) => Promise<void>;
  resetSettings: () => Promise<void>;
  applySettingsFromEvent: (settings: UserSettings) => void;
}

type SyncedSettingsStore = SyncedSettingsState & SyncedSettingsActions;

export { DEFAULT_SETTINGS };

export const useSyncedSettingsStore = create<SyncedSettingsStore>()((set) => ({
  settings: DEFAULT_SETTINGS,
  isLoading: true,
  isSyncing: false,

  loadSettings: async () => {
    set({ isLoading: true });
    try {
      const settings = await getSyncedSettings();
      set({ settings, isLoading: false });
    } catch {
      set({ isLoading: false });
    }
  },

  updateSettings: async (settings) => {
    // Optimistic update
    set({ settings, isSyncing: true });
    try {
      const saved = await updateSyncedSettings(settings);
      set({ settings: saved, isSyncing: false });
    } catch {
      set({ isSyncing: false });
    }
  },

  updateSetting: async (section, key, value) => {
    // Optimistic update — apply locally first
    set((s) => {
      const json = JSON.parse(JSON.stringify(s.settings)) as Record<string, Record<string, unknown>>;
      if (json[section]) {
        json[section][key] = value;
      }
      return { settings: json as unknown as UserSettings, isSyncing: true };
    });
    try {
      const saved = await updateSettingCmd(section, key, value);
      set({ settings: saved, isSyncing: false });
    } catch {
      set({ isSyncing: false });
    }
  },

  resetSettings: async () => {
    set({ settings: DEFAULT_SETTINGS, isSyncing: true });
    try {
      const saved = await resetSyncedSettings();
      set({ settings: saved, isSyncing: false });
    } catch {
      set({ isSyncing: false });
    }
  },

  applySettingsFromEvent: (settings) => {
    set({ settings });
  },
}));

// ── React hook selectors (trigger re-renders on specific value change) ──

export const selectTerminalFontSize = (s: SyncedSettingsState): number =>
  s.settings.appearance.terminal_font_size;

export const selectTerminalCursorStyle = (s: SyncedSettingsState): string =>
  s.settings.terminal.cursor_style;

export const selectDefaultEditor = (s: SyncedSettingsState): string =>
  s.settings.editor.default_ide ?? "";

export const selectDefaultBaseBranch = (s: SyncedSettingsState): string =>
  s.settings.git.default_base_branch;

export const selectNotificationSoundEnabled = (s: SyncedSettingsState): boolean =>
  s.settings.notifications.sound_enabled;

export const selectDesktopNotificationsEnabled = (s: SyncedSettingsState): boolean =>
  s.settings.notifications.desktop_enabled;

export const selectKeyboardShortcuts = (s: SyncedSettingsState): Record<string, string> =>
  s.settings.keyboard.shortcuts;
