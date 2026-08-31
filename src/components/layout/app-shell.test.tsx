/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, waitFor } from "@testing-library/react";

// ── Mutable mock state ──
let hasWorkspacesFlag = false;
let enableLazyFlag = false;
let activeDraftId: string | null = null;
let appStateReady = true;
let settingsLoaded = true;
let syncedLoading = false;
let showSettingsFlag = false;
let showAutomationsFlag = false;
let showDevicesFlag = false;
let showNewProjectScreenFlag = false;
let commandPaletteOpenFlag = false;
// Appearance state: the machine-local legacy palette key, the synced theme id,
// and the two writes the shell can make in response to them.
let localSettings: Record<string, string> = {};
let syncedThemeId = "default";
const {
  applyThemeSpy,
  markStartupSpy,
  schedulePrefetchSpy,
  cancelPrefetchSpy,
} = vi.hoisted(() => ({
  applyThemeSpy: vi.fn(),
  markStartupSpy: vi.fn((..._args: unknown[]) => {}),
  schedulePrefetchSpy: vi.fn((..._args: unknown[]) => {}),
  cancelPrefetchSpy: vi.fn(),
}));
const setLocalSetting = vi.fn((key: string, value: string) => {
  localSettings = { ...localSettings, [key]: value };
});
const updateSyncedSetting = vi.fn(
  async (_section: string, key: string, value: unknown) => {
    // Mirrors the real store's optimistic local write.
    if (key === "theme") syncedThemeId = String(value);
  },
);

// The real AppShell imports a lot. We mock the heavy children down to
// sentinel text nodes so we can assert which branch of the shell
// rendered.
vi.mock("./title-bar", () => ({
  TitleBar: () => <div data-testid="title-bar" />,
}));
vi.mock("./app-sidebar", () => ({
  AppSidebar: () => <div data-testid="app-sidebar" />,
}));
vi.mock("./workspace-main", () => ({
  WorkspaceMain: () => <div data-testid="workspace-main" />,
}));
// The peek overlay has its own dedicated test file
// (BrowserPeekOverlay.test.tsx); here it's just a shell child.
vi.mock("@/components/browser/BrowserPeekOverlay", () => ({
  BrowserPeekOverlay: () => <div data-testid="browser-peek-overlay" />,
}));
vi.mock("./empty-state", () => ({
  EmptyState: () => <div data-testid="empty-state" />,
}));
vi.mock("@/components/settings/settings-view", () => ({
  SettingsView: () => <div data-testid="settings-view" />,
}));
vi.mock("@/components/automations/automations-view", () => ({
  AutomationsView: () => <div data-testid="automations-view" />,
}));
vi.mock("@/components/devices/devices-view", () => ({
  DevicesView: () => <div data-testid="devices-view" />,
}));
vi.mock("@/components/overlays/command-palette", () => ({
  CommandPalette: () => null,
}));
vi.mock("@/components/overlays/new-project-screen", () => ({
  NewProjectScreen: () => <div data-testid="new-project-screen" />,
}));
vi.mock("@/components/search/file-search-dialog", () => ({
  FileSearchDialog: () => null,
}));
vi.mock("@/components/search/content-search-dialog", () => ({
  ContentSearchDialog: () => null,
}));
vi.mock("@/hooks/use-worktree-include-toast", () => ({
  useWorktreeIncludeToast: () => {},
}));
vi.mock("@/components/ui/sidebar", () => ({
  SidebarProvider: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
  SidebarInset: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
  // Consumed by `SidebarToggleBridge`, which publishes the real
  // `toggleSidebar` (mobile Sheet included) to the UI store for the Ctrl+B
  // keybind and the command palette.
  useSidebar: () => ({ toggleSidebar: vi.fn() }),
}));

vi.mock("@/stores/app-store", () => ({
  useAppStore: vi.fn((selector: (s: unknown) => unknown) =>
    selector({
      appState: appStateReady
        ? {
            workspaces: hasWorkspacesFlag
              ? [{ workspace_id: "ws-1" }]
              : [],
          }
        : null,
    }),
  ),
}));

vi.mock("@/stores/chat-draft-store", () => ({
  useChatDraftStore: vi.fn((selector: (s: unknown) => unknown) =>
    selector({ activeDraftId }),
  ),
}));

vi.mock("@/stores/feature-flags", () => ({
  useFeatureFlags: vi.fn((selector: (s: unknown) => unknown) =>
    selector({
      enableAgentChat: false,
      enableLazyWorkspaceCreation: enableLazyFlag,
      loaded: true,
    }),
  ),
}));

vi.mock("@/stores/ui-store", () => ({
  useUIStore: Object.assign(
    vi.fn((selector: (s: unknown) => unknown) =>
      selector({
        showSettings: showSettingsFlag,
        showAutomations: showAutomationsFlag,
        showDevices: showDevicesFlag,
        showNewProjectScreen: showNewProjectScreenFlag,
        showCommandPalette: commandPaletteOpenFlag,
        setShowCommandPalette: vi.fn(),
        setSidebarToggleFn: vi.fn(),
      }),
    ),
    {
      getState: () => ({
        setSidebarToggleFn: vi.fn(),
      }),
    },
  ),
}));

vi.mock("@/stores/settings-store", () => ({
  useSettingsStore: Object.assign(
    vi.fn((selector: (s: unknown) => unknown) =>
      selector({
        loaded: settingsLoaded,
        settings: localSettings,
      }),
    ),
    { getState: () => ({ load: vi.fn(), set: setLocalSetting }) },
  ),
  // Appearance selectors read by AppShell to apply palette/density attrs.
  selectPalette: () => "cool",
  selectDensity: () => "comfortable",
}));

const EMPTY_CUSTOM_THEMES: unknown[] = [];

vi.mock("@/stores/synced-settings-store", () => ({
  useSyncedSettingsStore: vi.fn((selector: (s: unknown) => unknown) =>
    selector({
      isLoading: syncedLoading,
      settings: {
        appearance: { theme: syncedThemeId, custom_themes: EMPTY_CUSTOM_THEMES },
      },
      updateSetting: updateSyncedSetting,
    }),
  ),
}));

// Only `applyTheme` is stubbed; the shell's theme resolution stays real so the
// spy sees the theme the user would actually get.
vi.mock("@/lib/themes", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/themes")>()),
  applyTheme: applyThemeSpy,
}));

vi.mock("@/lib/perf/interaction-trace", () => ({
  markStartup: (...args: unknown[]) => markStartupSpy(...args),
}));

vi.mock("@/lib/idle-prefetch", () => ({
  scheduleSequentialIdlePrefetch: (...args: unknown[]) => {
    schedulePrefetchSpy(...args);
    return cancelPrefetchSpy;
  },
}));

import { AppShell } from "./app-shell";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

function resetMockState() {
  hasWorkspacesFlag = false;
  enableLazyFlag = false;
  activeDraftId = null;
  appStateReady = true;
  settingsLoaded = true;
  syncedLoading = false;
  showSettingsFlag = false;
  showAutomationsFlag = false;
  showDevicesFlag = false;
  showNewProjectScreenFlag = false;
  commandPaletteOpenFlag = false;
  localSettings = {};
  syncedThemeId = "default";
  applyThemeSpy.mockClear();
  markStartupSpy.mockClear();
  schedulePrefetchSpy.mockClear();
  cancelPrefetchSpy.mockClear();
  setLocalSetting.mockClear();
  updateSyncedSetting.mockClear();
}

describe("AppShell rendering gates", () => {
  beforeEach(resetMockState);

  it("renders EmptyState when there are no workspaces and no active draft", () => {
    hasWorkspacesFlag = false;
    enableLazyFlag = false;
    activeDraftId = null;
    const { getByTestId, queryByTestId } = render(<AppShell />);
    expect(getByTestId("empty-state")).toBeInTheDocument();
    expect(queryByTestId("workspace-main")).toBeNull();
  });

  it("renders EmptyState when lazy is OFF even if an active draft somehow exists", () => {
    hasWorkspacesFlag = false;
    enableLazyFlag = false;
    activeDraftId = "d-1";
    const { getByTestId, queryByTestId } = render(<AppShell />);
    expect(getByTestId("empty-state")).toBeInTheDocument();
    expect(queryByTestId("workspace-main")).toBeNull();
  });

  it("bypasses EmptyState when lazy is ON AND an active draft exists with zero workspaces", () => {
    hasWorkspacesFlag = false;
    enableLazyFlag = true;
    activeDraftId = "d-1";
    const { getByTestId, queryByTestId } = render(<AppShell />);
    expect(getByTestId("workspace-main")).toBeInTheDocument();
    expect(queryByTestId("empty-state")).toBeNull();
    expect(getByTestId("app-sidebar")).toBeInTheDocument();
    expect(getByTestId("title-bar")).toBeInTheDocument();
  });

  it("renders WorkspaceMain when workspaces exist regardless of the lazy flag", () => {
    hasWorkspacesFlag = true;
    enableLazyFlag = false;
    activeDraftId = null;
    const { getByTestId, queryByTestId } = render(<AppShell />);
    expect(getByTestId("workspace-main")).toBeInTheDocument();
    expect(queryByTestId("empty-state")).toBeNull();
  });

  it("renders the Devices page when its UI-store flag is set", async () => {
    hasWorkspacesFlag = true;
    showDevicesFlag = true;
    const { findByTestId, queryByTestId } = render(<AppShell />);
    // The page is a lazy route, so it resolves after a microtask tick.
    expect(await findByTestId("devices-view")).toBeInTheDocument();
    // The shell early-returns the overlay so neither the regular
    // workspace pane nor the empty state should render alongside it.
    expect(queryByTestId("workspace-main")).toBeNull();
    expect(queryByTestId("empty-state")).toBeNull();
  });

  it("prefers Settings over the Devices page when both flags are set", async () => {
    showSettingsFlag = true;
    showDevicesFlag = true;
    const { findByTestId, queryByTestId } = render(<AppShell />);
    expect(await findByTestId("settings-view")).toBeInTheDocument();
    expect(queryByTestId("devices-view")).toBeNull();
  });

  it("marks shell ready, paints twice, then schedules idle prefetch", () => {
    const frames: FrameRequestCallback[] = [];
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
      frames.push(callback);
      return frames.length;
    });
    vi.stubGlobal("cancelAnimationFrame", vi.fn());
    syncedLoading = true;
    const { rerender } = render(<AppShell />);
    expect(markStartupSpy).not.toHaveBeenCalled();
    expect(schedulePrefetchSpy).not.toHaveBeenCalled();

    syncedLoading = false;
    rerender(<AppShell />);
    expect(markStartupSpy).toHaveBeenCalledWith("shell-ready");
    expect(schedulePrefetchSpy).not.toHaveBeenCalled();

    frames.shift()?.(1);
    expect(schedulePrefetchSpy).not.toHaveBeenCalled();
    frames.shift()?.(2);
    expect(markStartupSpy).toHaveBeenCalledWith("shell-first-paint");
    expect(schedulePrefetchSpy).toHaveBeenCalledTimes(1);
    vi.unstubAllGlobals();
  });
});

describe("AppShell theme activation", () => {
  beforeEach(resetMockState);

  it("does not touch the theme while the stores are still loading", () => {
    // The inline boot script has already painted the user's theme; the synced
    // store is still handing out the DEFAULT_SETTINGS placeholder here, so
    // applying it would repaint to Graphite and persist it over the boot
    // shadow.
    syncedLoading = true;
    render(<AppShell />);
    expect(applyThemeSpy).not.toHaveBeenCalled();

    cleanup();
    syncedLoading = false;
    settingsLoaded = false;
    render(<AppShell />);
    expect(applyThemeSpy).not.toHaveBeenCalled();
  });

  it("applies the synced theme once both stores have loaded", () => {
    syncedThemeId = "ocean";
    render(<AppShell />);
    expect(applyThemeSpy).toHaveBeenCalledTimes(1);
    expect(applyThemeSpy.mock.calls[0]![0]).toMatchObject({ id: "ocean" });
    // Persistence is left at its default: this is the real applied theme, so
    // it is exactly what the next boot should paint.
    expect(applyThemeSpy.mock.calls[0]![1]).toBeUndefined();
  });

  it("keeps density on the root even before the theme is applied", () => {
    syncedLoading = true;
    render(<AppShell />);
    expect(document.documentElement.dataset.density).toBe("comfortable");
  });
});

describe("AppShell legacy warm-palette migration", () => {
  beforeEach(resetMockState);

  it("migrates a stored warm palette once and then retires the local key", async () => {
    localSettings = { "appearance.palette": "warm" };
    const { rerender } = render(<AppShell />);

    await waitFor(() =>
      expect(updateSyncedSetting).toHaveBeenCalledWith(
        "appearance",
        "theme",
        "warm",
      ),
    );
    await waitFor(() =>
      expect(setLocalSetting).toHaveBeenCalledWith("appearance.palette", "cool"),
    );
    expect(updateSyncedSetting).toHaveBeenCalledTimes(1);

    // The user now deliberately picks Graphite. With the legacy key retired the
    // migration must stay quiet — it used to re-fire forever, making Graphite
    // unselectable on any machine that ever ran the Warm palette.
    syncedThemeId = "default";
    applyThemeSpy.mockClear();
    rerender(<AppShell />);
    await waitFor(() =>
      expect(applyThemeSpy.mock.calls[0]?.[0]).toMatchObject({ id: "default" }),
    );
    expect(updateSyncedSetting).toHaveBeenCalledTimes(1);
  });

  it("does not migrate while the stores are still loading", () => {
    // The placeholder synced id reads as "default" during load; acting on it
    // would stamp "warm" over whatever theme the account actually holds.
    localSettings = { "appearance.palette": "warm" };
    syncedThemeId = "iris";
    syncedLoading = true;
    render(<AppShell />);
    expect(updateSyncedSetting).not.toHaveBeenCalled();
  });

  it("leaves an explicitly chosen theme alone", () => {
    localSettings = { "appearance.palette": "warm" };
    syncedThemeId = "ember";
    render(<AppShell />);
    expect(updateSyncedSetting).not.toHaveBeenCalled();
    expect(applyThemeSpy.mock.calls[0]![0]).toMatchObject({ id: "ember" });
  });
});
