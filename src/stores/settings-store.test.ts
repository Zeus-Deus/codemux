import { describe, it, expect, vi, beforeEach } from "vitest";

// ── Mock Tauri commands ──

const mockDbGetAllSettings = vi.fn();
const mockDbSetSetting = vi.fn();

vi.mock("@/tauri/commands", () => ({
  dbGetAllSettings: (...args: unknown[]) => mockDbGetAllSettings(...args),
  dbSetSetting: (...args: unknown[]) => mockDbSetSetting(...args),
}));

// ── Mock synced-settings-store (needed for redirected per-user getters) ──

const mockSyncedState = {
  settings: {
    appearance: { theme: "system", shell_font: null as string | null, terminal_font_size: 13 },
    editor: { default_ide: null as string | null },
    terminal: { scrollback_limit: 10_000, cursor_style: "bar" },
    git: { default_base_branch: "main" },
    keyboard: { shortcuts: {} as Record<string, string> },
    notifications: { sound_enabled: true, desktop_enabled: true },
  },
};

const mockSyncedGetState = vi.fn(() => mockSyncedState);

vi.mock("./synced-settings-store", () => ({
  useSyncedSettingsStore: { getState: () => mockSyncedGetState() },
}));

import {
  useSettingsStore,
  SETTINGS_DEFAULTS,
  getTerminalCursorStyle,
  getTerminalColorTheme,
  selectLegacyTerminalFontFamily,
  getDefaultEditor,
  getDefaultBaseBranch,
  selectSidebarShowGitStats,
  selectSidebarAutoSettleDays,
  selectOrbMatchActivity,
} from "./settings-store";

function stateWith(settings: Record<string, string>) {
  return { loaded: true, settings } as Parameters<
    typeof selectSidebarShowGitStats
  >[0];
}

describe("settings-store", () => {
  beforeEach(() => {
    useSettingsStore.setState({ loaded: false, settings: {} });
    vi.clearAllMocks();
    mockDbSetSetting.mockResolvedValue(undefined);

    // Default: synced store returns default settings
    mockSyncedGetState.mockReturnValue(mockSyncedState);
  });

  // ── Machine-local defaults ──

  describe("machine-local defaults", () => {
    it("terminalColorTheme defaults to 'app'", () => {
      expect(getTerminalColorTheme()).toBe("app");
    });

    // The legacy machine-local terminal family is an override, not a default:
    // an install that never persisted one resolves from synced typography.
    it("selectLegacyTerminalFontFamily is undefined until a build persisted one", () => {
      expect(selectLegacyTerminalFontFamily(stateWith({}))).toBeUndefined();
      expect(
        selectLegacyTerminalFontFamily(stateWith({ "terminal.font_family": "Fira Code" })),
      ).toBe("Fira Code");
    });

    it("get() returns default for known key", () => {
      expect(useSettingsStore.getState().get("terminal.color_theme")).toBe("app");
    });

    it("get() returns empty string for key with no default", () => {
      expect(useSettingsStore.getState().get("nonexistent.key")).toBe("");
    });

    it("SETTINGS_DEFAULTS contains only machine-local keys", () => {
      expect(SETTINGS_DEFAULTS).toBeDefined();
      // Machine-local keys present
      expect(SETTINGS_DEFAULTS["terminal.color_theme"]).toBe("app");
      expect(SETTINGS_DEFAULTS["terminal.font_family"]).toBeDefined();
      expect(SETTINGS_DEFAULTS["ai_commit_message_enabled"]).toBe("true");
      expect(SETTINGS_DEFAULTS["auto_mcp_config"]).toBe("true");
      // Sidebar / agent-orb keys present
      expect(SETTINGS_DEFAULTS["sidebar.show_git_stats"]).toBe("true");
      expect(SETTINGS_DEFAULTS["sidebar.auto_settle_days"]).toBe("3");
      expect(SETTINGS_DEFAULTS["agents.orb_match_activity"]).toBe("true");
      // The retired working-indicator picker keys are gone, not migrated.
      expect(SETTINGS_DEFAULTS["sidebar.working_indicator"]).toBeUndefined();
      expect(
        SETTINGS_DEFAULTS["sidebar.working_indicator_color"],
      ).toBeUndefined();
      // Per-user keys NOT present
      expect(SETTINGS_DEFAULTS["terminal.font_size"]).toBeUndefined();
      expect(SETTINGS_DEFAULTS["terminal.cursor_style"]).toBeUndefined();
      expect(SETTINGS_DEFAULTS["editor.default"]).toBeUndefined();
      expect(SETTINGS_DEFAULTS["git.default_base_branch"]).toBeUndefined();
      expect(SETTINGS_DEFAULTS["notification_sound_enabled"]).toBeUndefined();
    });
  });

  // ── Sidebar / working-indicator selectors ──

  describe("sidebar selectors", () => {
    it("selectSidebarShowGitStats defaults to true", () => {
      expect(selectSidebarShowGitStats(stateWith({}))).toBe(true);
    });

    it("selectSidebarShowGitStats reflects a stored 'false' value", () => {
      expect(
        selectSidebarShowGitStats(
          stateWith({ "sidebar.show_git_stats": "false" }),
        ),
      ).toBe(false);
    });

    it("selectSidebarAutoSettleDays defaults to 3 days", () => {
      expect(selectSidebarAutoSettleDays(stateWith({}))).toBe(3);
    });

    it("selectSidebarAutoSettleDays returns null when set to 'off'", () => {
      expect(
        selectSidebarAutoSettleDays(
          stateWith({ "sidebar.auto_settle_days": "off" }),
        ),
      ).toBeNull();
    });

    it("selectSidebarAutoSettleDays reflects a stored day count", () => {
      expect(
        selectSidebarAutoSettleDays(
          stateWith({ "sidebar.auto_settle_days": "14" }),
        ),
      ).toBe(14);
    });

    it("selectOrbMatchActivity defaults to true", () => {
      expect(selectOrbMatchActivity(stateWith({}))).toBe(true);
    });

    it("selectOrbMatchActivity reflects a stored 'false' value", () => {
      expect(
        selectOrbMatchActivity(
          stateWith({ "agents.orb_match_activity": "false" }),
        ),
      ).toBe(false);
    });

    // Migration contract: an old settings row is dropped on read, not
    // mapped onto anything. Loading a pre-orb settings blob must neither
    // throw nor change how anything renders.
    it("ignores retired working-indicator keys left in stored settings", () => {
      const legacy = stateWith({
        "sidebar.working_indicator": "sweep",
        "sidebar.working_indicator_color": "accent-violet",
      });
      expect(selectOrbMatchActivity(legacy)).toBe(true);
      expect(selectSidebarShowGitStats(legacy)).toBe(true);
    });
  });

  // ── Redirected per-user getters (read from synced store) ──

  describe("per-user getters redirect to synced store", () => {
    it("getTerminalCursorStyle reads from synced store", () => {
      const defaults = mockSyncedState.settings;
      mockSyncedGetState.mockReturnValue({
        settings: {
          ...defaults,
          terminal: { ...defaults.terminal, cursor_style: "block" },
        },
      });
      expect(getTerminalCursorStyle()).toBe("block");
    });

    it("getDefaultEditor reads from synced store", () => {
      const defaults = mockSyncedState.settings;
      mockSyncedGetState.mockReturnValue({
        settings: {
          ...defaults,
          editor: { default_ide: "cursor" },
        },
      });
      expect(getDefaultEditor()).toBe("cursor");
    });

    it("getDefaultEditor returns empty string for null", () => {
      const defaults = mockSyncedState.settings;
      mockSyncedGetState.mockReturnValue({
        settings: {
          ...defaults,
          editor: { default_ide: null },
        },
      });
      expect(getDefaultEditor()).toBe("");
    });

    it("getDefaultBaseBranch reads from synced store", () => {
      const defaults = mockSyncedState.settings;
      mockSyncedGetState.mockReturnValue({
        settings: {
          ...defaults,
          git: { default_base_branch: "develop" },
        },
      });
      expect(getDefaultBaseBranch()).toBe("develop");
    });

    it("per-user getters return synced defaults when synced store is default", () => {
      expect(getTerminalCursorStyle()).toBe("bar");
      expect(getDefaultEditor()).toBe("");
      expect(getDefaultBaseBranch()).toBe("main");
    });
  });

  // ── Loading from database ──

  describe("load()", () => {
    it("loads saved values from database", async () => {
      mockDbGetAllSettings.mockResolvedValue({
        "terminal.color_theme": "system",
        "terminal.font_family": "Fira Code",
      });

      await useSettingsStore.getState().load();

      expect(useSettingsStore.getState().loaded).toBe(true);
      expect(getTerminalColorTheme()).toBe("system");
      expect(useSettingsStore.getState().get("terminal.font_family")).toBe("Fira Code");
    });

    it("sets loaded=true even with empty database", async () => {
      mockDbGetAllSettings.mockResolvedValue({});

      await useSettingsStore.getState().load();

      expect(useSettingsStore.getState().loaded).toBe(true);
    });

    it("sets loaded=true even on database error", async () => {
      mockDbGetAllSettings.mockRejectedValue(new Error("DB fail"));

      await useSettingsStore.getState().load();

      expect(useSettingsStore.getState().loaded).toBe(true);
    });

    it("always applies SQLite results (no race condition guard needed)", async () => {
      // Even if loaded is already true, load() still applies results
      useSettingsStore.setState({ loaded: true, settings: { "terminal.color_theme": "app" } });

      mockDbGetAllSettings.mockResolvedValue({ "terminal.color_theme": "system" });
      await useSettingsStore.getState().load();

      expect(getTerminalColorTheme()).toBe("system");
    });
  });

  // ── Setting values ──

  describe("set()", () => {
    it("updates the store state immediately", () => {
      useSettingsStore.getState().set("terminal.color_theme", "system");

      expect(getTerminalColorTheme()).toBe("system");
      expect(useSettingsStore.getState().get("terminal.color_theme")).toBe("system");
    });

    it("calls dbSetSetting to persist", () => {
      useSettingsStore.getState().set("terminal.color_theme", "system");

      expect(mockDbSetSetting).toHaveBeenCalledWith(
        "terminal.color_theme",
        "system",
      );
    });

    it("multiple sets are independent", () => {
      useSettingsStore.getState().set("terminal.color_theme", "system");
      useSettingsStore.getState().set("terminal.font_family", "Fira Code");

      expect(getTerminalColorTheme()).toBe("system");
      expect(useSettingsStore.getState().get("terminal.font_family")).toBe("Fira Code");
    });
  });
});
