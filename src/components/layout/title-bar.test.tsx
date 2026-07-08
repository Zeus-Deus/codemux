/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render } from "@testing-library/react";

import type {
  PresetStoreSnapshot,
  WorkspaceSnapshot,
  WorkspaceType,
} from "@/tauri/types";

const state = {
  enableAgentChat: false,
  enableLazy: false,
  activeDraftId: null as string | null,
  workspaceId: "ws-1" as string | null,
  workspaceType: "standard" as WorkspaceType | null,
};

// Heavy / GUI-only children → sentinels.
vi.mock("./title-bar-tabs", () => ({
  TitleBarTabs: () => <div data-testid="titlebar-tabs" />,
}));
vi.mock("./agent-launcher", () => ({
  AgentLauncher: () => <div data-testid="agent-launcher" />,
}));
vi.mock("./run-button", () => ({
  RunButton: () => <div data-testid="run-button" />,
}));
vi.mock("./resource-monitor", () => ({
  ResourceMonitor: () => <div data-testid="resource-monitor" />,
}));
vi.mock("./window-chrome", () => ({
  WindowControls: () => <div data-testid="window-controls" />,
}));

const chatPresetSnapshot: PresetStoreSnapshot = {
  bar_visible: true,
  default_preset_id: null,
  presets: [
    {
      id: "builtin-chat-agent",
      name: "Chat Agent",
      description: null,
      commands: [],
      working_directory: null,
      launch_mode: "new_tab",
      icon: "chat-agent",
      pinned: true,
      is_builtin: true,
      auto_run_on_workspace: false,
      auto_run_on_new_tab: false,
      kind: "chat_agent",
      launch_config: null,
    },
  ],
};

vi.mock("@/hooks/use-preset-store", () => ({
  usePresetStore: () => chatPresetSnapshot,
}));

vi.mock("@/stores/app-store", () => ({
  useActiveWorkspace: () => makeWorkspace(),
  useActiveWorkspaceId: () => state.workspaceId,
  useAppStore: vi.fn((sel: (s: unknown) => unknown) =>
    sel({
      appState: {
        active_workspace_id: state.workspaceId,
        workspaces: [
          {
            workspace_id: "ws-1",
            workspace_type: state.workspaceType,
          },
        ],
      },
    }),
  ),
}));

vi.mock("@/stores/chat-draft-store", () => ({
  useChatDraftStore: vi.fn((sel: (s: unknown) => unknown) =>
    sel({ activeDraftId: state.activeDraftId }),
  ),
}));

vi.mock("@/stores/feature-flags", () => ({
  useFeatureFlags: vi.fn((sel: (s: unknown) => unknown) =>
    sel({
      enableAgentChat: state.enableAgentChat,
      enableLazyWorkspaceCreation: state.enableLazy,
    }),
  ),
}));

vi.mock("@/stores/ui-store", () => ({
  useUIStore: Object.assign(
    vi.fn((sel: (s: unknown) => unknown) =>
      sel({
        rightPanelTabs: {},
        setRightPanelTab: vi.fn(),
      }),
    ),
    { getState: () => ({ setShowSettings: vi.fn() }) },
  ),
}));

vi.mock("@/stores/synced-settings-store", () => ({
  useSyncedSettingsStore: Object.assign(
    vi.fn(() => null),
    { getState: () => ({ isLoading: false, updateSetting: vi.fn() }) },
  ),
  selectDefaultEditor: () => null,
}));

vi.mock("@/tauri/commands", () => ({
  detectEditors: vi.fn().mockResolvedValue([]),
  openInEditor: vi.fn().mockResolvedValue(undefined),
  agentChatCreatePane: vi.fn().mockResolvedValue("pane-new"),
}));

vi.mock("@/lib/toast", () => ({
  toast: { error: vi.fn(), success: vi.fn(), info: vi.fn(), warning: vi.fn() },
}));

function makeWorkspace(): WorkspaceSnapshot {
  return {
    workspace_id: "ws-1",
    title: "demo",
    workspace_type: "standard",
    cwd: "/p",
    git_branch: "main",
    git_ahead: 0,
    git_behind: 0,
    git_additions: 0,
    git_deletions: 0,
    git_changed_files: 0,
    notification_count: 0,
    latest_agent_state: null,
    worktree_path: null,
    project_root: "/p",
    pr_number: null,
    pr_state: null,
    pr_url: null,
    linked_issue: null,
    notifications_muted: false,
    tabs: [],
    active_tab_id: "tab-1",
    active_surface_id: "surface-1",
    surfaces: [],
  };
}

import { TooltipProvider } from "@/components/ui/tooltip";
import { TitleBar } from "./title-bar";

function renderBar() {
  return render(
    <TooltipProvider>
      <TitleBar sidebarOpen onToggleSidebar={() => {}} />
    </TooltipProvider>,
  );
}

beforeEach(() => {
  state.enableAgentChat = false;
  state.enableLazy = false;
  state.activeDraftId = null;
  state.workspaceId = "ws-1";
  state.workspaceType = "standard";
});

afterEach(cleanup);

describe("TitleBar chrome gating", () => {
  it("renders the legacy bar (no tabs/launcher) when the Beta is OFF", () => {
    state.enableAgentChat = false;
    const { queryByTestId } = renderBar();
    expect(queryByTestId("titlebar-tabs")).toBeNull();
    expect(queryByTestId("agent-launcher")).toBeNull();
    expect(queryByTestId("window-controls")).toBeInTheDocument();
  });

  it("merges tabs + launcher into the bar when the Beta is ON", () => {
    state.enableAgentChat = true;
    const { getByTestId } = renderBar();
    expect(getByTestId("titlebar-tabs")).toBeInTheDocument();
    expect(getByTestId("agent-launcher")).toBeInTheDocument();
    // Rehomed right cluster.
    expect(getByTestId("run-button")).toBeInTheDocument();
    expect(getByTestId("titlebar-favorite-builtin-chat-agent")).toBeInTheDocument();
  });

  it("keeps the legacy bar for OpenFlow workspaces even with the Beta ON", () => {
    state.enableAgentChat = true;
    state.workspaceType = "open_flow";
    const { queryByTestId } = renderBar();
    expect(queryByTestId("titlebar-tabs")).toBeNull();
    expect(queryByTestId("agent-launcher")).toBeNull();
  });

  it("keeps the legacy bar while a lazy-creation draft is active", () => {
    state.enableAgentChat = true;
    state.enableLazy = true;
    state.activeDraftId = "draft-1";
    const { queryByTestId } = renderBar();
    expect(queryByTestId("titlebar-tabs")).toBeNull();
  });
});
