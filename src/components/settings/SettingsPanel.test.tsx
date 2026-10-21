import { describe, it, expect, vi, beforeEach, beforeAll, afterEach } from "vitest";
import { act, render, screen, fireEvent, within, cleanup, waitFor } from "@testing-library/react";
afterEach(cleanup);

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
const mockSetSidebarWidth = vi.fn();
let requestedSection: string | null = null;
let navigationVersion = 0;
const mockSignOut = vi.fn();
const mockSettingsSet = vi.fn();
const mockToastError = vi.fn();
let mockWorkspaces: Array<Record<string, unknown>> = [
  { workspace_id: "ws-1", project_root: "/tmp/proj" },
];
let mockActiveWorkspaceId: string | null = "ws-1";

vi.mock("@/lib/toast", () => ({
  toast: { error: (...args: unknown[]) => mockToastError(...args) },
}));

vi.mock("@/components/settings/custom-acp-settings", () => ({
  CustomAcpSettings: () => null,
}));
const mockUpdateSyncedSetting = vi.fn().mockResolvedValue(undefined);
let mockSignedIn = true;

vi.mock("@/stores/ui-store", () => ({
  useUIStore: (sel: (s: Record<string, unknown>) => unknown) =>
    sel({
      setShowSettings: mockSetShowSettings,
      sidebarWidth: 288,
      setSidebarWidth: mockSetSidebarWidth,
      settingsSection: requestedSection,
      settingsNavigationVersion: navigationVersion,
    }),
}));

vi.mock("@/stores/app-store", async (importOriginal) => ({
  ...await importOriginal<typeof import("@/stores/app-store")>(),
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
        active_workspace_id: mockActiveWorkspaceId,
        workspaces: mockWorkspaces,
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
    "sidebar.show_git_stats": "true",
    "sidebar.auto_settle_days": "3",
    "agents.orb_match_activity": "true",
    "agents.auto_resume_usage_limit": "true",
    "agents.auto_resume_after_restart": "true",
    "agents.cross_provider_delegation": "true",
    "chat.code_wrap": "false",
    "chat.default_checkout": "worktree",
    "diff.layout": "unified",
    "diff.hide_whitespace": "false",
    "diff.file_state": "expanded",
    "appearance.time_format": "system",
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
    selectSidebarShowGitStats: () => true,
    selectChatCodeWrap: () => false,
    selectReduceMotion: () => false,
    selectSidebarAutoSettleDays: () => 3,
    selectOrbMatchActivity: () => true,
    selectAutoResumeUsageLimit: () => true,
    selectAutoResumeAfterRestart: () => true,
    selectCrossProviderDelegation: () => false,
    selectNewChatCheckout: () => "worktree",
    selectDiffLayoutPreference: () => "unified",
    selectHideWhitespacePreference: () => false,
    selectDiffFileState: () => "expanded",
    selectTimeFormat: () => "system",
    getNewChatCheckout: () => "worktree",
    getTimeFormat: () => "system",
  };
});

vi.mock("@/stores/auth-store", () => ({
  useAuthStore: (sel: (s: Record<string, unknown>) => unknown) =>
    sel({
      user: mockSignedIn ? { id: "u1", email: "test@codemux.org", name: "Test User", image: null } : null,
      isAuthenticated: mockSignedIn,
      signOut: mockSignOut,
    }),
}));

// The synced sound flag differs from `config.notification_sound_enabled`
// above on purpose: the switches must follow the synced values.
const syncedNotifications = { sound_enabled: false, desktop_enabled: true };

vi.mock("@/stores/synced-settings-store", () => ({
  useSyncedSettingsStore: (sel: (s: Record<string, unknown>) => unknown) =>
    sel({
      settings: {
        appearance: { theme: "system", shell_font: null, terminal_font_size: 13 },
        editor: { default_ide: null },
        terminal: { scrollback_limit: 10000, cursor_style: "bar" },
        git: { default_base_branch: "main" },
        keyboard: { shortcuts: {} },
        notifications: syncedNotifications,
        session_restore: { enabled: true, scrollback_lines: 10000, max_total_mb: 100 },
        agent_chat: { checkpoints_enabled: false, background_browser_desktop_viewport: true },
      },
      updateSetting: mockUpdateSyncedSetting,
      isSyncing: false,
      syncIssue: null,
    }),
  selectTerminalCursorStyle: () => "bar",
  selectDefaultEditor: () => "",
  selectDefaultBaseBranch: () => "main",
  selectBrowserDefaultViewport: () => null,
  selectNotificationSoundEnabled: () => true,
  selectDesktopNotificationsEnabled: () => true,
  selectKeyboardShortcuts: () => ({}),
  // Source Control shares the Git page.
  selectOpenPrLinksInBrowser: () => false,
}));

vi.mock("@/tauri/commands", () => ({
  detectEditors: vi.fn().mockResolvedValue([]),
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
  hasCodemuxinclude: vi.fn().mockResolvedValue(false),
  getBrowserDataSize: vi.fn().mockResolvedValue(0),
  getPackageFormat: vi.fn().mockResolvedValue("appimage"),
  getPresets: vi.fn().mockResolvedValue({ presets: [], bar_visible: true, default_preset_id: null }),
  setPresetPinned: vi.fn().mockResolvedValue(undefined),
  setPresetBarVisible: vi.fn().mockResolvedValue(undefined),
  deletePreset: vi.fn().mockResolvedValue(undefined),
  updatePreset: vi.fn().mockResolvedValue(undefined),
  clearBrowserCookies: vi.fn().mockResolvedValue(undefined),
  clearAllBrowserData: vi.fn().mockResolvedValue(undefined),
  listLaunchGeminiModels: vi.fn().mockResolvedValue([]),
  listChatProviderCapabilities: vi.fn().mockResolvedValue({
    provider: "claude", models: [], effort_label_map: {},
  }),
  agentChatProviderHealth: vi.fn().mockResolvedValue({ status: "ready" }),
  // Usage section — needed so switching to it renders rather than
  // throwing on an undefined command wrapper.
  usageSummary: vi.fn().mockResolvedValue(null),
  usageExportCsv: vi.fn().mockResolvedValue(""),
  agentConnectorStatus: vi.fn().mockResolvedValue({ enabled: true, publicOrigin: null, listenerRunning: false, mcpUrl: null, localCommand: "codemux mcp", pending: [], clients: [] }),
  agentConnectorRevoke: vi.fn().mockResolvedValue(undefined),
  listMcpServers: vi.fn().mockResolvedValue([]),
  getMcpRuntimeStatus: vi.fn().mockResolvedValue([]),
  setMcpDisabledIds: vi.fn().mockResolvedValue(undefined),
  primeMcpRuntime: vi.fn().mockResolvedValue([]),
  usageRefreshQuota: vi.fn().mockResolvedValue({ quota: {}, statuses: [], refreshed_at_ms: 0 }),
  usagePriceOverrides: vi.fn().mockResolvedValue({}),
  // Updates section: no devices, so only this computer is checked.
  hostsList: vi.fn().mockResolvedValue([]),
  getLocalDeviceName: vi.fn().mockResolvedValue("this-computer"),
  // Sections that share a page with the ones above.
  discoverSourceControl: vi.fn().mockResolvedValue([]),
  listToolPermissions: vi.fn().mockResolvedValue([]),
  removeToolPermission: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));

vi.mock("@/tauri/events", () => ({
  onPresetsChanged: vi.fn().mockReturnValue(Promise.resolve(() => {})),
  onAgentConnectorChanged: vi.fn().mockResolvedValue(() => {}),
  onCustomAcpChanged: vi.fn().mockResolvedValue(() => {}),
  onCustomAcpCatalogChanged: vi.fn().mockResolvedValue(() => {}),
}));
vi.mock("./custom-acp-settings", () => ({ CustomAcpDetail: () => null }));

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
import { revealSettingsAnchor } from "./settings-search";
import * as commands from "@/tauri/commands";
import { SETTINGS_SEARCH_ENTRIES, searchSettings } from "@/lib/settings-search";

describe("SettingsPanel", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("lists one nav row per page, without group captions", () => {
    const view = render(<SettingsView />);
    const nav = view.container.querySelector("nav") as HTMLElement;
    const rows = within(nav).getAllByRole("button").map((row) => row.textContent);
    expect(rows).toEqual([
      "General", "Appearance", "Agents", "Providers", "Usage", "Extensions",
      "Workspaces", "Tools", "Git", "Connections", "System",
    ]);
    for (const caption of ["PERSONAL", "AGENTS", "WORKSPACES", "TOOLS", "SYSTEM"]) {
      expect(within(nav).queryByText(caption)).toBeNull();
    }
    view.unmount();
  });

  it("stacks a page's sections under the page title, each listed beside it", () => {
    const view = render(<SettingsView />);
    fireEvent.click(within(view.container.querySelector("nav")!).getByRole("button", { name: "Agents" }));
    expect(view.getByRole("heading", { level: 2, name: "Agents" })).toBeInTheDocument();
    const sections = Array.from(
      view.container.querySelectorAll<HTMLElement>("[data-settings-section]"),
      (el) => el.dataset.settingsSection,
    );
    expect(sections).toEqual(["interface", "agent", "permissions", "workflows"]);
    for (const name of ["Chat interface", "Models & behavior", "Permissions", "Workflows"]) {
      expect(view.getByRole("heading", { level: 3, name })).toBeInTheDocument();
    }
    const onThisPage = view.getByRole("navigation", { name: "On this page" });
    expect(within(onThisPage).getAllByRole("button").map((b) => b.textContent)).toEqual([
      "Chat interface", "Models & behavior", "Permissions", "Workflows",
    ]);
    expect(within(onThisPage).getByRole("button", { name: "Chat interface" }))
      .toHaveAttribute("aria-current", "location");
    view.unmount();
  });

  it("resizes the nav with the app sidebar's shared width", () => {
    const view = render(<SettingsView />);
    const seam = view.getByRole("separator", { name: "Resize settings navigation" });
    expect(seam).toHaveAttribute("aria-valuenow", "288");
    fireEvent.keyDown(seam, { key: "ArrowRight" });
    expect(mockSetSidebarWidth).toHaveBeenCalledWith(304);
    fireEvent.keyDown(seam, { key: "Home" });
    expect(mockSetSidebarWidth).toHaveBeenCalledWith(180);
    view.unmount();
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

    const toolsButtons = screen.getAllByRole("button", { name: /Tools/i });
    fireEvent.click(toolsButtons[0]);
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
    const toolsButtons = within(container).getAllByRole("button", {
      name: /^Tools$/i,
    });
    fireEvent.click(toolsButtons[0]);
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

describe("SettingsPanel — Git section diff defaults", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  function openGitSection() {
    render(<SettingsView />);
    fireEvent.click(screen.getAllByRole("button", { name: /Git/i })[0]);
  }

  it("writes the diff layout and file state defaults", () => {
    openGitSection();
    fireEvent.click(screen.getAllByRole("radio", { name: /^Split$/i })[0]);
    expect(mockSettingsSet).toHaveBeenCalledWith("diff.layout", "split");
    fireEvent.click(screen.getAllByRole("radio", { name: /^Collapsed$/i })[0]);
    expect(mockSettingsSet).toHaveBeenCalledWith("diff.file_state", "collapsed");
  });

  it("toggling Hide whitespace changes writes diff.hide_whitespace", () => {
    openGitSection();
    fireEvent.click(screen.getByRole("switch", { name: "Hide whitespace changes" }));
    expect(mockSettingsSet).toHaveBeenCalledWith("diff.hide_whitespace", "true");
  });
});

describe("SettingsPanel — Notifications section", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  function openNotifications() {
    render(<SettingsView />);
    fireEvent.click(screen.getAllByRole("button", { name: /Notifications/i })[0]);
  }

  it("shows the synced sound and desktop preferences", () => {
    openNotifications();
    expect(
      screen.getAllByRole("switch", { name: "Notification sounds" })[0],
    ).toHaveAttribute("aria-checked", "false");
    expect(
      screen.getAllByRole("switch", { name: "Desktop notifications" })[0],
    ).toHaveAttribute("aria-checked", "true");
  });

  it("writes each toggle to the synced notification settings", () => {
    openNotifications();
    fireEvent.click(screen.getAllByRole("switch", { name: "Notification sounds" })[0]);
    expect(mockUpdateSyncedSetting).toHaveBeenCalledWith(
      "notifications",
      "sound_enabled",
      true,
    );
    fireEvent.click(screen.getAllByRole("switch", { name: "Desktop notifications" })[0]);
    expect(mockUpdateSyncedSetting).toHaveBeenCalledWith(
      "notifications",
      "desktop_enabled",
      false,
    );
  });
});

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
    // The row's label names its switch.
    const [toggle] = screen.getAllByRole("switch", { name: "Show git stats" });
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

  it("choosing a time format writes appearance.time_format", () => {
    openAppearance();
    const [twentyFour] = screen.getAllByRole("radio", { name: /^24-hour$/i });
    fireEvent.click(twentyFour);
    expect(mockSettingsSet).toHaveBeenCalledWith("appearance.time_format", "24h");
  });

  it("toggling Match the orb to the activity writes agents.orb_match_activity", () => {
    openAppearance();
    fireEvent.click(
      screen.getAllByRole("switch", { name: "Match the orb to the activity" })[0],
    );
    expect(mockSettingsSet).toHaveBeenCalledWith(
      "agents.orb_match_activity",
      "false",
    );
  });

  it("toggling Reduce motion writes appearance.reduce_motion", () => {
    openAppearance();
    fireEvent.click(screen.getAllByRole("switch", { name: "Reduce motion" })[0]);
    expect(mockSettingsSet).toHaveBeenCalledWith(
      "appearance.reduce_motion",
      "true",
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

  it("drops the read-only Border radius row", () => {
    openAppearance();
    expect(screen.queryByText("Border radius")).toBeNull();
  });
});


describe("Settings footer navigation", () => {
  beforeEach(() => { cleanup(); requestedSection = null; navigationVersion = 0; });

  it("honors new and repeated requests while already mounted", () => {
    const view = render(<SettingsView />);
    requestedSection = "appearance";
    navigationVersion++;
    view.rerender(<SettingsView />);
    expect(view.getByRole("button", { name: "Appearance" })).toHaveAttribute("aria-current", "page");
    fireEvent.click(view.getByRole("button", { name: "General" }));
    navigationVersion++;
    view.rerender(<SettingsView />);
    expect(view.getByRole("button", { name: "Appearance" })).toHaveAttribute("aria-current", "page");
    view.unmount();
  });

  it("shows an explicit unavailable page instead of an unrelated destination", () => {
    requestedSection = "removed-section";
    const view = render(<SettingsView />);
    expect(view.getByRole("status")).toHaveTextContent("Settings section unavailable");
    expect(view.queryByRole("button", { name: "Pin to footer" })).toBeNull();
    view.unmount();
  });

  it("keeps inbound administration on the shared MCP route in classic CLI mode without mounting outbound runtime", async () => {
    const { useFeatureFlags } = await import("@/stores/feature-flags");
    useFeatureFlags.setState({ enableAgentChat: false });
    vi.clearAllMocks();
    const status = { enabled: true, publicOrigin: null, listenerRunning: false, mcpUrl: null,
      localCommand: "codemux mcp", pending: [], clients: [{ id: "synthetic-granted", clientName: "Synthetic read-only assistant", callbackOrigin: "https://client.example",
        access: "read_only" as const, createdAt: new Date().toISOString(), expiresAt: new Date(Date.now() + 3_600_000).toISOString() }] };
    vi.mocked(commands.agentConnectorStatus).mockResolvedValue(status);
    requestedSection = "mcp";
    const view = render(<SettingsView />);
    try {
      expect(view.getByRole("button", { name: "Extensions" })).toHaveAttribute("aria-current", "page");
      fireEvent.click(await view.findByRole("button", { name: "Revoke" }));
      vi.mocked(commands.agentConnectorStatus).mockResolvedValue({ ...status, clients: [] });
      fireEvent.click(view.getByRole("button", { name: "Revoke access" }));
      await waitFor(() => expect(commands.agentConnectorRevoke).toHaveBeenCalledWith("synthetic-granted"));
      await waitFor(() => expect(view.queryByText("Synthetic read-only assistant")).not.toBeInTheDocument());
      expect(commands.agentConnectorStatus).toHaveBeenCalledTimes(2);
      for (const fn of [commands.listMcpServers, commands.getMcpRuntimeStatus, commands.setMcpDisabledIds, commands.primeMcpRuntime]) expect(fn).not.toHaveBeenCalled();
      expect(view.queryByLabelText("Refresh MCP servers")).not.toBeInTheDocument();
      expect(useFeatureFlags.getState().enableAgentChat).toBe(false);
    } finally { view.unmount(); useFeatureFlags.setState({ enableAgentChat: true }); requestedSection = null; }
  });

  it("does not render a chat-only Settings section when the feature is disabled", async () => {
    const { useFeatureFlags } = await import("@/stores/feature-flags");
    useFeatureFlags.setState({ enableAgentChat: false });
    requestedSection = "skills";
    const view = render(<SettingsView />);
    expect(view.getByRole("status")).toHaveTextContent("Settings section unavailable");
    expect(view.queryByRole("button", { name: "Skills" })).toBeNull();
    view.unmount();
    useFeatureFlags.setState({ enableAgentChat: true });
  });

  it("renders the Hermes setup once on the Providers page, not on Agent", async () => {
    requestedSection = "agent";
    const agent = render(<SettingsView />);
    expect(agent.queryByRole("region", { name: "Hermes setup" })).toBeNull();
    agent.unmount();
    requestedSection = "providers";
    const view = render(<SettingsView />);
    fireEvent.click(view.getByRole("button", { name: /^Hermes/ }));
    expect(await view.findAllByRole("region", { name: "Hermes setup" })).toHaveLength(1);
    view.unmount();
  });

  it("opens each section at the top rather than the previous page's offset", () => {
    const view = render(<SettingsView />);
    const viewport = () =>
      view.container.querySelector('[data-slot="scroll-area-viewport"]') as HTMLElement;
    viewport().scrollTop = 600;
    fireEvent.click(view.getByRole("button", { name: "Tools" }));
    expect(viewport().scrollTop).toBe(0);
    view.unmount();
  });

  it("lets the nav column scroll on its own on short windows", () => {
    const view = render(<SettingsView />);
    const nav = view.container.querySelector("nav") as HTMLElement;
    const scroller = nav.querySelector(".overflow-y-auto");
    expect(scroller).toContainElement(within(nav).getByRole("button", { name: "General" }));
    expect(scroller).not.toContainElement(view.getByRole("combobox", { name: "Search settings" }));
    view.unmount();
  });

  it("saves the default base branch once on commit and refuses an empty one", () => {
    requestedSection = "git";
    const view = render(<SettingsView />);
    const input = view.getByRole("textbox", { name: "Default base branch" });
    fireEvent.change(input, { target: { value: "dev" } });
    fireEvent.change(input, { target: { value: "develop" } });
    expect(mockUpdateSyncedSetting).not.toHaveBeenCalled();
    fireEvent.keyDown(input, { key: "Enter" });
    fireEvent.blur(input);
    expect(mockUpdateSyncedSetting).toHaveBeenCalledTimes(1);
    expect(mockUpdateSyncedSetting).toHaveBeenCalledWith("git", "default_base_branch", "develop");

    mockUpdateSyncedSetting.mockClear();
    fireEvent.change(input, { target: { value: "   " } });
    expect(input).toHaveAttribute("aria-invalid", "true");
    fireEvent.blur(input);
    expect(mockUpdateSyncedSetting).not.toHaveBeenCalled();
    // The mocked store still holds "main", so the field snaps back to it.
    expect(input).toHaveValue("main");
    expect(input).not.toHaveAttribute("aria-invalid");
    view.unmount();
  });

  it("saves a preset name typed just before Escape closes the editor", async () => {
    vi.mocked(commands.getPresets).mockResolvedValueOnce({
      presets: [{
        id: "p1",
        name: "Git Pull",
        description: null,
        commands: ["git pull"],
        working_directory: null,
        launch_mode: "new_tab",
        icon: null,
        pinned: true,
        is_builtin: false,
        auto_run_on_workspace: false,
        auto_run_on_new_tab: false,
        kind: "cli",
        launch_config: null,
      }],
      bar_visible: true,
      default_preset_id: null,
    });
    requestedSection = "presets";
    const view = render(<SettingsView />);
    fireEvent.click(await view.findByText("Git Pull"));
    const name = await screen.findByPlaceholderText("e.g. Git Pull");
    fireEvent.change(name, { target: { value: "Git Pull --rebase" } });
    expect(commands.updatePreset).not.toHaveBeenCalled();
    navigationVersion += 1;
    view.rerender(<SettingsView />);
    expect(screen.getByPlaceholderText("e.g. Git Pull")).toHaveValue("Git Pull --rebase");
    fireEvent.keyDown(name, { key: "Escape" });
    expect(commands.updatePreset).toHaveBeenCalledWith(
      expect.objectContaining({ id: "p1", name: "Git Pull --rebase" }),
    );
    view.unmount();
  });

  it("preserves a new preset's draft fields when Settings is requested again", async () => {
    vi.mocked(commands.updatePreset).mockClear();
    requestedSection = "presets";
    const view = render(<SettingsView />);
    await view.findByText("Show preset bar");
    fireEvent.click(view.getByRole("button", { name: "New preset" }));
    const name = await screen.findByPlaceholderText("e.g. Git Pull");
    const description = screen.getByPlaceholderText("Optional description");
    const prompt = screen.getByPlaceholderText("e.g. pull the latest changes and resolve any conflicts");
    fireEvent.change(name, { target: { value: "Review my changes" } });
    fireEvent.change(description, { target: { value: "A draft I am still editing" } });
    fireEvent.change(prompt, { target: { value: "Review the diff and run the affected tests" } });
    const viewport = view.container.querySelector<HTMLElement>('[data-slot="scroll-area-viewport"]')!;
    viewport.scrollTop = 400;

    // Ctrl+, requests Settings without a section while it is already open.
    requestedSection = null;
    navigationVersion += 1;
    view.rerender(<SettingsView />);

    expect(screen.getByPlaceholderText("e.g. Git Pull")).toHaveValue("Review my changes");
    expect(screen.getByPlaceholderText("Optional description")).toHaveValue("A draft I am still editing");
    expect(screen.getByPlaceholderText("e.g. pull the latest changes and resolve any conflicts")).toHaveValue("Review the diff and run the affected tests");
    expect(view.container.querySelector('[data-slot="scroll-area-viewport"]')).toBe(viewport);
    expect(viewport.scrollTop).toBe(0);
    expect(commands.updatePreset).not.toHaveBeenCalled();
    view.unmount();
  });

  it("shows session-restore slider values beside the control", () => {
    requestedSection = "session_restore";
    const view = render(<SettingsView />);
    expect(view.getByText("10,000 lines")).toBeInTheDocument();
    expect(view.getByText("100 MB")).toBeInTheDocument();
    expect(view.queryByText(/agent sessions/)).toBeNull();
    view.unmount();
  });

  it("saves a scrollback slider once when the drag ends, not on every tick", () => {
    requestedSection = "session_restore";
    mockUpdateSyncedSetting.mockClear();
    const view = render(<SettingsView />);
    const [lines] = view.getAllByRole("slider");
    // jsdom has no layout: give the track a 360px width so pointer
    // positions map onto the 1,000–50,000 range.
    const track = lines.closest('[data-slot="slider"]') as HTMLElement;
    track.getBoundingClientRect = () =>
      ({ left: 0, right: 360, width: 360, top: 0, bottom: 10, height: 10, x: 0, y: 0, toJSON: () => ({}) }) as DOMRect;
    track.setPointerCapture = () => {};
    track.releasePointerCapture = () => {};
    track.hasPointerCapture = () => true;

    fireEvent.pointerDown(track, { clientX: 72, pointerId: 1, button: 0 });
    fireEvent.pointerMove(track, { clientX: 144, pointerId: 1 });
    fireEvent.pointerMove(track, { clientX: 180, pointerId: 1 });
    expect(view.getByText("26,000 lines")).toBeInTheDocument();
    expect(mockUpdateSyncedSetting).not.toHaveBeenCalled();

    fireEvent.pointerUp(track, { clientX: 180, pointerId: 1 });
    expect(mockUpdateSyncedSetting).toHaveBeenCalledTimes(1);
    expect(mockUpdateSyncedSetting).toHaveBeenCalledWith("session_restore", "scrollback_lines", 26000);
    view.unmount();
  });

  it("returns to the top when a deep link reopens the current section", () => {
    requestedSection = "terminal";
    const view = render(<SettingsView />);
    const viewport = () =>
      view.container.querySelector('[data-slot="scroll-area-viewport"]') as HTMLElement;
    viewport().scrollTop = 400;
    navigationVersion += 1;
    view.rerender(<SettingsView />);
    expect(viewport().scrollTop).toBe(0);
    view.unmount();
  });

  it("pins the current Settings page from its header", async () => {
    const { useFooterPinsStore } = await import("@/stores/footer-pins-store");
    useFooterPinsStore.getState().reset();
    requestedSection = "appearance";
    const view = render(<SettingsView />);
    fireEvent.click(view.getByRole("button", { name: "Pin to footer" }));
    expect(useFooterPinsStore.getState().pins.slice(-1)[0]?.id).toBe("codemux.settings.appearance");
    expect(view.getByRole("button", { name: "Unpin from footer" })).toBeInTheDocument();
    view.unmount();
    requestedSection = null;
  });
});

describe("Settings section tracking", () => {
  it("follows the reader again once they scroll by the scrollbar after a jump", async () => {
    const { useFooterPinsStore } = await import("@/stores/footer-pins-store");
    useFooterPinsStore.getState().reset();
    requestedSection = "terminal";
    const view = render(<SettingsView />);
    try {
      const onThisPage = view.getByRole("navigation", { name: "On this page" });
      const current = () => within(onThisPage).getByRole("button", { current: "location" });
      const scrollArea = view.container.querySelector<HTMLElement>('[data-slot="scroll-area"]')!;
      const viewport = scrollArea.querySelector<HTMLElement>('[data-slot="scroll-area-viewport"]')!;
      // The deep link holds Terminal while the scroll it caused settles.
      await waitFor(() => expect(current()).toHaveTextContent("Terminal"));
      fireEvent.scroll(viewport);
      await new Promise((resolve) => requestAnimationFrame(resolve));
      expect(current()).toHaveTextContent("Terminal");

      // A thumb drag starts on the scrollbar, outside the viewport. jsdom
      // has no layout, so the scroll then reads as the bottom: Browser.
      fireEvent.pointerDown(scrollArea);
      fireEvent.scroll(viewport);
      await waitFor(() => expect(current()).toHaveTextContent("Browser"));
      fireEvent.click(view.getByRole("button", { name: "Pin to footer" }));
      expect(useFooterPinsStore.getState().pins.slice(-1)[0]?.id).toBe("codemux.settings.browser");
    } finally {
      view.unmount();
      requestedSection = null;
    }
  });
});

describe("Settings destructive actions", () => {
  beforeEach(() => {
    cleanup();
    vi.clearAllMocks();
  });
  afterEach(() => {
    requestedSection = null;
  });

  const customPreset = {
    id: "p-1",
    name: "Review bot",
    description: null,
    commands: ["claude"],
    working_directory: null,
    launch_mode: "new_tab",
    icon: "claude",
    pinned: false,
    is_builtin: false,
    auto_run_on_workspace: false,
    auto_run_on_new_tab: false,
    kind: "cli",
    launch_config: null,
  };

  it("asks before deleting a preset from the list, and only deletes on confirm", async () => {
    vi.mocked(commands.getPresets).mockResolvedValueOnce({
      presets: [customPreset],
      bar_visible: true,
      default_preset_id: null,
    } as never);
    requestedSection = "presets";
    render(<SettingsView />);

    const remove = await screen.findByRole("button", { name: "Delete preset Review bot" });
    fireEvent.click(remove);
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog).toHaveTextContent('Delete "Review bot"?');
    expect(commands.deletePreset).not.toHaveBeenCalled();

    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(commands.deletePreset).not.toHaveBeenCalled();

    fireEvent.click(remove);
    fireEvent.click(
      within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Delete preset" }),
    );
    await waitFor(() => expect(commands.deletePreset).toHaveBeenCalledWith("p-1"));
  });

  it("opens a preset's editor from the keyboard", async () => {
    vi.mocked(commands.getPresets).mockResolvedValueOnce({
      presets: [customPreset],
      bar_visible: true,
      default_preset_id: null,
    } as never);
    requestedSection = "presets";
    render(<SettingsView />);

    const row = await screen.findByRole("button", { name: "Edit Review bot" });
    expect(row.tagName).toBe("BUTTON");
    fireEvent.click(row);
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
  });

  it("clears browser data through the app's dialog, not window.confirm", async () => {
    const nativeConfirm = vi.spyOn(window, "confirm");
    requestedSection = "browser";
    render(<SettingsView />);

    fireEvent.click(screen.getByRole("button", { name: "Clear all data" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog).toHaveTextContent("Clear all browser data?");
    fireEvent.click(within(dialog).getByRole("button", { name: "Clear all data" }));

    await waitFor(() => expect(commands.clearAllBrowserData).toHaveBeenCalled());
    expect(nativeConfirm).not.toHaveBeenCalled();
    expect(await screen.findByRole("status")).toHaveTextContent("All browser data cleared.");
    nativeConfirm.mockRestore();
  });

  it("shows a failed clear as an error, not a success", async () => {
    vi.mocked(commands.clearBrowserCookies).mockRejectedValueOnce("profile locked");
    requestedSection = "browser";
    render(<SettingsView />);

    fireEvent.click(screen.getByRole("button", { name: "Clear cookies" }));
    fireEvent.click(
      within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Clear cookies" }),
    );
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Couldn't clear cookies: profile locked");
    expect(alert.querySelector(".bg-destructive")).not.toBeNull();
  });
});

describe("Settings → Projects", () => {
  const chooseOtherProject = async () => {
    Element.prototype.hasPointerCapture ??= () => false;
    Element.prototype.scrollIntoView ??= () => {};
    fireEvent.keyDown(screen.getByRole("combobox", { name: "Project" }), { key: "Enter" });
    fireEvent.keyDown(await screen.findByRole("option", { name: "other" }), { key: "Enter" });
  };
  beforeEach(() => {
    cleanup();
    vi.clearAllMocks();
    requestedSection = "projects";
  });
  afterEach(() => {
    requestedSection = null;
    mockWorkspaces = [{ workspace_id: "ws-1", project_root: "/tmp/proj" }];
    mockActiveWorkspaceId = "ws-1";
  });

  it("offers to open a project instead of editable fields when there is none", () => {
    mockWorkspaces = [];
    mockActiveWorkspaceId = null;
    render(<SettingsView />);

    expect(screen.getByText("Open a project to configure its lifecycle scripts.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Open project/ })).toBeInTheDocument();
    expect(screen.queryByPlaceholderText("e.g. npm install")).toBeNull();
  });

  it("edits another project without switching workspaces", async () => {
    mockWorkspaces = [
      { workspace_id: "ws-1", project_root: "/tmp/proj" },
      { workspace_id: "ws-2", project_root: "/tmp/other" },
    ];
    render(<SettingsView />);

    // Defaults to the active workspace's project.
    await waitFor(() => expect(commands.getProjectScripts).toHaveBeenCalledWith("/tmp/proj"));
    const picker = screen.getByRole("combobox", { name: "Project" });
    expect(picker).toHaveTextContent("proj");

    // Radix Select reaches for pointer capture and scrolling jsdom lacks.
    Element.prototype.hasPointerCapture ??= () => false;
    Element.prototype.scrollIntoView ??= () => {};
    fireEvent.keyDown(picker, { key: "Enter" });
    fireEvent.keyDown(await screen.findByRole("option", { name: "other" }), { key: "Enter" });
    await waitFor(() => expect(commands.getProjectScripts).toHaveBeenCalledWith("/tmp/other"));
    expect(screen.getByRole("combobox", { name: "Project" })).toHaveTextContent("other");
  });

  it("saves on blur only when something changed, then says so", async () => {
    render(<SettingsView />);
    await waitFor(() => expect(commands.getProjectScripts).toHaveBeenCalled());
    const setup = screen.getByPlaceholderText("e.g. npm install");

    fireEvent.blur(setup);
    expect(commands.setProjectScripts).not.toHaveBeenCalled();

    fireEvent.change(setup, { target: { value: "npm ci" } });
    fireEvent.blur(setup);
    expect(commands.setProjectScripts).toHaveBeenCalledWith("/tmp/proj", {
      setup: ["npm ci"],
      teardown: [],
      run: null,
      worktree_includes: [],
    });
    // Every field carries a hidden note; only the one just saved shows.
    const visibleNotes = () =>
      screen.getAllByText("Saved").filter((el) => !el.closest("[aria-hidden='true']"));
    await waitFor(() => expect(visibleNotes()).toHaveLength(1));
    expect(visibleNotes()[0].closest(".space-y-2")).toContainElement(setup);
  });

  it("cannot save the previous project's values while the new project is loading", async () => {
    mockWorkspaces = [
      { workspace_id: "ws-1", project_root: "/tmp/proj" },
      { workspace_id: "ws-2", project_root: "/tmp/other" },
    ];
    let resolveOther!: (scripts: Awaited<ReturnType<typeof commands.getProjectScripts>>) => void;
    vi.mocked(commands.getProjectScripts)
      .mockResolvedValueOnce({ setup: ["setup-for-a"], teardown: [], run: null, worktree_includes: [] })
      .mockReturnValueOnce(new Promise((resolve) => { resolveOther = resolve; }));
    render(<SettingsView />);
    await waitFor(() => expect(screen.getByPlaceholderText("e.g. npm install")).toHaveValue("setup-for-a"));
    await chooseOtherProject();

    const setup = screen.getByPlaceholderText("e.g. npm install");
    expect(setup).toBeDisabled();
    expect(setup).toHaveValue("");
    const projects = document.querySelector<HTMLElement>('[data-settings-section="projects"]')!;
    expect(within(projects).getByRole("status")).toHaveTextContent("Loading project scripts");
    fireEvent.blur(setup);
    expect(commands.setProjectScripts).not.toHaveBeenCalled();

    resolveOther({ setup: ["setup-for-b"], teardown: [], run: null, worktree_includes: [] });
    await waitFor(() => expect(setup).toBeEnabled());
    expect(setup).toHaveValue("setup-for-b");
    fireEvent.blur(setup);
    expect(commands.setProjectScripts).not.toHaveBeenCalled();
  });

  it("keeps fields disabled and shows a project load error", async () => {
    vi.mocked(commands.getProjectScripts).mockRejectedValueOnce(new Error("permission denied"));
    render(<SettingsView />);
    expect(await screen.findByRole("alert")).toHaveTextContent("Couldn't load proj's scripts: Error: permission denied");
    const setup = screen.getByPlaceholderText("e.g. npm install");
    expect(setup).toBeDisabled();
    fireEvent.blur(setup);
    expect(commands.setProjectScripts).not.toHaveBeenCalled();
  });

  it("ignores a previous project's late save completion", async () => {
    mockWorkspaces = [
      { workspace_id: "ws-1", project_root: "/tmp/proj" },
      { workspace_id: "ws-2", project_root: "/tmp/other" },
    ];
    let resolveSave!: () => void;
    vi.mocked(commands.setProjectScripts).mockReturnValueOnce(new Promise((resolve) => { resolveSave = resolve; }));
    render(<SettingsView />);
    const setup = screen.getByPlaceholderText("e.g. npm install");
    await waitFor(() => expect(setup).toBeEnabled());
    fireEvent.change(setup, { target: { value: "setup-for-a" } });
    fireEvent.blur(setup);
    await chooseOtherProject();
    await waitFor(() => expect(setup).toBeEnabled());
    resolveSave();
    await waitFor(() => expect(screen.getAllByText("Saved").filter((el) => !el.closest("[aria-hidden='true']"))).toHaveLength(0));

    fireEvent.blur(setup);
    expect(commands.setProjectScripts).toHaveBeenCalledTimes(1);
  });

  it("reports a failed save and keeps the edit dirty", async () => {
    vi.mocked(commands.setProjectScripts).mockRejectedValueOnce("read-only");
    render(<SettingsView />);
    await waitFor(() => expect(commands.getProjectScripts).toHaveBeenCalled());
    const run = screen.getByPlaceholderText("e.g. npm run dev");

    fireEvent.change(run, { target: { value: "npm run dev" } });
    fireEvent.blur(run);
    await waitFor(() => expect(mockToastError).toHaveBeenCalled());
    expect(mockToastError.mock.calls[0][0]).toBe("Couldn't save proj's scripts");

    // Still dirty: the next blur tries again.
    fireEvent.blur(run);
    expect(commands.setProjectScripts).toHaveBeenCalledTimes(2);
  });
});

describe("Settings rows", () => {
  beforeEach(() => {
    cleanup();
    requestedSection = "appearance";
  });
  afterEach(() => {
    requestedSection = null;
  });

  it("marks settings that stay on this device", () => {
    render(<SettingsView />);
    const density = screen.getByRole("group", { name: "Density" });
    expect(density).toHaveAccessibleDescription(/Only on this device/);
    // Resource monitor syncs with the account, so it carries no mark.
    expect(
      screen.getByRole("switch", { name: "Resource monitor" }),
    ).not.toHaveAccessibleDescription(/Only on this device/);
  });

  it("marks the Agent page's machine-local settings", async () => {
    const { useFeatureFlags } = await import("@/stores/feature-flags");
    useFeatureFlags.setState({ enableAgentChat: true });
    requestedSection = "agent";
    render(<SettingsView />);
    for (const name of [
      "Auto-configure MCP for workspaces",
      "Resume automatically after usage limits reset",
      "Auto-resume agents after restart",
    ]) {
      expect(screen.getByRole("switch", { name })).toHaveAccessibleDescription(
        /Only on this device/,
      );
    }
    // Checkpoints sync with the account, so they carry no mark.
    expect(
      screen.getByRole("switch", { name: "Per-turn revert checkpoints" }),
    ).not.toHaveAccessibleDescription(/Only on this device/);
  });
});

// ── Settings search ──

describe("Settings search", () => {
  beforeEach(async () => {
    cleanup();
    requestedSection = null;
    navigationVersion = 0;
    mockSignedIn = true;
    const { useFeatureFlags } = await import("@/stores/feature-flags");
    useFeatureFlags.setState({ enableAgentChat: true });
  });

  const searchField = (view: ReturnType<typeof render>) =>
    view.getByRole("combobox", { name: "Search settings" });

  it("updates search results when Agent Chat or sign-in state changes", async () => {
    const { useFeatureFlags } = await import("@/stores/feature-flags");
    const view = render(<SettingsView />);
    const hasResult = (label: string) => view.queryAllByRole("option").some((option) => option.textContent?.includes(label));
    fireEvent.change(searchField(view), { target: { value: "checkpoint" } });
    expect(hasResult("Per-turn revert checkpoints")).toBe(true);

    act(() => useFeatureFlags.setState({ enableAgentChat: false }));
    view.rerender(<SettingsView />);
    expect(hasResult("Per-turn revert checkpoints")).toBe(false);
    fireEvent.change(searchField(view), { target: { value: "skills sync" } });
    expect(hasResult("Skills sync")).toBe(false);

    act(() => useFeatureFlags.setState({ enableAgentChat: true }));
    view.rerender(<SettingsView />);
    expect(hasResult("Skills sync")).toBe(true);
    mockSignedIn = false;
    view.rerender(<SettingsView />);
    expect(hasResult("Skills sync")).toBe(false);
    fireEvent.change(searchField(view), { target: { value: "email" } });
    expect(hasResult("Email")).toBe(false);
    view.unmount();
  });

  it("lists matching rows by page and opens the page on Enter", async () => {
    const view = render(<SettingsView />);
    fireEvent.change(searchField(view), { target: { value: "orb" } });

    const results = view.getByRole("listbox", { name: "Settings search results" });
    expect(within(results).getByRole("group", { name: "Appearance" })).toBeInTheDocument();
    expect(within(results).getByRole("option", { name: /Match the orb to the activity/ }))
      .toHaveAttribute("aria-selected", "true");
    // The nav is replaced while searching.
    expect(within(view.container.querySelector("nav")!).queryByRole("button", { name: "General" })).toBeNull();

    fireEvent.keyDown(searchField(view), { key: "Enter" });

    expect(view.getByRole("button", { name: "Appearance" })).toHaveAttribute("aria-current", "page");
    expect(searchField(view)).toHaveValue("");
    // The row search named is lit once the page has painted.
    await waitFor(() =>
      expect(
        view.getByText("Match the orb to the activity").closest("[data-settings-row]"),
      ).toHaveAttribute("data-settings-flash"),
    );
    view.unmount();
  });

  it("scrolls the nav to the page a result opened", () => {
    const view = render(<SettingsView />);
    // jsdom has no layout: the nav shows y 100–500 and the About row sits at
    // y 900–932, below the fold.
    const scroller = within(view.container.querySelector("nav")!)
      .getByRole("button", { name: "General" })
      .closest<HTMLElement>(".overflow-y-auto")!;
    let scrollTop = 0;
    Object.defineProperty(scroller, "scrollTop", {
      configurable: true,
      get: () => scrollTop,
      set: (value: number) => {
        scrollTop = value;
      },
    });
    scroller.getBoundingClientRect = () => new DOMRect(0, 100, 240, 400);
    const rects = vi
      .spyOn(HTMLElement.prototype, "getBoundingClientRect")
      .mockImplementation(function (this: HTMLElement) {
        return this.getAttribute("aria-current") === "page"
          ? new DOMRect(0, 900, 240, 32)
          : new DOMRect(0, 0, 0, 0);
      });

    fireEvent.change(searchField(view), { target: { value: "diagnostics" } });
    fireEvent.keyDown(searchField(view), { key: "Enter" });

    expect(view.getByRole("button", { name: "System" })).toHaveAttribute("aria-current", "page");
    // 932 − 500, plus the 12px margin.
    expect(scrollTop).toBe(444);
    rects.mockRestore();
    view.unmount();
  });

  it("moves through results with the arrow keys", () => {
    const view = render(<SettingsView />);
    fireEvent.change(searchField(view), { target: { value: "cursor" } });
    const options = () => view.getAllByRole("option");
    expect(options()[0]).toHaveAttribute("aria-selected", "true");
    fireEvent.keyDown(searchField(view), { key: "ArrowDown" });
    expect(options()[1]).toHaveAttribute("aria-selected", "true");
    fireEvent.keyDown(searchField(view), { key: "ArrowUp" });
    fireEvent.keyDown(searchField(view), { key: "ArrowUp" });
    expect(options()[options().length - 1]).toHaveAttribute("aria-selected", "true");
    view.unmount();
  });

  it("says so when nothing matches, and Escape clears the query", () => {
    const view = render(<SettingsView />);
    fireEvent.change(searchField(view), { target: { value: "zzzz" } });
    expect(view.getByRole("status")).toHaveTextContent("No settings match");
    fireEvent.keyDown(searchField(view), { key: "Escape" });
    expect(searchField(view)).toHaveValue("");
    expect(view.getByRole("button", { name: "Account" })).toBeInTheDocument();
    view.unmount();
  });

  it("focuses the field on Ctrl+F, and on / outside a text field", () => {
    const view = render(<SettingsView />);
    const field = searchField(view);

    fireEvent.keyDown(document.body, { key: "f", ctrlKey: true });
    expect(field).toHaveFocus();

    field.blur();
    fireEvent.keyDown(document.body, { key: "/" });
    expect(field).toHaveFocus();

    // "/" typed into another field is text, not a shortcut.
    const other = document.createElement("input");
    document.body.appendChild(other);
    other.focus();
    fireEvent.keyDown(other, { key: "/" });
    expect(other).toHaveFocus();
    other.remove();
    view.unmount();
  });

  it.each([
    { agentChat: true, signedIn: true },
    { agentChat: false, signedIn: true },
    { agentChat: true, signedIn: false },
    { agentChat: false, signedIn: false },
  ])("finds every searchable row on its rendered page (Agent Chat: $agentChat, signed in: $signedIn)", async ({ agentChat, signedIn }) => {
    // The index stands in for pages that are not rendered, so a renamed row
    // must be renamed here too or search lands on the top of the page. The
    // check uses the same lookup navigation does, not just any text match.
    const { useFeatureFlags } = await import("@/stores/feature-flags");
    useFeatureFlags.setState({ enableAgentChat: agentChat });
    mockSignedIn = signedIn;
    const view = render(<SettingsView />);
    const searchable = SETTINGS_SEARCH_ENTRIES.flatMap((entry) =>
      searchSettings(entry.label, agentChat, signedIn).flatMap((group) => group.results)
        .filter((result) => result.section === entry.section && result.anchor === entry.label),
    );
    const sections = new Set(searchable.map((result) => result.section));
    for (const section of sections) {
      requestedSection = section;
      navigationVersion++;
      view.rerender(<SettingsView />);
      for (const entry of searchable.filter((result) => result.section === section)) {
        // Some pages fill in after an async load (Presets).
        await waitFor(() =>
          expect(
            revealSettingsAnchor(view.container, entry.label),
            `${section} → ${entry.label}`,
          ).toBe(true),
        );
      }
    }
    view.unmount();
  });
});

describe("agent collaboration setting", () => {
  it("describes chat tools and inherited permissions", async () => {
    const { useFeatureFlags } = await import("@/stores/feature-flags");
    useFeatureFlags.setState({ enableAgentChat: true });
    requestedSection = "agent";
    render(<SettingsView />);
    expect(await screen.findByText("Let agents work with other chats")).toBeInTheDocument();
    expect(screen.getByText(/within the current chat’s permissions/)).toBeInTheDocument();
    expect(screen.queryByText("Delegate to other agents")).not.toBeInTheDocument();
    requestedSection = null;
  });
});
