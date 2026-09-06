import { describe, it, expect, vi, beforeEach, beforeAll } from "vitest";
import { render, screen, fireEvent, within } from "@testing-library/react";

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
const mockSettingsSet = vi.fn();

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
        config: {
          notification_sound_enabled: true,
          ai_commit_message_enabled: true,
          ai_commit_message_cli: "claude",
          ai_commit_message_model: null,
          ai_resolver_enabled: false,
          ai_resolver_cli: "claude",
          ai_resolver_model: null,
          ai_resolver_strategy: "smart_merge",
        },
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
    "appearance.palette": "cool",
    "appearance.density": "comfortable",
    "appearance.smooth_scrolling": "false",
    "sidebar.show_git_stats": "true",
    "sidebar.auto_settle_days": "3",
    "agents.orb_match_activity": "true",
    "chat.code_wrap": "false",
  };
  // Built lazily: `mockSettingsSet` is hoisted below this factory.
  const state = () => ({
    set: mockSettingsSet,
    get: (key: string) => defaults[key] ?? "",
    settings: defaults,
  });
  return {
    SETTINGS_DEFAULTS: defaults,
    // `getState` too: the commit-message row resolves the Utility agent
    // through `utilitySelectionFromStores()` while rendering.
    useSettingsStore: Object.assign(
      (sel: (s: Record<string, unknown>) => unknown) => sel(state()),
      { getState: state },
    ),
    selectTerminalColorTheme: () => "app",
    selectPalette: () => "cool",
    selectDensity: () => "comfortable",
    selectSmoothScrolling: () => false,
    selectSidebarShowGitStats: () => true,
    selectChatCodeWrap: () => false,
    selectSidebarAutoSettleDays: () => 3,
    selectOrbMatchActivity: () => true,
  };
});

vi.mock("@/stores/auth-store", () => ({
  useAuthStore: (sel: (s: Record<string, unknown>) => unknown) =>
    sel({
      user: { id: "u1", email: "test@codemux.org", name: "Test User", image: null },
      isAuthenticated: true,
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
  setAiCommitMessageCli: vi.fn().mockResolvedValue(undefined),
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
  // Usage section — needed so switching to it renders rather than
  // throwing on an undefined command wrapper.
  usageSummary: vi.fn().mockResolvedValue(null),
  usageExportCsv: vi.fn().mockResolvedValue(""),
}));

vi.mock("@/tauri/events", () => ({
  onPresetsChanged: vi.fn().mockReturnValue(Promise.resolve(() => {})),
}));

// ── Model picker mock ──
//
// Both settings rows use `MultiProviderModelPicker`, each pinned to the
// providers their Rust backend can actually run (`build_resolver_argv`:
// claude / codex / opencode). We mock the picker as a thin shell that:
//   - surfaces `allowedProviders` as a data attribute so tests can pin
//     the allowlist each row passes
//   - fires a cross-provider change tuple on click (the LAST allowed
//     provider, which is never the seeded one) so each row's test can
//     prove both setters are invoked atomically.
//
// The picker's real internals (search, rail, error states, loading
// skeletons, favorites) are covered by chat-side tests. Reusing the
// real component in jsdom needs a full Radix Popover + cmdk environment
// we don't control from this test.

vi.mock("@/components/chat/pickers/MultiProviderModelPicker", () => ({
  MultiProviderModelPicker: ({
    provider,
    model,
    onProviderModelChange,
    allowedProviders,
  }: {
    provider: string;
    model: string | null;
    onProviderModelChange: (provider: string, model: string) => void;
    allowedProviders?: ReadonlyArray<string>;
  }) => {
    const allowed =
      Array.isArray(allowedProviders) && allowedProviders.length > 0
        ? allowedProviders
        : null;
    const allowedKey = allowed ? allowed.join(",") : "all";
    return (
      <button
        type="button"
        data-testid="multi-provider-model-picker"
        data-provider={provider}
        data-model={model ?? ""}
        data-allowed={allowedKey}
        onClick={() => {
          // Simulate flipping to a DIFFERENT provider so each test can
          // verify the cli setter and the model setter fire atomically.
          // A restricted picker switches within its own allowlist.
          const next = allowed ? allowed[allowed.length - 1]! : "opencode";
          onProviderModelChange(next, "anthropic/claude-3-5-sonnet");
        }}
      >
        MultiPicker:{provider}/{model ?? "default"}/{allowedKey}
      </button>
    );
  },
}));

import { SettingsView } from "./settings-view";
import * as commands from "@/tauri/commands";

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
    expect(screen.getAllByText("Typography").length).toBeGreaterThan(0);

    const terminalButtons = screen.getAllByRole("button", { name: /Terminal/i });
    fireEvent.click(terminalButtons[0]);
    expect(screen.getAllByText("Cursor style").length).toBeGreaterThan(0);
  });

  it("gives Usage a wide content column and everything else the reading measure", () => {
    const { container } = render(<SettingsView />);

    // The shell is one tree with a per-section width, so locate the
    // content wrapper by the padding it always carries.
    const wrapper = () =>
      container.querySelector("div.mx-auto.px-11.pt-8.pb-20") as HTMLElement;

    // Default section (Account) is a form — reading measure.
    expect(wrapper()).toHaveClass("max-w-3xl");

    // Scope every query to THIS render: the file has no per-test
    // cleanup, so earlier renders are still mounted and a `screen`
    // lookup would drive one of those instead.
    const usageButtons = within(container).getAllByRole("button", {
      name: /^Usage$/i,
    });
    fireEvent.click(usageButtons[0]);
    // Usage is a dashboard — wide column.
    expect(wrapper()).toHaveClass("max-w-[1400px]");
    expect(wrapper()).not.toHaveClass("max-w-3xl");

    // Switching away restores the reading measure.
    const terminalButtons = within(container).getAllByRole("button", {
      name: /^Terminal$/i,
    });
    fireEvent.click(terminalButtons[0]);
    expect(wrapper()).toHaveClass("max-w-3xl");
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

// ── Git section: model pickers wiring ──
//
// Verifies the freeform "Model override" Inputs were replaced with the
// shared chat pickers, and that selecting a model writes through the
// right Tauri setters. The pickers' own behavior (search, rail, error
// states) is covered by chat tests; here we only verify settings calls
// the right setters when the picker fires.

describe("SettingsPanel — Git section model pickers", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  function openGitSection() {
    render(<SettingsView />);
    const gitButtons = screen.getAllByRole("button", { name: /Git/i });
    fireEvent.click(gitButtons[0]);
  }

  /** Both pickers in the Git section use the same shared
   *  `MultiProviderModelPicker`, both pinned to the CLIs
   *  `build_resolver_argv` actually dispatches (claude / codex /
   *  opencode). Distinguish them by document order: the FIRST live picker is the
   *  "Used to generate commit messages" row, the SECOND is the merge
   *  resolver row. ScrollArea duplicates trees for measurement, so we
   *  filter to the unique instances by stable side-effect (the
   *  duplicates render the same data-* attributes but are positioned
   *  off-screen — `getAllByTestId` returns them all; we take 0 and N
   *  where N is the next live one). For our purposes, since both
   *  duplicates fire the same wiring, picking either copy of each
   *  picker works. */
  function getPickers(): HTMLElement[] {
    const all = screen.getAllByTestId("multi-provider-model-picker");
    expect(all.length).toBeGreaterThanOrEqual(2);
    return all;
  }

  it("renders MultiProviderModelPicker for both rows, both seeded with the current cli + model", () => {
    // Both rows use the same picker (same component, same favorites
    // store, identical visual style) and both offer the same drivers.
    openGitSection();

    const pickers = getPickers();
    // Mock app-store seeds both rows with provider="claude", model=null.
    expect(pickers[0]!.getAttribute("data-provider")).toBe("claude");
    expect(pickers[1]!.getAttribute("data-provider")).toBe("claude");
    // Both rows are pinned to the CLIs `build_resolver_argv` knows.
    // Cursor is a chat-only provider: it has no non-interactive mode,
    // and an unrecognized cli silently falls through to the claude arm,
    // which would run `claude` with a Cursor model slug.
    expect(pickers[0]!.getAttribute("data-allowed")).toBe(
      "claude,codex,opencode",
    );
    expect(pickers[1]!.getAttribute("data-allowed")).toBe(
      "claude,codex,opencode",
    );
  });

  it("clicking the commits picker writes BOTH ai_commit_message_cli AND ai_commit_message_model atomically", () => {
    // Atomicity guarantee for the commits row, mirroring what the
    // resolver row already had. The Rust side now supports arbitrary
    // CLIs for commit-message generation (`generate_commit_message`
    // dispatches via `build_resolver_argv`), so both fields must
    // travel together — picking an OpenCode model while leaving the
    // CLI on "claude" would crash the generator.
    openGitSection();
    fireEvent.click(getPickers()[0]!);

    expect(vi.mocked(commands.setAiCommitMessageCli)).toHaveBeenCalledWith(
      "opencode",
    );
    expect(vi.mocked(commands.setAiCommitMessageModel)).toHaveBeenCalledWith(
      "anthropic/claude-3-5-sonnet",
    );
    // Side-effect isolation: clicking commits picker must NOT touch
    // the resolver's settings.
    expect(vi.mocked(commands.setAiResolverCli)).not.toHaveBeenCalled();
    expect(vi.mocked(commands.setAiResolverModel)).not.toHaveBeenCalled();
  });

  it("clicking the resolver picker writes BOTH ai_resolver_cli and ai_resolver_model atomically", () => {
    // Key wiring guarantee: under the old "CLI Select + freeform model
    // Input" pair, the two could drift out of sync (user picks opencode
    // CLI but leaves model as "claude-sonnet-4"). The unified picker
    // must always update both setters together so they stay consistent.
    openGitSection();
    fireEvent.click(getPickers()[1]!);

    expect(vi.mocked(commands.setAiResolverCli)).toHaveBeenCalledWith("opencode");
    expect(vi.mocked(commands.setAiResolverModel)).toHaveBeenCalledWith(
      "anthropic/claude-3-5-sonnet",
    );
    // Side-effect isolation: clicking resolver picker must NOT touch
    // the commit-message settings.
    expect(vi.mocked(commands.setAiCommitMessageCli)).not.toHaveBeenCalled();
    expect(vi.mocked(commands.setAiCommitMessageModel)).not.toHaveBeenCalled();
  });

  it("does not render the legacy freeform 'Model override' Input or the standalone CLI Select", () => {
    // Regression guard: if a future refactor reintroduces the freeform
    // text input or the redundant CLI Select, this fails. Both UI
    // shapes were sources of the drift bug we just fixed.
    openGitSection();

    // The CLI tool Select had a rendered SelectItem text "Claude Code"
    // (the human label); the new picker doesn't render that string in
    // the trigger.
    expect(screen.queryByText("Claude Code")).not.toBeInTheDocument();
    // The freeform Input had placeholder="Default" — none should remain
    // on the Git section.
    expect(
      screen.queryByPlaceholderText("Default"),
    ).not.toBeInTheDocument();
  });
});

// ── Appearance → Agents section ──
//
// The "Agents" subsection now holds exactly one control: whether the agent
// orb's animation follows the current activity. The working-indicator tile
// picker and the indicator-color swatches were deleted outright when the
// orb replaced them — there is nothing left to pick.

describe("SettingsPanel — Appearance Agents section", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  function openAppearance() {
    render(<SettingsView />);
    const appearanceButtons = screen.getAllByRole("button", {
      name: /Appearance/i,
    });
    fireEvent.click(appearanceButtons[0]);
  }

  it("renders the Agents subsection with the match-activity toggle", () => {
    openAppearance();
    expect(screen.getAllByText("Agents").length).toBeGreaterThan(0);
    expect(
      screen.getAllByText("Match the orb to the activity").length,
    ).toBeGreaterThan(0);
  });

  it("no longer offers the retired working-indicator or color pickers", () => {
    openAppearance();
    expect(screen.queryByText("Working indicator")).toBeNull();
    expect(screen.queryByText("Indicator color")).toBeNull();
    expect(
      screen.queryByRole("radiogroup", { name: /Working indicator/i }),
    ).toBeNull();
    expect(
      screen.queryByRole("radiogroup", { name: /Indicator color/i }),
    ).toBeNull();
  });

  it("renders the Sidebar subsection and toggling Show git stats writes sidebar.show_git_stats", () => {
    openAppearance();
    expect(screen.getAllByText("Show git stats").length).toBeGreaterThan(0);
    const row = screen
      .getAllByText("Show git stats")[0]
      .closest("div")!.parentElement!;
    const toggle = within(row).getByRole("switch");
    fireEvent.click(toggle);
    expect(mockSettingsSet).toHaveBeenCalledWith(
      "sidebar.show_git_stats",
      "false",
    );
  });

  it("choosing an auto-settle window writes sidebar.auto_settle_days", () => {
    openAppearance();
    const [sevenDay] = screen.getAllByRole("radio", { name: /^7d$/i });
    fireEvent.click(sevenDay);
    expect(mockSettingsSet).toHaveBeenCalledWith(
      "sidebar.auto_settle_days",
      "7",
    );
  });

  it("toggling Match the orb to the activity writes agents.orb_match_activity", () => {
    openAppearance();
    const row = screen
      .getAllByText("Match the orb to the activity")[0]
      .closest("div")!.parentElement!;
    fireEvent.click(within(row).getByRole("switch"));
    expect(mockSettingsSet).toHaveBeenCalledWith(
      "agents.orb_match_activity",
      "false",
    );
  });

  it("replaces the theme grid with a single row plus its two doors", () => {
    openAppearance();
    // Picking happens in the command palette now.
    expect(screen.getAllByRole("button", { name: /Change/ }).length).toBeGreaterThan(0);
    expect(screen.getAllByRole("button", { name: "Customize" }).length).toBeGreaterThan(0);
    expect(screen.queryByText("Create or import")).toBeNull();
  });

  it("keeps every control that was on the page before the theme row landed", () => {
    openAppearance();
    for (const label of [
      "Typography",
      "Border radius",
      "Resource monitor",
      "Density",
      "Wrap code in chat",
      "Show git stats",
      "Auto-settle idle work",
      "Match the orb to the activity",
    ]) {
      expect(screen.getAllByText(label).length).toBeGreaterThan(0);
    }
  });
});
