import { describe, it, expect, vi, beforeEach } from "vitest";

// ── Mock Tauri commands ──

const mockDbGetAllSettings = vi.fn();
const mockDbSetSetting = vi.fn();

vi.mock("@/tauri/commands", () => ({
  dbGetAllSettings: (...args: unknown[]) => mockDbGetAllSettings(...args),
  dbSetSetting: (...args: unknown[]) => mockDbSetSetting(...args),
}));

import {
  useSettingsStore,
  getTerminalFontSize,
  getTerminalCursorStyle,
  getTerminalColorTheme,
  getTerminalFontFamily,
  getDefaultEditor,
  getDefaultBaseBranch,
} from "./settings-store";

describe("settings-store", () => {
  beforeEach(() => {
    useSettingsStore.setState({ loaded: false, settings: {} });
    vi.clearAllMocks();
    mockDbSetSetting.mockResolvedValue(undefined);
  });

  // ── Defaults ──

  describe("defaults when no saved values exist", () => {
    it("terminalFontSize defaults to 13", () => {
      expect(getTerminalFontSize()).toBe(13);
    });

    it("terminalCursorStyle defaults to 'bar'", () => {
      expect(getTerminalCursorStyle()).toBe("bar");
    });

    it("terminalColorTheme defaults to 'app'", () => {
      expect(getTerminalColorTheme()).toBe("app");
    });

    it("terminalFontFamily defaults to JetBrains Mono", () => {
      expect(getTerminalFontFamily()).toBe(
        "'JetBrains Mono Variable', monospace",
      );
    });

    it("defaultEditor defaults to empty string", () => {
      expect(getDefaultEditor()).toBe("");
    });

    it("defaultBaseBranch defaults to 'main'", () => {
      expect(getDefaultBaseBranch()).toBe("main");
    });

    it("get() returns default for known key", () => {
      expect(useSettingsStore.getState().get("terminal.font_size")).toBe("13");
    });

    it("get() returns empty string for key with no default", () => {
      expect(useSettingsStore.getState().get("nonexistent.key")).toBe("");
    });
  });

  // ── Loading from database ──

  describe("load()", () => {
    it("loads saved values from database", async () => {
      mockDbGetAllSettings.mockResolvedValue({
        "terminal.font_size": "16",
        "terminal.cursor_style": "block",
        "editor.default": "cursor",
      });

      await useSettingsStore.getState().load();

      expect(useSettingsStore.getState().loaded).toBe(true);
      expect(getTerminalFontSize()).toBe(16);
      expect(getTerminalCursorStyle()).toBe("block");
      expect(getDefaultEditor()).toBe("cursor");
    });

    it("sets loaded=true even with empty database", async () => {
      mockDbGetAllSettings.mockResolvedValue({});

      await useSettingsStore.getState().load();

      expect(useSettingsStore.getState().loaded).toBe(true);
      expect(getTerminalFontSize()).toBe(13);
    });

    it("sets loaded=true even on database error", async () => {
      mockDbGetAllSettings.mockRejectedValue(new Error("DB fail"));

      await useSettingsStore.getState().load();

      expect(useSettingsStore.getState().loaded).toBe(true);
    });

    it("typed getters return saved values over defaults", async () => {
      mockDbGetAllSettings.mockResolvedValue({
        "terminal.font_family": "Fira Code",
        "git.default_base_branch": "develop",
        "terminal.color_theme": "system",
      });

      await useSettingsStore.getState().load();

      expect(getTerminalFontFamily()).toBe("Fira Code");
      expect(getDefaultBaseBranch()).toBe("develop");
      expect(getTerminalColorTheme()).toBe("system");
    });
  });

  // ── Setting values ──

  describe("set()", () => {
    it("updates the store state immediately", () => {
      useSettingsStore.getState().set("terminal.font_size", "18");

      expect(getTerminalFontSize()).toBe(18);
      expect(useSettingsStore.getState().get("terminal.font_size")).toBe("18");
    });

    it("calls dbSetSetting to persist", () => {
      useSettingsStore.getState().set("terminal.cursor_style", "block");

      expect(mockDbSetSetting).toHaveBeenCalledWith(
        "terminal.cursor_style",
        "block",
      );
    });

    it("multiple sets are independent", () => {
      useSettingsStore.getState().set("terminal.font_size", "16");
      useSettingsStore.getState().set("terminal.cursor_style", "underline");

      expect(getTerminalFontSize()).toBe(16);
      expect(getTerminalCursorStyle()).toBe("underline");
      expect(getDefaultBaseBranch()).toBe("main");
    });

    it("overwrites previous value", () => {
      useSettingsStore.getState().set("editor.default", "code");
      expect(getDefaultEditor()).toBe("code");

      useSettingsStore.getState().set("editor.default", "cursor");
      expect(getDefaultEditor()).toBe("cursor");
    });
  });
});
