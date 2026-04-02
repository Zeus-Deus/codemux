import { describe, it, expect, vi, beforeEach } from "vitest";

// ── Mock Tauri commands ──

const mockGetSyncedSettings = vi.fn();
const mockUpdateSyncedSettings = vi.fn();
const mockUpdateSetting = vi.fn();
const mockResetSyncedSettings = vi.fn();

vi.mock("@/tauri/commands", () => ({
  getSyncedSettings: (...args: unknown[]) => mockGetSyncedSettings(...args),
  updateSyncedSettings: (...args: unknown[]) => mockUpdateSyncedSettings(...args),
  updateSetting: (...args: unknown[]) => mockUpdateSetting(...args),
  resetSyncedSettings: (...args: unknown[]) => mockResetSyncedSettings(...args),
}));

import {
  useSyncedSettingsStore,
  DEFAULT_SETTINGS,
  selectTerminalFontSize,
  selectTerminalCursorStyle,
  selectDefaultEditor,
  selectDefaultBaseBranch,
  selectNotificationSoundEnabled,
  selectDesktopNotificationsEnabled,
} from "./synced-settings-store";
import type { UserSettings } from "@/tauri/types";

const DARK_SETTINGS: UserSettings = {
  appearance: { theme: "dark", shell_font: "Fira Code", terminal_font_size: 16 },
  editor: { default_ide: "cursor" },
  terminal: { scrollback_limit: 5000, cursor_style: "block" },
  git: { default_base_branch: "develop" },
  keyboard: { shortcuts: { "ctrl+s": "save" } },
  notifications: { sound_enabled: false, desktop_enabled: true },
  file_tree: { show_hidden_files: true },
};

describe("synced-settings-store", () => {
  beforeEach(() => {
    useSyncedSettingsStore.setState({
      settings: DEFAULT_SETTINGS,
      isLoading: true,
      isSyncing: false,
    });
    vi.clearAllMocks();
  });

  describe("initial state", () => {
    it("starts with default settings", () => {
      const { settings } = useSyncedSettingsStore.getState();
      expect(settings.appearance.theme).toBe("system");
      expect(settings.appearance.terminal_font_size).toBe(13);
      expect(settings.terminal.scrollback_limit).toBe(10_000);
      expect(settings.terminal.cursor_style).toBe("bar");
      expect(settings.git.default_base_branch).toBe("main");
      expect(settings.notifications.sound_enabled).toBe(true);
      expect(settings.notifications.desktop_enabled).toBe(true);
    });

    it("isLoading starts true (guards AppShell until loadSettings completes)", () => {
      const state = useSyncedSettingsStore.getState();
      expect(state.isLoading).toBe(true);
      expect(state.isSyncing).toBe(false);
    });
  });

  describe("loadSettings", () => {
    it("fetches settings and updates store", async () => {
      mockGetSyncedSettings.mockResolvedValue(DARK_SETTINGS);

      await useSyncedSettingsStore.getState().loadSettings();

      const state = useSyncedSettingsStore.getState();
      expect(state.isLoading).toBe(false);
      expect(state.settings.appearance.theme).toBe("dark");
      expect(state.settings.editor.default_ide).toBe("cursor");
      expect(state.settings.terminal.scrollback_limit).toBe(5000);
    });

    it("sets isLoading during fetch", async () => {
      let resolvePromise: (v: UserSettings) => void;
      mockGetSyncedSettings.mockReturnValue(
        new Promise<UserSettings>((r) => { resolvePromise = r; }),
      );

      const promise = useSyncedSettingsStore.getState().loadSettings();
      expect(useSyncedSettingsStore.getState().isLoading).toBe(true);

      resolvePromise!(DEFAULT_SETTINGS);
      await promise;
      expect(useSyncedSettingsStore.getState().isLoading).toBe(false);
    });

    it("keeps defaults on error", async () => {
      mockGetSyncedSettings.mockRejectedValue(new Error("Network error"));

      await useSyncedSettingsStore.getState().loadSettings();

      const state = useSyncedSettingsStore.getState();
      expect(state.isLoading).toBe(false);
      expect(state.settings).toEqual(DEFAULT_SETTINGS);
    });
  });

  describe("updateSettings", () => {
    it("optimistically updates and syncs", async () => {
      mockUpdateSyncedSettings.mockResolvedValue(DARK_SETTINGS);

      await useSyncedSettingsStore.getState().updateSettings(DARK_SETTINGS);

      const state = useSyncedSettingsStore.getState();
      expect(state.settings).toEqual(DARK_SETTINGS);
      expect(state.isSyncing).toBe(false);
      expect(mockUpdateSyncedSettings).toHaveBeenCalledWith(DARK_SETTINGS);
    });

    it("isSyncing toggles correctly", async () => {
      let resolvePromise: (v: UserSettings) => void;
      mockUpdateSyncedSettings.mockReturnValue(
        new Promise<UserSettings>((r) => { resolvePromise = r; }),
      );

      const promise = useSyncedSettingsStore.getState().updateSettings(DARK_SETTINGS);
      expect(useSyncedSettingsStore.getState().isSyncing).toBe(true);

      resolvePromise!(DARK_SETTINGS);
      await promise;
      expect(useSyncedSettingsStore.getState().isSyncing).toBe(false);
    });
  });

  describe("updateSetting", () => {
    it("updates a single field", async () => {
      const returned = {
        ...DEFAULT_SETTINGS,
        appearance: { ...DEFAULT_SETTINGS.appearance, theme: "dark" },
      };
      mockUpdateSetting.mockResolvedValue(returned);

      await useSyncedSettingsStore.getState().updateSetting("appearance", "theme", "dark");

      const state = useSyncedSettingsStore.getState();
      expect(state.settings.appearance.theme).toBe("dark");
      expect(state.isSyncing).toBe(false);
      expect(mockUpdateSetting).toHaveBeenCalledWith("appearance", "theme", "dark");
    });
  });

  describe("resetSettings", () => {
    it("resets to defaults", async () => {
      // Start with non-default settings
      useSyncedSettingsStore.setState({ settings: DARK_SETTINGS });
      mockResetSyncedSettings.mockResolvedValue(DEFAULT_SETTINGS);

      await useSyncedSettingsStore.getState().resetSettings();

      const state = useSyncedSettingsStore.getState();
      expect(state.settings).toEqual(DEFAULT_SETTINGS);
      expect(state.isSyncing).toBe(false);
    });
  });

  describe("applySettingsFromEvent", () => {
    it("directly applies settings from Tauri event", () => {
      useSyncedSettingsStore.getState().applySettingsFromEvent(DARK_SETTINGS);

      expect(useSyncedSettingsStore.getState().settings).toEqual(DARK_SETTINGS);
    });
  });

  describe("user isolation", () => {
    it("signOut resets store to defaults (User A's data does not leak)", () => {
      // Simulate User A's settings in the store
      useSyncedSettingsStore.setState({ settings: DARK_SETTINGS });
      expect(useSyncedSettingsStore.getState().settings.appearance.theme).toBe("dark");

      // Simulate signOut — frontend should reset to defaults
      useSyncedSettingsStore.setState({ settings: DEFAULT_SETTINGS });

      const state = useSyncedSettingsStore.getState();
      expect(state.settings.appearance.theme).toBe("system");
      expect(state.settings.appearance.terminal_font_size).toBe(13);
      expect(state.settings.terminal.cursor_style).toBe("bar");
      expect(state.settings.notifications.sound_enabled).toBe(true);
    });

    it("loadSettings after auth replaces previous user's data entirely", async () => {
      // User A's settings in the store
      useSyncedSettingsStore.setState({ settings: DARK_SETTINGS });

      // User B logs in — loadSettings fetches User B's settings from server
      const userBSettings: UserSettings = {
        ...DEFAULT_SETTINGS,
        appearance: { theme: "light", shell_font: null, terminal_font_size: 14 },
        git: { default_base_branch: "develop" },
      };
      mockGetSyncedSettings.mockResolvedValue(userBSettings);

      await useSyncedSettingsStore.getState().loadSettings();

      const state = useSyncedSettingsStore.getState();
      // User B's settings should fully replace User A's
      expect(state.settings.appearance.theme).toBe("light");
      expect(state.settings.appearance.terminal_font_size).toBe(14);
      expect(state.settings.git.default_base_branch).toBe("develop");
      // User A's custom values should NOT be present
      expect(state.settings.editor.default_ide).toBeNull(); // B's default, not A's "cursor"
      expect(state.settings.notifications.sound_enabled).toBe(true); // B's default, not A's false
    });

    it("applySettingsFromEvent fully replaces, never merges", () => {
      // User A in store
      useSyncedSettingsStore.setState({ settings: DARK_SETTINGS });

      // Event arrives with User B's settings (only appearance changed, rest is defaults)
      const userBSettings: UserSettings = {
        ...DEFAULT_SETTINGS,
        appearance: { theme: "light", shell_font: null, terminal_font_size: 12 },
      };
      useSyncedSettingsStore.getState().applySettingsFromEvent(userBSettings);

      const state = useSyncedSettingsStore.getState();
      // Should be User B's full settings, not a merge
      expect(state.settings.appearance.theme).toBe("light");
      expect(state.settings.editor.default_ide).toBeNull(); // default, not A's "cursor"
      expect(state.settings.keyboard.shortcuts).toEqual({}); // default, not A's shortcuts
    });
  });

  describe("typed selectors", () => {
    it("selectTerminalFontSize returns font size", () => {
      useSyncedSettingsStore.setState({ settings: DARK_SETTINGS });
      expect(selectTerminalFontSize(useSyncedSettingsStore.getState())).toBe(16);
    });

    it("selectTerminalFontSize returns default", () => {
      expect(selectTerminalFontSize(useSyncedSettingsStore.getState())).toBe(13);
    });

    it("selectTerminalCursorStyle returns cursor style", () => {
      useSyncedSettingsStore.setState({ settings: DARK_SETTINGS });
      expect(selectTerminalCursorStyle(useSyncedSettingsStore.getState())).toBe("block");
    });

    it("selectDefaultEditor returns editor id", () => {
      useSyncedSettingsStore.setState({ settings: DARK_SETTINGS });
      expect(selectDefaultEditor(useSyncedSettingsStore.getState())).toBe("cursor");
    });

    it("selectDefaultEditor returns empty string when null", () => {
      expect(selectDefaultEditor(useSyncedSettingsStore.getState())).toBe("");
    });

    it("selectDefaultBaseBranch returns branch", () => {
      useSyncedSettingsStore.setState({ settings: DARK_SETTINGS });
      expect(selectDefaultBaseBranch(useSyncedSettingsStore.getState())).toBe("develop");
    });

    it("selectNotificationSoundEnabled returns boolean", () => {
      useSyncedSettingsStore.setState({ settings: DARK_SETTINGS });
      expect(selectNotificationSoundEnabled(useSyncedSettingsStore.getState())).toBe(false);
    });

    it("selectDesktopNotificationsEnabled returns boolean", () => {
      expect(selectDesktopNotificationsEnabled(useSyncedSettingsStore.getState())).toBe(true);
    });
  });

  describe("all setting types survive logout/login cycle", () => {
    it("every field restores after logout and login", async () => {
      // 1. User has custom settings
      useSyncedSettingsStore.setState({ settings: DARK_SETTINGS });

      // 2. Logout — reset to defaults
      useSyncedSettingsStore.setState({ settings: DEFAULT_SETTINGS });
      expect(useSyncedSettingsStore.getState().settings).toEqual(DEFAULT_SETTINGS);

      // 3. Login — server returns user's settings
      mockGetSyncedSettings.mockResolvedValue(DARK_SETTINGS);
      await useSyncedSettingsStore.getState().loadSettings();

      // 4. Every field restored
      const s = useSyncedSettingsStore.getState().settings;
      expect(s.appearance.theme).toBe("dark");
      expect(s.appearance.shell_font).toBe("Fira Code");
      expect(s.appearance.terminal_font_size).toBe(16);
      expect(s.editor.default_ide).toBe("cursor");
      expect(s.terminal.scrollback_limit).toBe(5000);
      expect(s.terminal.cursor_style).toBe("block");
      expect(s.git.default_base_branch).toBe("develop");
      expect(s.keyboard.shortcuts).toEqual({ "ctrl+s": "save" });
      expect(s.notifications.sound_enabled).toBe(false);
      expect(s.notifications.desktop_enabled).toBe(true);
    });
  });
});
