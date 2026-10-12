import { describe, it, expect, vi, beforeEach } from "vitest";

// ── Mock Tauri commands ──

const mockGetSyncedSettings = vi.fn();
const mockUpdateSyncedSettings = vi.fn();
const mockUpdateSetting = vi.fn();
const mockResetSyncedSettings = vi.fn();

vi.mock("@/tauri/commands", () => {
  const result = async (request: Promise<UserSettings | SettingsPersistence>) => {
    const value = await request;
    return "settings" in value ? value : { settings: value, synced: true };
  };
  return {
    getSyncedSettings: (...args: unknown[]) => result(mockGetSyncedSettings(...args)),
    updateSyncedSettings: (...args: unknown[]) => result(mockUpdateSyncedSettings(...args)),
    updateSetting: (...args: unknown[]) => result(mockUpdateSetting(...args)),
    resetSyncedSettings: (...args: unknown[]) => result(mockResetSyncedSettings(...args)),
  };
});

import {
  useSyncedSettingsStore,
  DEFAULT_SETTINGS,
  selectTerminalCursorStyle,
  selectDefaultEditor,
  selectDefaultBaseBranch,
  selectNotificationSoundEnabled,
  selectDesktopNotificationsEnabled,
  selectBackgroundBrowserDesktopViewport,
  selectBrowserDefaultViewport,
  parseViewportString,
} from "./synced-settings-store";
import type { UserSettings, SettingsPersistence } from "@/tauri/types";

const DARK_SETTINGS: UserSettings = {
  appearance: { theme: "dark", shell_font: "Fira Code", terminal_font_size: 16, show_resource_monitor: true },
  editor: { default_ide: "cursor" },
  terminal: { scrollback_limit: 5000, cursor_style: "block" },
  git: { default_base_branch: "develop" },
  source_control: { custom_hosts: {} },
  keyboard: { shortcuts: { "ctrl+s": "save" } },
  notifications: { sound_enabled: false, desktop_enabled: true },
  file_tree: { show_hidden_files: true },
  session_restore: { enabled: true, scrollback_lines: 10000, max_total_mb: 100 },
  agent_chat: { checkpoints_enabled: true, background_browser_desktop_viewport: false },
  browser: { default_viewport: "2560x1440" },
};

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}

describe("synced-settings-store", () => {
  beforeEach(() => {
    useSyncedSettingsStore.getState().replaceSessionSettings(DEFAULT_SETTINGS);
    useSyncedSettingsStore.setState({
      settings: DEFAULT_SETTINGS,
      isLoading: true,
      isSyncing: false,
      syncIssue: null,
    });
    vi.resetAllMocks();
  });

  describe("initial state", () => {
    it("starts with default settings", () => {
      const { settings } = useSyncedSettingsStore.getState();
      expect(settings.appearance.theme).toBe("default");
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
    it("reports an offline full write and clears the queue only after confirmed reconciliation", async () => {
      mockUpdateSyncedSettings.mockResolvedValue({ settings: DARK_SETTINGS, synced: false });
      await useSyncedSettingsStore.getState().updateSettings(DARK_SETTINGS);
      expect(useSyncedSettingsStore.getState().syncIssue).toBe("saved-locally");

      mockGetSyncedSettings.mockResolvedValue({ settings: DARK_SETTINGS, synced: false });
      await useSyncedSettingsStore.getState().loadSettings();
      expect(useSyncedSettingsStore.getState().syncIssue).toBe("saved-locally");
      mockGetSyncedSettings.mockResolvedValue({ settings: DARK_SETTINGS, synced: true });
      await useSyncedSettingsStore.getState().loadSettings();
      expect(useSyncedSettingsStore.getState().syncIssue).toBeNull();
    });

    it("does not let an older full write clear a newer failed reset", async () => {
      const old = deferred<UserSettings>();
      mockUpdateSyncedSettings.mockReturnValue(old.promise);
      const write = useSyncedSettingsStore.getState().updateSettings(DARK_SETTINGS);
      mockResetSyncedSettings.mockRejectedValue(new Error("reset failed"));
      await useSyncedSettingsStore.getState().resetSettings();
      old.resolve(DARK_SETTINGS);
      await write;
      expect(useSyncedSettingsStore.getState().syncIssue).toBe("failed");
    });
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
    it("does not dispatch a fallback after the request's account was replaced", async () => {
      const old = deferred<UserSettings>();
      mockUpdateSetting.mockReturnValue(old.promise);
      const write = useSyncedSettingsStore.getState().updateSetting("appearance", "theme", "old-account");
      useSyncedSettingsStore.getState().replaceSessionSettings(DARK_SETTINGS);
      old.reject(new Error("account changed"));
      await write;
      expect(mockUpdateSyncedSettings).not.toHaveBeenCalled();
      expect(useSyncedSettingsStore.getState().settings).toEqual(DARK_SETTINGS);
      expect(useSyncedSettingsStore.getState().syncIssue).toBeNull();
    });

    it("reports an online full fallback as synced", async () => {
      mockUpdateSetting.mockRejectedValue(new Error("PATCH rejected"));
      mockUpdateSyncedSettings.mockImplementation(async (settings: UserSettings) => ({ settings, synced: true }));
      await useSyncedSettingsStore.getState().updateSetting("git", "default_base_branch", "dev");
      expect(useSyncedSettingsStore.getState().syncIssue).toBeNull();
    });

    it("does not let an older PATCH success hide a newer failed edit", async () => {
      const old = deferred<UserSettings>();
      mockUpdateSetting.mockReturnValueOnce(old.promise).mockRejectedValueOnce(new Error("PATCH failed"));
      mockUpdateSyncedSettings.mockRejectedValue(new Error("PUT failed"));
      const write = useSyncedSettingsStore.getState().updateSetting("git", "default_base_branch", "dev");
      await useSyncedSettingsStore.getState().updateSetting("terminal", "cursor_style", "block");
      expect(useSyncedSettingsStore.getState().syncIssue).toBe("failed");
      old.resolve({ ...DEFAULT_SETTINGS, git: { default_base_branch: "dev" } });
      await write;
      expect(useSyncedSettingsStore.getState().syncIssue).toBe("failed");
      expect(useSyncedSettingsStore.getState().isSyncing).toBe(false);

      // Even a newer unrelated PATCH cannot confirm the failed cursor edit.
      mockUpdateSetting.mockResolvedValue({ ...DEFAULT_SETTINGS, git: { default_base_branch: "other" } });
      await useSyncedSettingsStore.getState().updateSetting("git", "default_base_branch", "other");
      expect(useSyncedSettingsStore.getState().syncIssue).toBe("failed");
    });

    it("keeps a fallback scoped to its account until it completes", async () => {
      const fallback = deferred<SettingsPersistence>();
      mockUpdateSetting.mockRejectedValue(new Error("PATCH failed"));
      mockUpdateSyncedSettings.mockReturnValue(fallback.promise);
      const write = useSyncedSettingsStore.getState().updateSetting("git", "default_base_branch", "dev");
      await vi.waitFor(() => expect(mockUpdateSyncedSettings).toHaveBeenCalled());
      useSyncedSettingsStore.getState().replaceSessionSettings(DARK_SETTINGS);
      fallback.resolve({ settings: DEFAULT_SETTINGS, synced: false });
      await write;
      expect(useSyncedSettingsStore.getState().syncIssue).toBeNull();
      expect(useSyncedSettingsStore.getState().settings).toEqual(DARK_SETTINGS);
    });

    it("reports a signed-out PATCH as locally queued", async () => {
      mockUpdateSetting.mockResolvedValue({ settings: DARK_SETTINGS, synced: false });
      await useSyncedSettingsStore.getState().updateSetting("appearance", "theme", "dark");
      expect(useSyncedSettingsStore.getState().syncIssue).toBe("saved-locally");
      useSyncedSettingsStore.getState().applySettingsFromEvent(DARK_SETTINGS, false);
      expect(useSyncedSettingsStore.getState().syncIssue).toBe("saved-locally");
      useSyncedSettingsStore.getState().applySettingsFromEvent(DARK_SETTINGS, true);
      expect(useSyncedSettingsStore.getState().syncIssue).toBeNull();
    });
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
      expect(state.syncIssue).toBeNull();
    });

    it("falls back to a full write that keeps the edit locally when the single-field write fails", async () => {
      mockUpdateSetting.mockRejectedValue("Network error: offline");
      mockUpdateSyncedSettings.mockImplementation(async (settings: UserSettings) => ({ settings, synced: false }));

      await useSyncedSettingsStore.getState().updateSetting("terminal", "cursor_style", "block");

      // The full write carries the edit the user made, not the old value.
      expect(mockUpdateSyncedSettings).toHaveBeenCalledTimes(1);
      expect(mockUpdateSyncedSettings.mock.calls[0][0].terminal.cursor_style).toBe("block");
      const state = useSyncedSettingsStore.getState();
      expect(state.settings.terminal.cursor_style).toBe("block");
      expect(state.syncIssue).toBe("saved-locally");
      expect(state.isSyncing).toBe(false);
    });

    it("reports a write that could not be saved anywhere, and clears it on the next success", async () => {
      mockUpdateSetting.mockRejectedValueOnce("API error: 500");
      mockUpdateSyncedSettings.mockRejectedValueOnce("API error: 500");

      await useSyncedSettingsStore.getState().updateSetting("git", "default_base_branch", "dev");
      expect(useSyncedSettingsStore.getState().syncIssue).toBe("failed");

      mockUpdateSetting.mockResolvedValueOnce({
        ...DEFAULT_SETTINGS,
        git: { default_base_branch: "dev" },
      });
      await useSyncedSettingsStore.getState().updateSetting("git", "default_base_branch", "dev");
      expect(useSyncedSettingsStore.getState().syncIssue).toBeNull();
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

    it("does not let a delayed remote refresh clobber a newer optimistic write", async () => {
      let resolveWrite!: (settings: UserSettings) => void;
      mockUpdateSetting.mockReturnValue(
        new Promise<UserSettings>((resolve) => {
          resolveWrite = resolve;
        }),
      );
      const remoteToken = useSyncedSettingsStore
        .getState()
        .remoteReconcileToken();
      const write = useSyncedSettingsStore
        .getState()
        .updateSetting("appearance", "theme", "local-newer");

      expect(
        useSyncedSettingsStore
          .getState()
          .reconcileRemoteSettings(DARK_SETTINGS, remoteToken),
      ).toBe(false);
      useSyncedSettingsStore
        .getState()
        .finishRemoteReconcile(remoteToken);
      expect(useSyncedSettingsStore.getState().settings.appearance.theme).toBe(
        "local-newer",
      );

      resolveWrite(DARK_SETTINGS);
      await write;
      expect(useSyncedSettingsStore.getState().settings.appearance.theme).toBe(
        "local-newer",
      );
    });

    it("does not let the previous account's write block the next account's settings", async () => {
      let resolveOldWrite!: (settings: UserSettings) => void;
      mockUpdateSetting.mockReturnValue(
        new Promise<UserSettings>((resolve) => {
          resolveOldWrite = resolve;
        }),
      );
      const oldWrite = useSyncedSettingsStore
        .getState()
        .updateSetting("appearance", "theme", "user-a-pending");

      // User A signs out and User B signs in while A's backend request is
      // still pending. B's refresh begins after the session replacement.
      useSyncedSettingsStore
        .getState()
        .replaceSessionSettings(DEFAULT_SETTINGS);
      const userBToken = useSyncedSettingsStore
        .getState()
        .remoteReconcileToken();
      const userBSettings: UserSettings = {
        ...DEFAULT_SETTINGS,
        appearance: {
          ...DEFAULT_SETTINGS.appearance,
          theme: "user-b",
        },
      };

      expect(
        useSyncedSettingsStore
          .getState()
          .reconcileRemoteSettings(userBSettings, userBToken),
      ).toBe(true);
      useSyncedSettingsStore
        .getState()
        .finishRemoteReconcile(userBToken);
      expect(useSyncedSettingsStore.getState().settings.appearance.theme).toBe(
        "user-b",
      );

      // A's late response must neither replace B's settings nor disturb B's
      // session-scoped reconciliation bookkeeping.
      resolveOldWrite(DARK_SETTINGS);
      await oldWrite;
      expect(useSyncedSettingsStore.getState().settings.appearance.theme).toBe(
        "user-b",
      );
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
      expect(state.settings.appearance.theme).toBe("default");
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
        appearance: { theme: "light", shell_font: null, terminal_font_size: 14, show_resource_monitor: true },
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
        appearance: { theme: "light", shell_font: null, terminal_font_size: 12, show_resource_monitor: true },
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

    it("selectBackgroundBrowserDesktopViewport returns boolean", () => {
      useSyncedSettingsStore.setState({ settings: DARK_SETTINGS });
      expect(selectBackgroundBrowserDesktopViewport(useSyncedSettingsStore.getState())).toBe(false);
    });

    it("selectBackgroundBrowserDesktopViewport defaults to true", () => {
      expect(selectBackgroundBrowserDesktopViewport(useSyncedSettingsStore.getState())).toBe(true);
    });

    it("selectBrowserDefaultViewport returns the raw spec string", () => {
      useSyncedSettingsStore.setState({ settings: DARK_SETTINGS });
      expect(selectBrowserDefaultViewport(useSyncedSettingsStore.getState())).toBe("2560x1440");
    });

    it("selectBrowserDefaultViewport defaults to null", () => {
      expect(selectBrowserDefaultViewport(useSyncedSettingsStore.getState())).toBe(null);
    });
  });

  describe("parseViewportString", () => {
    it("parses WxH strings", () => {
      expect(parseViewportString("2560x1440")).toEqual({ width: 2560, height: 1440 });
      expect(parseViewportString(" 1920X1080 ")).toEqual({ width: 1920, height: 1080 });
    });

    it("returns null for unset, preset names, and garbage", () => {
      expect(parseViewportString(null)).toBe(null);
      expect(parseViewportString("")).toBe(null);
      expect(parseViewportString("desktop-large")).toBe(null);
      expect(parseViewportString("0x0")).toBe(null);
      expect(parseViewportString("99999x99999")).toBe(null);
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
