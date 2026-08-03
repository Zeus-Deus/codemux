/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type {
  PresetStoreSnapshot,
  TerminalPreset,
  WorkspaceSnapshot,
  WorkspaceType,
} from "@/tauri/types";

const state = {
  enableAgentChat: false,
  enableLazy: false,
  activeDraftId: null as string | null,
  workspaceId: "ws-1" as string | null,
  workspaceType: "standard" as WorkspaceType | null,
  rightPanelTab: null as "files" | null,
};

// Heavy / GUI-only children → sentinels.
vi.mock("./title-bar-tabs", () => ({
  TitleBarTabs: () => <div data-testid="titlebar-tabs" />,
}));
vi.mock("./agent-launcher", () => ({
  AgentLauncher: () => <div data-testid="agent-launcher" />,
  DraftAgentLauncher: () => <div data-testid="draft-agent-launcher" />,
}));
vi.mock("./run-button", () => ({
  RunButton: () => <div data-testid="run-button" />,
}));
vi.mock("./resource-monitor", () => ({
  ResourceMonitor: ({ variant }: { variant?: string }) => (
    <div data-testid="resource-monitor" data-variant={variant ?? "ghost"} />
  ),
}));
vi.mock("./window-chrome", () => ({
  WindowControls: () => <div data-testid="window-controls" />,
}));

const chatPreset: TerminalPreset = {
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
};

const pinnedCliPreset: TerminalPreset = {
  id: "cli-claude",
  name: "Claude Code",
  description: null,
  commands: ["claude"],
  working_directory: null,
  launch_mode: "new_tab",
  icon: "claude",
  pinned: true,
  is_builtin: true,
  auto_run_on_workspace: false,
  auto_run_on_new_tab: false,
  kind: "cli",
  launch_config: null,
};

const chatPresetSnapshot: PresetStoreSnapshot = {
  bar_visible: true,
  default_preset_id: null,
  presets: [chatPreset],
};

// Mutable so individual tests can swap in a pinned CLI preset alongside the
// chat favorite without affecting the other describe blocks.
let presetSnapshot: PresetStoreSnapshot = chatPresetSnapshot;

vi.mock("@/hooks/use-preset-store", () => ({
  usePresetStore: () => presetSnapshot,
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

vi.mock("@/stores/chat-draft-store", () => {
  interface MockDraftState {
    activeDraftId: string | null;
    draftsById: Record<string, unknown>;
  }
  const snapshot = (): MockDraftState => ({
    activeDraftId: state.activeDraftId,
    draftsById: state.activeDraftId
      ? {
          [state.activeDraftId]: {
            draftId: state.activeDraftId,
            target: { kind: "project", projectPath: "/p" },
            promoting: false,
          },
        }
      : {},
  });
  return {
    useChatDraftStore: vi.fn((sel: (s: unknown) => unknown) => sel(snapshot())),
    // Mirror the real selector so TitleBarDraftSlots resolves the draft.
    selectActiveDraft: (s: MockDraftState) =>
      s.activeDraftId ? s.draftsById[s.activeDraftId] ?? null : null,
  };
});

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
        rightPanelTabs: state.rightPanelTab
          ? { "ws-1": state.rightPanelTab }
          : {},
        rightPanelWidth: 320,
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
  applyPreset: vi.fn().mockResolvedValue(undefined),
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
import { agentChatCreatePane, applyPreset } from "@/tauri/commands";
import { useTitlebarPinsStore } from "@/stores/titlebar-pins-store";
import { useRemoteConnectionStore } from "@/remote/remote-connection-store";
import {
  clearTitlebarContentUnder,
  publishTitlebarContentUnder,
} from "@/lib/titlebar-content-under";
import { TitleBar } from "./title-bar";

function renderBar() {
  return render(
    // `delayDuration={0}` short-circuits Radix's default hover delay so
    // `userEvent.hover` + `findByText` can settle within the test timeout
    // (same pattern used in preset-bar.test.tsx).
    <TooltipProvider delayDuration={0}>
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
  state.rightPanelTab = null;
  presetSnapshot = chatPresetSnapshot;
  vi.mocked(agentChatCreatePane).mockClear();
  vi.mocked(applyPreset).mockClear();
  // Titlebar tile pins are opt-in and persisted (localStorage) — reset to
  // the real default (empty) before every test so tests don't leak into
  // each other or depend on ordering.
  localStorage.clear();
  useTitlebarPinsStore.setState({ pinnedIds: [] });
});

afterEach(() => {
  cleanup();
  localStorage.clear();
  useTitlebarPinsStore.setState({ pinnedIds: [] });
});

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
    const { getByRole, getByTestId, queryByTestId } = renderBar();
    expect(getByTestId("titlebar-tabs")).toBeInTheDocument();
    expect(getByTestId("agent-launcher")).toBeInTheDocument();
    // Rehomed right cluster.
    const runButton = getByTestId("run-button");
    const resourceMonitor = getByTestId("resource-monitor");
    const panelToggle = getByRole("button", { name: "Open panel" });
    const windowControls = getByTestId("window-controls");
    expect(runButton).toBeInTheDocument();
    expect(resourceMonitor).toBeInTheDocument();
    expect(panelToggle).toBeInTheDocument();
    expect(
      runButton.compareDocumentPosition(panelToggle) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(
      panelToggle.compareDocumentPosition(windowControls) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    // Pinned tiles are opt-in (see the dedicated describe blocks below) —
    // the titlebar-pins store defaults empty, so no tile renders here even
    // though the fixture's chat preset has `pinned: true` (that flag is
    // the unrelated legacy-PresetBar concept).
    expect(queryByTestId("titlebar-favorite-builtin-chat-agent")).toBeNull();
  });

  it("renders GUI chrome as frameless floating clusters instead of a full-width bar", () => {
    state.enableAgentChat = true;
    const { getByTestId } = renderBar();
    const root = getByTestId("floating-titlebar");
    const clusters = [
      getByTestId("titlebar-sidebar-cluster"),
      getByTestId("titlebar-workspace-island"),
      getByTestId("titlebar-action-island"),
    ];

    expect(root.className).toContain("absolute");
    expect(root.className).toContain("pointer-events-none");
    expect(root.className).not.toContain("border-b");
    expect(root.className).not.toContain("bg-card");
    for (const cluster of clusters) {
      expect(cluster.className).not.toContain("border");
      expect(cluster.className).not.toContain("bg-card");
      expect(cluster.className).not.toContain("shadow");
      expect(cluster.className).not.toContain("backdrop-blur");
    }
  });

  it("mirrors the left sidebar glyph for the right-panel toggle", () => {
    state.enableAgentChat = true;
    const { getByRole } = renderBar();

    expect(
      getByRole("button", { name: "Toggle sidebar" }).querySelector(
        ".lucide-panel-left",
      ),
    ).toBeInTheDocument();
    expect(
      getByRole("button", { name: "Open panel" }).querySelector(
        ".lucide-panel-right",
      ),
    ).toBeInTheDocument();
  });

  it("keeps the open right-panel toggle frameless", () => {
    state.enableAgentChat = true;
    state.rightPanelTab = "files";
    const { getByRole } = renderBar();
    const panelToggle = getByRole("button", { name: "Close panel" });
    expect(panelToggle).toHaveClass("text-foreground");
    expect(panelToggle).not.toHaveClass("bg-card");
  });

  it("groups primary actions separately from utility controls", () => {
    state.enableAgentChat = true;
    const { getByRole, getByTestId } = renderBar();
    const primary = getByTestId("titlebar-primary-actions");
    const utilities = getByTestId("titlebar-utility-actions");

    expect(primary).toContainElement(getByTestId("run-button"));
    expect(utilities).toContainElement(getByTestId("resource-monitor"));
    expect(getByTestId("resource-monitor")).toHaveAttribute(
      "data-variant",
      "toolbar",
    );
    expect(utilities).toContainElement(
      getByRole("button", { name: "Open panel" }),
    );
    expect(
      primary.compareDocumentPosition(utilities) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(utilities).toHaveClass("ml-1");
  });

  it("keeps overlap surfaces dormant in the normal frameless state", () => {
    state.enableAgentChat = true;
    const { getByTestId } = renderBar();

    expect(getByTestId("floating-titlebar")).not.toHaveAttribute(
      "data-chat-overlap",
    );
    expect(getByTestId("titlebar-workspace-island")).toHaveClass(
      "titlebar-overlap-surface",
      "h-8",
    );
    expect(getByTestId("titlebar-action-island")).toHaveClass(
      "titlebar-overlap-surface",
      "h-8",
    );
  });

  it("raises only the intersecting islands once transcript content is underneath", async () => {
    state.enableAgentChat = true;
    const transcript = document.createElement("div");
    transcript.dataset.slot = "transcript-list";
    transcript.getBoundingClientRect = () =>
      ({ left: 288, right: 1260, width: 972, height: 800 } as DOMRect);
    document.body.appendChild(transcript);
    const source = Symbol("overlap-test");

    const { getByTestId } = renderBar();
    getByTestId("titlebar-workspace-island").getBoundingClientRect = () =>
      ({ left: 300, right: 470, width: 170, height: 32 } as DOMRect);
    getByTestId("titlebar-action-island").getBoundingClientRect = () =>
      ({ left: 990, right: 1150, width: 160, height: 32 } as DOMRect);

    fireEvent(window, new Event("resize"));
    act(() => publishTitlebarContentUnder("ws-1", source, true));
    await waitFor(() =>
      expect(getByTestId("floating-titlebar")).toHaveAttribute(
        "data-chat-overlap",
        "true",
      ),
    );

    act(() => clearTitlebarContentUnder("ws-1", source));
    transcript.remove();
  });

  it("keeps the legacy bar for OpenFlow workspaces even with the Beta ON", () => {
    state.enableAgentChat = true;
    state.workspaceType = "open_flow";
    const { queryByTestId } = renderBar();
    expect(queryByTestId("titlebar-tabs")).toBeNull();
    expect(queryByTestId("agent-launcher")).toBeNull();
  });

  it("renders the GUI draft chrome (draft pill + draft launcher) while a lazy-creation draft is active", () => {
    // Regression coverage: the draft used to fall back to the legacy
    // h-9 bar + PresetBar rows, so pressing "+" flashed the old chrome
    // until the first prompt materialised the workspace.
    state.enableAgentChat = true;
    state.enableLazy = true;
    state.activeDraftId = "draft-1";
    const { getByTestId, queryByRole, queryByTestId } = renderBar();
    expect(getByTestId("titlebar-draft-tab")).toBeInTheDocument();
    expect(getByTestId("draft-agent-launcher")).toBeInTheDocument();
    // Workspace slots + workspace-scoped controls stay off — the
    // backend's "active workspace" is whatever was focused before the
    // draft opened, which is not what's on screen.
    expect(queryByTestId("titlebar-tabs")).toBeNull();
    expect(queryByTestId("agent-launcher")).toBeNull();
    expect(queryByTestId("run-button")).toBeNull();
    expect(queryByRole("button", { name: "Open panel" })).toBeNull();
    // Shared shell bits still render.
    expect(getByTestId("titlebar-sidebar-cluster")).toBeInTheDocument();
    expect(getByTestId("window-controls")).toBeInTheDocument();
  });

  it("keeps the legacy bar for a draft when the Beta flag is off", () => {
    state.enableAgentChat = false;
    state.enableLazy = true;
    state.activeDraftId = "draft-1";
    const { queryByTestId } = renderBar();
    expect(queryByTestId("titlebar-draft-tab")).toBeNull();
    expect(queryByTestId("draft-agent-launcher")).toBeNull();
  });
});

describe("TitleBar GUI chrome — floating placement", () => {
  it("starts the workspace band after the measured sidebar", () => {
    state.enableAgentChat = true;
    const { getByTestId } = renderBar();
    const band = getByTestId("titlebar-floating-band");
    // jsdom has no real `SidebarProvider` DOM to measure (no
    // `[data-slot="sidebar-gap"]` node exists in this test tree), so
    // `useSidebarGapWidth` stays on its fallback — which matches
    // `SidebarProvider`'s own default width (256px). The floating band
    // begins six pixels beyond that edge and stops before window controls.
    expect(band.style.left).toBe("262px");
    expect(band.style.right).toBe("104px");
  });

  it("moves the workspace actions left of an open right panel", () => {
    state.enableAgentChat = true;
    state.rightPanelTab = "files";
    const { getByTestId } = renderBar();
    expect(getByTestId("titlebar-floating-band").style.right).toBe("328px");
  });

  it("renders the sidebar toggle as a compact frameless cluster", () => {
    state.enableAgentChat = true;
    const { getByTestId } = renderBar();
    const cluster = getByTestId("titlebar-sidebar-cluster");
    expect(cluster.className).toContain("absolute");
    expect(cluster.style.width).toBe("");
    expect(cluster.className).not.toContain("border");
    expect(cluster.className).not.toContain("bg-card");
  });
});

describe("TitleBar GUI chrome — pinned preset tiles default to none", () => {
  it("renders no tiles and no divider when the titlebar-pins store is empty, even though every preset has preset.pinned=true", () => {
    // Regression coverage: `preset.pinned` means "show in the legacy
    // PresetBar" (src-tauri/src/presets.rs ships that true on nearly
    // every built-in). Tiles here must be gated on the separate,
    // user-controlled `useTitlebarPinsStore`, which defaults to empty.
    state.enableAgentChat = true;
    presetSnapshot = {
      ...chatPresetSnapshot,
      presets: [chatPreset, pinnedCliPreset],
    };
    const { queryByTestId } = renderBar();
    expect(queryByTestId("titlebar-favorite-builtin-chat-agent")).toBeNull();
    expect(queryByTestId("titlebar-pin-cli-claude")).toBeNull();
  });
});

describe("TitleBar GUI chrome — pinned preset tiles", () => {
  beforeEach(() => {
    state.enableAgentChat = true;
    presetSnapshot = {
      ...chatPresetSnapshot,
      presets: [chatPreset, pinnedCliPreset],
    };
    // Opt both builtins into the titlebar-pin store so this describe block
    // still exercises tile rendering/launch behavior end to end.
    useTitlebarPinsStore.setState({
      pinnedIds: ["builtin-chat-agent", "cli-claude"],
    });
  });

  it("renders the ember chat favorite tile and launches a new chat tab on click", async () => {
    const { getByTestId } = renderBar();
    await userEvent.click(getByTestId("titlebar-favorite-builtin-chat-agent"));
    expect(vi.mocked(agentChatCreatePane)).toHaveBeenCalledWith(
      "ws-1",
      "claude",
      null,
      "new_tab",
    );
  });

  it("renders a neutral tile for a pinned CLI preset, right of a divider", () => {
    const { getByTestId } = renderBar();
    expect(getByTestId("titlebar-pin-cli-claude")).toBeInTheDocument();
  });

  it("still renders the tile for a CLI preset with preset.pinned=false, as long as its id is in the titlebar-pins store", () => {
    // `preset.pinned` is the unrelated legacy-PresetBar flag — the tile
    // here is gated purely on titlebar-pin store membership.
    presetSnapshot = {
      ...chatPresetSnapshot,
      presets: [chatPreset, { ...pinnedCliPreset, pinned: false }],
    };
    const { getByTestId } = renderBar();
    expect(getByTestId("titlebar-pin-cli-claude")).toBeInTheDocument();
  });

  it("does not render a tile for a preset.pinned=true CLI preset whose id isn't in the titlebar-pins store", () => {
    useTitlebarPinsStore.setState({ pinnedIds: ["builtin-chat-agent"] });
    const { queryByTestId } = renderBar();
    expect(queryByTestId("titlebar-pin-cli-claude")).toBeNull();
  });

  it("launches a pinned CLI preset in a new tab on plain click", async () => {
    const { getByTestId } = renderBar();
    await userEvent.click(getByTestId("titlebar-pin-cli-claude"));
    expect(vi.mocked(applyPreset)).toHaveBeenCalledWith(
      "ws-1",
      "cli-claude",
      "new_tab",
      null,
      null,
    );
  });

  it("splits a pinned CLI preset on Shift-click", () => {
    const { getByTestId } = renderBar();
    // userEvent v14's shift-modifier syntax doesn't reliably set
    // `MouseEvent.shiftKey` on the synthetic click; use `fireEvent.click`
    // with an explicit init, matching the pattern in preset-bar.test.tsx.
    fireEvent.click(getByTestId("titlebar-pin-cli-claude"), { shiftKey: true });
    expect(vi.mocked(applyPreset)).toHaveBeenCalledWith(
      "ws-1",
      "cli-claude",
      "split_pane",
      null,
      null,
    );
  });

  it("shows the preset name in a hover tooltip for a pinned tile", async () => {
    renderBar();
    await userEvent.hover(screen.getByTestId("titlebar-pin-cli-claude"));
    await waitFor(() =>
      expect(screen.getAllByText("Claude Code").length).toBeGreaterThan(0),
    );
  });
});

describe("TitleBar — web-remote connection chip placement", () => {
  afterEach(() => {
    (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__ = undefined;
    useRemoteConnectionStore.setState({
      status: null,
      host: "",
      offlineMessage: null,
    });
  });

  it("renders the connection chip in the right cluster on the web client when connected (legacy bar)", () => {
    (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__ = true;
    useRemoteConnectionStore.getState().setConnected("127.0.0.1:4379");
    state.enableAgentChat = false;
    const { getByTestId } = renderBar();
    expect(getByTestId("remote-connection-chip")).toBeInTheDocument();
  });

  it("renders the connection chip in the right cluster on the web client when connected (GUI chrome bar)", () => {
    (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__ = true;
    useRemoteConnectionStore.getState().setConnected("127.0.0.1:4379");
    state.enableAgentChat = true;
    const { getByTestId } = renderBar();
    expect(getByTestId("remote-connection-chip")).toBeInTheDocument();
  });

  it("renders no chip on desktop (non-remote), byte-identical to before", () => {
    useRemoteConnectionStore.getState().setConnected("127.0.0.1:4379");
    const { queryByTestId } = renderBar();
    expect(queryByTestId("remote-connection-chip")).toBeNull();
  });

  it("renders no chip on the web client while reconnecting (that is the banner's job)", () => {
    (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__ = true;
    useRemoteConnectionStore.getState().setReconnecting("127.0.0.1:4379");
    const { queryByTestId } = renderBar();
    expect(queryByTestId("remote-connection-chip")).toBeNull();
  });
});
