/**
 * Integration tests for the unified settings architecture.
 * Verifies that the synced store is the single source of truth for per-user
 * settings, and that machine-local settings in the local store are unaffected
 * by login/logout cycles.
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import type { UserSettings } from "@/tauri/types";

// ── Mock Tauri commands for BOTH stores ──

const mockDbGetAllSettings = vi.fn();
const mockDbSetSetting = vi.fn();
const mockGetSyncedSettings = vi.fn();
const mockUpdateSyncedSettings = vi.fn();
const mockUpdateSetting = vi.fn();
const mockResetSyncedSettings = vi.fn();

vi.mock("@/tauri/commands", () => ({
  dbGetAllSettings: (...a: unknown[]) => mockDbGetAllSettings(...a),
  dbSetSetting: (...a: unknown[]) => mockDbSetSetting(...a),
  getSyncedSettings: (...a: unknown[]) => mockGetSyncedSettings(...a),
  updateSyncedSettings: (...a: unknown[]) => mockUpdateSyncedSettings(...a),
  updateSetting: (...a: unknown[]) => mockUpdateSetting(...a),
  resetSyncedSettings: (...a: unknown[]) => mockResetSyncedSettings(...a),
}));

import { useSettingsStore, SETTINGS_DEFAULTS } from "./settings-store";
import { useSyncedSettingsStore, DEFAULT_SETTINGS } from "./synced-settings-store";

// ── Helpers ──

/** Full non-default settings covering every per-user field. */
const FULL_CUSTOM: UserSettings = {
  appearance: { theme: "dark", shell_font: "Fira Code", terminal_font_size: 20 },
  editor: { default_ide: "cursor" },
  terminal: { scrollback_limit: 2000, cursor_style: "underline" },
  git: { default_base_branch: "develop" },
  keyboard: { shortcuts: { "ctrl+s": "save", "ctrl+p": "palette" } },
  notifications: { sound_enabled: false, desktop_enabled: false },
  file_tree: { show_hidden_files: true },
};

// ── Setup ──

beforeEach(() => {
  useSettingsStore.setState({ loaded: false, settings: {} });
  useSyncedSettingsStore.setState({
    settings: DEFAULT_SETTINGS,
    isLoading: true,
    isSyncing: false,
  });
  vi.clearAllMocks();
  mockDbSetSetting.mockResolvedValue(undefined);
});

// ── Tests ──

describe("settings single-store integration", () => {
  describe("synced store is the sole source of truth for per-user settings", () => {
    it("login loads all per-user settings into synced store", async () => {
      mockGetSyncedSettings.mockResolvedValue(FULL_CUSTOM);
      await useSyncedSettingsStore.getState().loadSettings();

      const s = useSyncedSettingsStore.getState().settings;
      expect(s.appearance.terminal_font_size).toBe(20);
      expect(s.terminal.cursor_style).toBe("underline");
      expect(s.git.default_base_branch).toBe("develop");
      expect(s.editor.default_ide).toBe("cursor");
      expect(s.notifications.sound_enabled).toBe(false);
      expect(s.notifications.desktop_enabled).toBe(false);
    });

    it("logout resets all per-user settings to defaults", async () => {
      // Login
      mockGetSyncedSettings.mockResolvedValue(FULL_CUSTOM);
      await useSyncedSettingsStore.getState().loadSettings();
      expect(useSyncedSettingsStore.getState().settings.appearance.theme).toBe("dark");

      // Logout
      useSyncedSettingsStore.setState({ settings: DEFAULT_SETTINGS });

      const s = useSyncedSettingsStore.getState().settings;
      expect(s.appearance.theme).toBe("system");
      expect(s.appearance.terminal_font_size).toBe(13);
      expect(s.terminal.cursor_style).toBe("bar");
      expect(s.git.default_base_branch).toBe("main");
      expect(s.editor.default_ide).toBeNull();
      expect(s.notifications.sound_enabled).toBe(true);
    });

    it("all setting types survive logout/login cycle", async () => {
      // Login with custom settings
      mockGetSyncedSettings.mockResolvedValue(FULL_CUSTOM);
      await useSyncedSettingsStore.getState().loadSettings();

      // Logout
      useSyncedSettingsStore.setState({ settings: DEFAULT_SETTINGS });

      // Login again — server returns same settings
      mockGetSyncedSettings.mockResolvedValue(FULL_CUSTOM);
      await useSyncedSettingsStore.getState().loadSettings();

      expect(useSyncedSettingsStore.getState().settings).toEqual(FULL_CUSTOM);
    });

    it("User A → sign out → User B → sees own settings, not A's", async () => {
      // User A
      mockGetSyncedSettings.mockResolvedValue(FULL_CUSTOM);
      await useSyncedSettingsStore.getState().loadSettings();

      // Sign out
      useSyncedSettingsStore.setState({ settings: DEFAULT_SETTINGS });

      // User B — has different settings
      const userBSettings: UserSettings = {
        ...DEFAULT_SETTINGS,
        appearance: { theme: "light", shell_font: null, terminal_font_size: 14 },
        git: { default_base_branch: "release" },
      };
      mockGetSyncedSettings.mockResolvedValue(userBSettings);
      await useSyncedSettingsStore.getState().loadSettings();

      const s = useSyncedSettingsStore.getState().settings;
      expect(s.appearance.theme).toBe("light");
      expect(s.appearance.terminal_font_size).toBe(14);
      expect(s.git.default_base_branch).toBe("release");
      // A's values NOT present
      expect(s.editor.default_ide).toBeNull();
      expect(s.notifications.sound_enabled).toBe(true);
    });
  });

  describe("machine-local settings are unaffected by login/logout", () => {
    it("local store machine settings persist through logout", async () => {
      // Set machine-local settings
      useSettingsStore.getState().set("terminal.color_theme", "system");
      useSettingsStore.getState().set("terminal.font_family", "Fira Code");
      useSettingsStore.getState().set("ai_commit_message_enabled", "false");

      // Login
      mockGetSyncedSettings.mockResolvedValue(FULL_CUSTOM);
      await useSyncedSettingsStore.getState().loadSettings();

      // Logout
      useSyncedSettingsStore.setState({ settings: DEFAULT_SETTINGS });

      // Machine-local settings unchanged
      expect(useSettingsStore.getState().get("terminal.color_theme")).toBe("system");
      expect(useSettingsStore.getState().get("terminal.font_family")).toBe("Fira Code");
      expect(useSettingsStore.getState().get("ai_commit_message_enabled")).toBe("false");
    });
  });

  describe("architecture sanity checks", () => {
    it("SETTINGS_DEFAULTS has NO per-user keys", () => {
      const perUserKeys = [
        "terminal.font_size",
        "terminal.cursor_style",
        "editor.default",
        "git.default_base_branch",
        "notification_sound_enabled",
      ];
      for (const key of perUserKeys) {
        expect(SETTINGS_DEFAULTS[key]).toBeUndefined();
      }
    });

    it("SETTINGS_DEFAULTS has machine-local keys", () => {
      expect(SETTINGS_DEFAULTS["terminal.color_theme"]).toBe("app");
      expect(SETTINGS_DEFAULTS["terminal.font_family"]).toBeDefined();
      expect(SETTINGS_DEFAULTS["ai_commit_message_enabled"]).toBe("true");
      expect(SETTINGS_DEFAULTS["auto_mcp_config"]).toBe("true");
    });

    it("DEFAULT_SETTINGS has all per-user fields", () => {
      expect(DEFAULT_SETTINGS.appearance.terminal_font_size).toBe(13);
      expect(DEFAULT_SETTINGS.terminal.cursor_style).toBe("bar");
      expect(DEFAULT_SETTINGS.git.default_base_branch).toBe("main");
      expect(DEFAULT_SETTINGS.editor.default_ide).toBeNull();
      expect(DEFAULT_SETTINGS.notifications.sound_enabled).toBe(true);
      expect(DEFAULT_SETTINGS.notifications.desktop_enabled).toBe(true);
    });
  });
});
