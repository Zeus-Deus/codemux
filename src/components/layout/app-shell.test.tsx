/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render } from "@testing-library/react";

// ── Mutable mock state ──
let hasWorkspacesFlag = false;
let enableLazyFlag = false;
let activeDraftId: string | null = null;
let appStateReady = true;
let settingsLoaded = true;
let syncedLoading = false;
let showSettingsFlag = false;
let showAutomationsFlag = false;
let showWorkspacesOverviewFlag = false;
let showNewProjectScreenFlag = false;
let commandPaletteOpenFlag = false;

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
// The context bar has its own dedicated test file
// (workspace-context-bar.test.tsx); here it's just a shell child.
vi.mock("./workspace-context-bar", () => ({
  WorkspaceContextBar: () => <div data-testid="workspace-context-bar" />,
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
vi.mock("@/components/workspaces-overview/workspaces-overview-view", () => ({
  WorkspacesOverviewView: () => (
    <div data-testid="workspaces-overview-view" />
  ),
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
        showWorkspacesOverview: showWorkspacesOverviewFlag,
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
        settings: {},
      }),
    ),
    { getState: () => ({ load: vi.fn() }) },
  ),
  // Appearance selectors read by AppShell to apply palette/density attrs.
  selectPalette: () => "cool",
  selectDensity: () => "comfortable",
}));

vi.mock("@/stores/synced-settings-store", () => ({
  useSyncedSettingsStore: vi.fn((selector: (s: unknown) => unknown) =>
    selector({ isLoading: syncedLoading }),
  ),
}));

import { AppShell } from "./app-shell";

afterEach(() => cleanup());

describe("AppShell rendering gates", () => {
  beforeEach(() => {
    hasWorkspacesFlag = false;
    enableLazyFlag = false;
    activeDraftId = null;
    appStateReady = true;
    settingsLoaded = true;
    syncedLoading = false;
    showSettingsFlag = false;
    showAutomationsFlag = false;
    showWorkspacesOverviewFlag = false;
    showNewProjectScreenFlag = false;
    commandPaletteOpenFlag = false;
  });

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

  it("renders the Workspaces overview when its UI-store flag is set", () => {
    hasWorkspacesFlag = true;
    showWorkspacesOverviewFlag = true;
    const { getByTestId, queryByTestId } = render(<AppShell />);
    expect(getByTestId("workspaces-overview-view")).toBeInTheDocument();
    // The shell early-returns the overlay so neither the regular
    // workspace pane nor the empty state should render alongside it.
    expect(queryByTestId("workspace-main")).toBeNull();
    expect(queryByTestId("empty-state")).toBeNull();
  });

  it("prefers Settings over the Workspaces overview when both flags are set", () => {
    showSettingsFlag = true;
    showWorkspacesOverviewFlag = true;
    const { getByTestId, queryByTestId } = render(<AppShell />);
    expect(getByTestId("settings-view")).toBeInTheDocument();
    expect(queryByTestId("workspaces-overview-view")).toBeNull();
  });
});
