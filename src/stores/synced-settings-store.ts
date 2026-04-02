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
  file_tree: { show_hidden_files: false },
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

// Each optimistic write increments this. Async responses only apply if
// the generation matches, preventing stale backend responses from
// reverting a newer optimistic update.
let _settingsGen = 0;
// How many writes are currently awaiting a backend response. While > 0,
// applySettingsFromEvent skips incoming Tauri events because the async
// response path already handles them with a gen-check.
let _inflightWrites = 0;

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
    const gen = ++_settingsGen;
    _inflightWrites++;
    set({ settings, isSyncing: true });
    try {
      const saved = await updateSyncedSettings(settings);
      if (_settingsGen === gen) set({ settings: saved, isSyncing: false });
      else set({ isSyncing: false });
    } catch {
      set({ isSyncing: false });
    } finally {
      _inflightWrites--;
    }
  },

  updateSetting: async (section, key, value) => {
    const gen = ++_settingsGen;
    _inflightWrites++;
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
      if (_settingsGen === gen) {
        // Re-apply our intended value on top of the server response.
        // The server PATCH deep-merges nested objects, so sending
        // e.g. { shortcuts: {} } is a no-op from the server's
        // perspective. Force-set the exact field we wrote.
        const patched = JSON.parse(JSON.stringify(saved)) as Record<string, Record<string, unknown>>;
        if (patched[section]) patched[section][key] = value;
        const corrected = patched as unknown as UserSettings;
        set({ settings: corrected, isSyncing: false });

        // If the server response didn't match our intent (deep-merge
        // semantics), do a background full PUT to correct the server.
        const serverVal = JSON.stringify((saved as unknown as Record<string, Record<string, unknown>>)[section]?.[key]);
        const intendedVal = JSON.stringify(value);
        if (serverVal !== intendedVal) {
          updateSyncedSettings(corrected).catch(() => {});
        }
      } else {
        set({ isSyncing: false });
      }
    } catch {
      set({ isSyncing: false });
    } finally {
      _inflightWrites--;
    }
  },

  resetSettings: async () => {
    const gen = ++_settingsGen;
    _inflightWrites++;
    set({ settings: DEFAULT_SETTINGS, isSyncing: true });
    try {
      const saved = await resetSyncedSettings();
      if (_settingsGen === gen) set({ settings: saved, isSyncing: false });
      else set({ isSyncing: false });
    } catch {
      set({ isSyncing: false });
    } finally {
      _inflightWrites--;
    }
  },

  applySettingsFromEvent: (settings) => {
    // Skip events while local writes are in flight — the async response
    // path handles those with gen-checks. Only apply events from
    // external sources (other devices, server push).
    if (_inflightWrites > 0) return;
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

export const selectShowHiddenFiles = (s: SyncedSettingsState): boolean =>
  s.settings.file_tree.show_hidden_files;
