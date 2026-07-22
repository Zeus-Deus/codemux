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
    "sidebar.show_git_stats": "true",
    "sidebar.working_indicator": "braille",
    "sidebar.working_indicator_color": "status-working",
  };
  return {
    useSettingsStore: (sel: (s: Record<string, unknown>) => unknown) =>
      sel({
        set: mockSettingsSet,
        get: (key: string) => defaults[key] ?? "",
        settings: defaults,
      }),
    selectTerminalColorTheme: () => "app",
    selectPalette: () => "cool",
    selectDensity: () => "comfortable",
    selectSidebarShowGitStats: () => true,
    selectWorkingIndicator: () => "braille",
    selectWorkingIndicatorColor: () => "status-working",
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
}));

vi.mock("@/tauri/events", () => ({
  onPresetsChanged: vi.fn().mockReturnValue(Promise.resolve(() => {})),
}));

// ── Model picker mock ──
//
// Both settings rows now use `MultiProviderModelPicker`; the commits
// row passes `allowedProviders=["claude"]` to keep that backend's
// claude-only constraint, while the resolver row leaves it open. We
// mock the picker as a thin shell that:
//   - surfaces `allowedProviders` as a data attribute so tests can
//     distinguish the two instances on the page
//   - fires a sensible change tuple on click. When restricted to a
//     single provider, the mock fires THAT provider; otherwise it
//     simulates a cross-provider switch (claude → opencode) so the
//     resolver test can prove both setters are invoked atomically.
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
    const restricted =
      Array.isArray(allowedProviders) && allowedProviders.length > 0;
    const allowedKey = restricted ? allowedProviders!.join(",") : "all";
    return (
      <button
        type="button"
        data-testid="multi-provider-model-picker"
        data-provider={provider}
        data-model={model ?? ""}
        data-allowed={allowedKey}
        onClick={() => {
          if (restricted) {
            // Pinned-provider case (commits): pick a model in the same
            // provider so the wiring stays type-correct. The first
            // allowed provider wins.
            onProviderModelChange(allowedProviders![0]!, "claude-test-model");
          } else {
            // Multi-provider case (resolver): simulate flipping to a
            // different provider so the test can verify BOTH the cli
            // setter and the model setter fire atomically.
            onProviderModelChange("opencode", "anthropic/claude-3-5-sonnet");
          }
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

  /** Both pickers in the Git section now use the same shared
   *  `MultiProviderModelPicker` with no `allowedProviders` allowlist —
   *  the commits row went open after the backend was generalised to
   *  accept any CLI (claude / codex / opencode) via `build_resolver_argv`.
   *  Distinguish them by document order: the FIRST live picker is the
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
    // store, identical visual style) and now both show all providers.
    // The commits row going open is what closes the visual-consistency
    // bug the user reported in the screenshots.
    openGitSection();

    const pickers = getPickers();
    // Mock app-store seeds both rows with provider="claude", model=null.
    expect(pickers[0]!.getAttribute("data-provider")).toBe("claude");
    expect(pickers[1]!.getAttribute("data-provider")).toBe("claude");
    // Neither picker passes `allowedProviders` anymore — both surfaces
    // accept all three drivers now. (The chat composer still passes no
    // allowlist either, so all three pickers in the app are
    // interchangeable wiring-wise.)
    expect(pickers[0]!.getAttribute("data-allowed")).toBe("all");
    expect(pickers[1]!.getAttribute("data-allowed")).toBe("all");
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
// The new "Agents" subsection exposes the three sidebar-live settings:
// grouping mode (segmented control), working-indicator variant (tile
// picker), and indicator color (swatches). Each writes to the machine-local
// settings store via the shared `set`.

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

  it("renders the Agents subsection with both indicator controls", () => {
    openAppearance();
    expect(screen.getAllByText("Agents").length).toBeGreaterThan(0);
    expect(screen.getAllByText("Working indicator").length).toBeGreaterThan(0);
    expect(screen.getAllByText("Indicator color").length).toBeGreaterThan(0);
    // Tile picker + swatch groups are present.
    expect(
      screen.getAllByRole("radiogroup", { name: /Working indicator/i }).length,
    ).toBeGreaterThan(0);
    expect(
      screen.getAllByRole("radiogroup", { name: /Indicator color/i }).length,
    ).toBeGreaterThan(0);
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

  it("choosing a working-indicator tile writes sidebar.working_indicator", () => {
    openAppearance();
    const [sweep] = screen.getAllByRole("radio", { name: /^Sweep$/i });
    fireEvent.click(sweep);
    expect(mockSettingsSet).toHaveBeenCalledWith(
      "sidebar.working_indicator",
      "sweep",
    );
  });

  it("choosing a color swatch writes sidebar.working_indicator_color", () => {
    openAppearance();
    const [violet] = screen.getAllByRole("radio", { name: /^Violet$/i });
    fireEvent.click(violet);
    expect(mockSettingsSet).toHaveBeenCalledWith(
      "sidebar.working_indicator_color",
      "accent-violet",
    );
  });
});
