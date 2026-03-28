import { describe, it, expect, vi, beforeEach, beforeAll } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";

// Polyfill ResizeObserver for jsdom (used by Radix Slider)
beforeAll(() => {
  if (typeof globalThis.ResizeObserver === "undefined") {
    globalThis.ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    } as unknown as typeof ResizeObserver;
  }
});

// ── Mocks ──

const mockSetShowSettings = vi.fn();
const mockSignOut = vi.fn();

vi.mock("@/stores/ui-store", () => ({
  useUIStore: (sel: (s: Record<string, unknown>) => unknown) =>
    sel({
      setShowSettings: mockSetShowSettings,
      settingsSection: null,
    }),
}));

vi.mock("@/stores/app-store", () => ({
  useAppStore: (sel: (s: Record<string, unknown>) => unknown) =>
    sel({
      appState: {
        config: { notification_sound_enabled: true, ai_commit_message_enabled: true, ai_resolver_enabled: false },
        active_workspace_id: "ws-1",
        workspaces: [{ workspace_id: "ws-1", project_root: "/tmp/proj" }],
      },
    }),
}));

vi.mock("@/stores/settings-store", () => {
  const defaults: Record<string, string> = {
    "terminal.color_theme": "app",
    "terminal.font_family": "'JetBrains Mono Variable', monospace",
    auto_mcp_config: "true",
  };
  return {
    useSettingsStore: (sel: (s: Record<string, unknown>) => unknown) =>
      sel({
        set: vi.fn(),
        get: (key: string) => defaults[key] ?? "",
        settings: defaults,
      }),
    selectTerminalColorTheme: () => "app",
  };
});

vi.mock("@/stores/auth-store", () => ({
  useAuthStore: (sel: (s: Record<string, unknown>) => unknown) =>
    sel({
      user: { id: "u1", email: "test@codemux.org", name: "Test User", image: null },
      isAuthenticated: true,
      devBypass: false,
      signOut: mockSignOut,
    }),
}));

vi.mock("@/stores/synced-settings-store", () => ({
  useSyncedSettingsStore: (sel: (s: Record<string, unknown>) => unknown) =>
    sel({
      settings: {
        appearance: { theme: "system", shell_font: null, terminal_font_size: 13 },
        editor: { default_ide: null },
        terminal: { scrollback_limit: 10000, cursor_style: "bar" },
        git: { default_base_branch: "main" },
        keyboard: { shortcuts: {} },
        notifications: { sound_enabled: true, desktop_enabled: true },
      },
      updateSetting: vi.fn(),
    }),
  selectTerminalFontSize: () => 13,
  selectTerminalCursorStyle: () => "bar",
  selectDefaultEditor: () => "",
  selectDefaultBaseBranch: () => "main",
  selectNotificationSoundEnabled: () => true,
  selectDesktopNotificationsEnabled: () => true,
}));

vi.mock("@/tauri/commands", () => ({
  detectEditors: vi.fn().mockResolvedValue([]),
  setNotificationSoundEnabled: vi.fn().mockResolvedValue(undefined),
  setAiCommitMessageEnabled: vi.fn().mockResolvedValue(undefined),
  setAiCommitMessageModel: vi.fn().mockResolvedValue(undefined),
  setAiResolverEnabled: vi.fn().mockResolvedValue(undefined),
  setAiResolverCli: vi.fn().mockResolvedValue(undefined),
  setAiResolverModel: vi.fn().mockResolvedValue(undefined),
  setAiResolverStrategy: vi.fn().mockResolvedValue(undefined),
  getProjectScripts: vi.fn().mockResolvedValue(null),
  setProjectScripts: vi.fn().mockResolvedValue(undefined),
  getWorkspaceConfig: vi.fn().mockResolvedValue(null),
  getPresets: vi.fn().mockResolvedValue({ presets: [], bar_visible: true, default_preset_id: null }),
  setPresetPinned: vi.fn().mockResolvedValue(undefined),
  setPresetBarVisible: vi.fn().mockResolvedValue(undefined),
  deletePreset: vi.fn().mockResolvedValue(undefined),
  updatePreset: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@/tauri/events", () => ({
  onPresetsChanged: vi.fn().mockReturnValue(Promise.resolve(() => {})),
}));

import { SettingsView } from "./settings-view";

describe("SettingsPanel", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("renders nav with all section groups", () => {
    render(<SettingsView />);

    // Group headers
    expect(screen.getByText("PERSONAL")).toBeInTheDocument();
    expect(screen.getByText("EDITOR & WORKFLOW")).toBeInTheDocument();

    // Nav items
    expect(screen.getByRole("button", { name: /Account/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Appearance/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Notifications/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Shortcuts/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Editor/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Terminal/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Git/i })).toBeInTheDocument();
  });

  it("shows Account section by default with user info", () => {
    render(<SettingsView />);

    // ScrollArea may duplicate content for measurement; check at least one exists
    expect(screen.getAllByText("test@codemux.org").length).toBeGreaterThan(0);
    expect(screen.getAllByText("Test User").length).toBeGreaterThan(0);
  });

  it("clicking nav items switches content", () => {
    render(<SettingsView />);

    // ScrollArea may duplicate nav buttons; click the first match (the actual nav button)
    const appearanceButtons = screen.getAllByRole("button", { name: /Appearance/i });
    fireEvent.click(appearanceButtons[0]);
    expect(screen.getAllByText("Theme preset").length).toBeGreaterThan(0);

    const terminalButtons = screen.getAllByRole("button", { name: /Terminal/i });
    fireEvent.click(terminalButtons[0]);
    expect(screen.getAllByText("Cursor style").length).toBeGreaterThan(0);
  });

  it("back button closes settings overlay", () => {
    render(<SettingsView />);

    // The first ghost button is the back arrow
    const backButton = screen.getAllByRole("button").find(
      (btn) => btn.querySelector("svg.lucide-arrow-left") !== null,
    );
    expect(backButton).toBeDefined();
    fireEvent.click(backButton!);

    expect(mockSetShowSettings).toHaveBeenCalledWith(false);
  });

  it("sign out button calls signOut and closes settings", () => {
    render(<SettingsView />);

    // Multiple sign out buttons may render due to ScrollArea; click the first
    const signOutButtons = screen.getAllByRole("button", { name: /Sign out/i });
    fireEvent.click(signOutButtons[0]);

    expect(mockSignOut).toHaveBeenCalled();
    expect(mockSetShowSettings).toHaveBeenCalledWith(false);
  });
});
