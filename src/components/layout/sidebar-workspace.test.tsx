/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, act, cleanup } from "@testing-library/react";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { WorkspaceSnapshot } from "@/tauri/types";

const setShowNewWorkspaceDialogMock = vi.fn();
let enableAgentChatFlag = false;
let enableLazyFlag = false;

// Mock Tauri commands
vi.mock("@/tauri/commands", () => ({
  activateWorkspace: vi.fn().mockResolvedValue(undefined),
  checkoutDefaultBranchInWorkspace: vi.fn().mockResolvedValue("main"),
  closeWorkspace: vi.fn().mockResolvedValue(undefined),
  closeWorkspaceWithWorktree: vi.fn().mockResolvedValue(undefined),
  renameWorkspace: vi.fn().mockResolvedValue(undefined),
  detectEditors: vi.fn().mockResolvedValue([]),
  getDefaultBranch: vi.fn().mockResolvedValue("main"),
  openInEditor: vi.fn().mockResolvedValue(undefined),
  runWorkspaceSetup: vi.fn().mockResolvedValue(undefined),
  dbGetUiState: vi.fn().mockResolvedValue(null),
  dbSetUiState: vi.fn().mockResolvedValue(undefined),
  revealInFileManager: vi.fn().mockResolvedValue(undefined),
  createEmptyWorkspace: vi.fn().mockResolvedValue("ws-new"),
  agentChatCreatePane: vi.fn().mockResolvedValue("pane-new"),
  getGithubIssue: vi.fn().mockResolvedValue({
    number: 92, title: "Test", state: "Open", labels: [], assignees: [],
    url: "https://github.com/u/r/issues/92", body: null,
  }),
}));

// `useDefaultBranch` uses a module-level cache; reset between suites so a
// mock return from a prior test doesn't leak in.
import { __resetDefaultBranchCacheForTests } from "./sidebar-workspace-row.test-utils";
beforeEach(() => __resetDefaultBranchCacheForTests());

// Flush pending microtasks + unmount before the next test so late-resolving
// `getDefaultBranch` promises finish their React update inside the jsdom
// lifetime instead of after teardown (which would log a spurious
// "window is not defined" from React's scheduler).
afterEach(async () => {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
  cleanup();
});

// Mock stores
vi.mock("@/stores/ui-store", () => ({
  useUIStore: vi.fn((selector) => {
    const state = {
      showNewWorkspaceDialog: false,
      setShowNewWorkspaceDialog: setShowNewWorkspaceDialogMock,
    };
    return selector(state);
  }),
}));

vi.mock("@/stores/feature-flags", () => ({
  useFeatureFlags: vi.fn((selector) => {
    const state = {
      enableAgentChat: enableAgentChatFlag,
      enableLazyWorkspaceCreation: enableLazyFlag,
      loaded: true,
    };
    return selector(state);
  }),
}));

import { SidebarProjectGroup } from "./sidebar-project-group";
import { SidebarWorkspaceRow } from "./sidebar-workspace-row";
import {
  activateWorkspace,
  agentChatCreatePane,
  createEmptyWorkspace,
} from "@/tauri/commands";
import { useChatDraftStore } from "@/stores/chat-draft-store";

function makeWorkspace(overrides: Partial<WorkspaceSnapshot> = {}): WorkspaceSnapshot {
  return {
    workspace_id: "ws-1",
    title: "Test Workspace",
    workspace_type: "standard",
    cwd: "/home/user/projects/myapp",
    git_branch: "main",
    git_ahead: 0,
    git_behind: 0,
    git_additions: 0,
    git_deletions: 0,
    git_changed_files: 0,
    notification_count: 0,
    notifications_muted: false,
    latest_agent_state: null,
    worktree_path: null,
    project_root: null,
    pr_number: null,
    pr_state: null,
    pr_url: null,
    linked_issue: null,
    tabs: [],
    active_tab_id: "",
    active_surface_id: "",
    surfaces: [],
    ...overrides,
  };
}

describe("SidebarProjectGroup", () => {
  it("shows letter avatar with first letter of project name", () => {
    render(
      <TooltipProvider>
        <SidebarProjectGroup
          projectName="codemux"
          projectPath="/home/user/codemux"
          workspaces={[]}
          activeWorkspaceId=""
        />
      </TooltipProvider>,
    );
    // First letter uppercase
    expect(screen.getByText("C")).toBeInTheDocument();
  });

  it("default avatar has neutral styling (no custom color)", () => {
    render(
      <TooltipProvider>
        <SidebarProjectGroup
          projectName="myproject"
          projectPath="/home/user/myproject"
          workspaces={[]}
          activeWorkspaceId=""
        />
      </TooltipProvider>,
    );
    const avatar = screen.getByText("M").closest("div");
    // Should have muted classes, no inline style for color
    expect(avatar).toHaveClass("bg-muted");
    expect(avatar).toHaveClass("text-muted-foreground");
    expect(avatar?.style.color).toBeFalsy();
  });

  describe("+ button click behavior", () => {
    const PROJECT_PATH = "/home/user/myproject";

    function renderGroup() {
      const utils = render(
        <TooltipProvider>
          <SidebarProjectGroup
            projectName="myproject"
            projectPath={PROJECT_PATH}
            workspaces={[]}
            activeWorkspaceId=""
          />
        </TooltipProvider>,
      );
      const plus = utils.container.querySelector(
        'button[aria-label="New workspace"]',
      ) as HTMLElement;
      return { ...utils, plus };
    }

    beforeEach(() => {
      setShowNewWorkspaceDialogMock.mockClear();
      vi.mocked(createEmptyWorkspace).mockClear();
      vi.mocked(createEmptyWorkspace).mockResolvedValue("ws-new");
      vi.mocked(activateWorkspace).mockClear();
      vi.mocked(agentChatCreatePane).mockClear();
      enableAgentChatFlag = false;
      enableLazyFlag = false;
      useChatDraftStore.setState({
        draftsById: {},
        activeHomeDraftId: null,
        projectDraftIdByPath: {},
        activeDraftId: null,
      });
    });

    it("flag OFF + plain click → opens NewWorkspaceDialog", () => {
      enableAgentChatFlag = false;
      const { plus } = renderGroup();
      fireEvent.click(plus);
      expect(setShowNewWorkspaceDialogMock).toHaveBeenCalledWith(true, PROJECT_PATH);
      expect(createEmptyWorkspace).not.toHaveBeenCalled();
      expect(agentChatCreatePane).not.toHaveBeenCalled();
    });

    it("flag OFF + Shift+click → opens NewWorkspaceDialog", () => {
      enableAgentChatFlag = false;
      const { plus } = renderGroup();
      fireEvent.click(plus, { shiftKey: true });
      expect(setShowNewWorkspaceDialogMock).toHaveBeenCalledWith(true, PROJECT_PATH);
      expect(createEmptyWorkspace).not.toHaveBeenCalled();
      expect(agentChatCreatePane).not.toHaveBeenCalled();
    });

    it("flag ON + plain click → creates empty workspace + activates + opens chat pane (no terminal pane)", async () => {
      enableAgentChatFlag = true;
      const { plus } = renderGroup();
      fireEvent.click(plus);
      await vi.waitFor(() => {
        expect(agentChatCreatePane).toHaveBeenCalled();
      });
      // createEmptyWorkspace (not createWorkspace) — the empty variant
      // doesn't spawn a terminal session, so agent_chat_create_pane
      // mounts its own fresh surface containing only the chat pane.
      // No skipSetup arg: projects SHOULD get setup scripts + MCP
      // config; only the home-chat path opts out.
      expect(createEmptyWorkspace).toHaveBeenCalledWith(PROJECT_PATH);
      const call = vi.mocked(createEmptyWorkspace).mock.calls[0];
      expect(call[1]).toBeUndefined();
      expect(activateWorkspace).toHaveBeenCalledWith("ws-new");
      expect(agentChatCreatePane).toHaveBeenCalledWith("ws-new", null, PROJECT_PATH);
      expect(setShowNewWorkspaceDialogMock).not.toHaveBeenCalled();
    });

    it("flag ON + Shift+click → opens dialog, does NOT call chat commands", () => {
      enableAgentChatFlag = true;
      const { plus } = renderGroup();
      fireEvent.click(plus, { shiftKey: true });
      expect(setShowNewWorkspaceDialogMock).toHaveBeenCalledWith(true, PROJECT_PATH);
      expect(createEmptyWorkspace).not.toHaveBeenCalled();
      expect(agentChatCreatePane).not.toHaveBeenCalled();
    });

    it("flag ON + createEmptyWorkspace rejects → falls back to dialog", async () => {
      enableAgentChatFlag = true;
      vi.mocked(createEmptyWorkspace).mockRejectedValueOnce(new Error("boom"));
      const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
      const { plus } = renderGroup();
      fireEvent.click(plus);
      await vi.waitFor(() => {
        expect(setShowNewWorkspaceDialogMock).toHaveBeenCalledWith(true, PROJECT_PATH);
      });
      expect(agentChatCreatePane).not.toHaveBeenCalled();
      errSpy.mockRestore();
    });

    describe("lazy workspace creation (Stage C §10)", () => {
      it("lazy ON + plain click → creates project draft, no Tauri commands called", () => {
        enableAgentChatFlag = true;
        enableLazyFlag = true;
        const { plus } = renderGroup();
        fireEvent.click(plus);

        const state = useChatDraftStore.getState();
        expect(state.projectDraftIdByPath[PROJECT_PATH]).toBeTruthy();
        expect(state.activeDraftId).toBe(state.projectDraftIdByPath[PROJECT_PATH]);
        const draft = state.draftsById[state.activeDraftId!];
        expect(draft.target).toEqual({
          kind: "project",
          projectPath: PROJECT_PATH,
        });
        expect(createEmptyWorkspace).not.toHaveBeenCalled();
        expect(agentChatCreatePane).not.toHaveBeenCalled();
        expect(setShowNewWorkspaceDialogMock).not.toHaveBeenCalled();
      });

      it("lazy ON + plain click reuses the existing project draft on a second click", () => {
        enableAgentChatFlag = true;
        enableLazyFlag = true;
        const { plus } = renderGroup();
        fireEvent.click(plus);
        const firstId =
          useChatDraftStore.getState().projectDraftIdByPath[PROJECT_PATH];
        fireEvent.click(plus);
        expect(
          useChatDraftStore.getState().projectDraftIdByPath[PROJECT_PATH],
        ).toBe(firstId);
        expect(
          Object.keys(useChatDraftStore.getState().draftsById),
        ).toHaveLength(1);
      });

      it("lazy ON + Shift+click still opens the dialog (eager path preserved)", () => {
        enableAgentChatFlag = true;
        enableLazyFlag = true;
        const { plus } = renderGroup();
        fireEvent.click(plus, { shiftKey: true });
        expect(setShowNewWorkspaceDialogMock).toHaveBeenCalledWith(
          true,
          PROJECT_PATH,
        );
        expect(useChatDraftStore.getState().activeDraftId).toBeNull();
      });

      it("lazy OFF + plain click falls through to the legacy eager path", async () => {
        enableAgentChatFlag = true;
        enableLazyFlag = false;
        const { plus } = renderGroup();
        fireEvent.click(plus);
        await vi.waitFor(() => {
          expect(createEmptyWorkspace).toHaveBeenCalled();
        });
        expect(useChatDraftStore.getState().activeDraftId).toBeNull();
      });
    });
  });
});

describe("SidebarWorkspaceRow", () => {
  it("shows Laptop icon for primary checkout (no worktree_path)", () => {
    const ws = makeWorkspace({ worktree_path: null });
    const { container } = render(
      <SidebarWorkspaceRow workspace={ws} isActive={false} />,
    );
    // Laptop icon renders as an SVG — check for the lucide class
    const laptopIcon = container.querySelector("svg.lucide-laptop");
    expect(laptopIcon).toBeInTheDocument();
  });

  it("shows GitBranch icon for worktree checkout", () => {
    const ws = makeWorkspace({ worktree_path: "/home/user/.worktrees/feature" });
    const { container } = render(
      <SidebarWorkspaceRow workspace={ws} isActive={false} />,
    );
    const branchIcon = container.querySelector("svg.lucide-git-branch");
    expect(branchIcon).toBeInTheDocument();
  });

  it("hides remove button for primary checkout (Hide-only via right-click)", () => {
    const ws = makeWorkspace({ worktree_path: null });
    const { container } = render(
      <SidebarWorkspaceRow workspace={ws} isActive={false} />,
    );
    expect(container.querySelector("[aria-label='Remove workspace']")).toBeNull();
  });

  it("shows remove button for worktree checkout", () => {
    const ws = makeWorkspace({ worktree_path: "/home/user/.worktrees/feature" });
    const { container } = render(
      <SidebarWorkspaceRow workspace={ws} isActive={false} />,
    );
    expect(container.querySelector("[aria-label='Remove workspace']")).not.toBeNull();
  });

  it("shows ahead/behind indicators when counts > 0", () => {
    const ws = makeWorkspace({ git_ahead: 3, git_behind: 1 });
    render(
      <SidebarWorkspaceRow workspace={ws} isActive={false} />,
    );
    expect(screen.getByText("↑3")).toBeInTheDocument();
    expect(screen.getByText("↓1")).toBeInTheDocument();
  });

  it("hides ahead/behind when both are 0", () => {
    const ws = makeWorkspace({ git_ahead: 0, git_behind: 0 });
    const { container } = render(
      <SidebarWorkspaceRow workspace={ws} isActive={false} />,
    );
    expect(container.textContent).not.toMatch(/↑\d/);
    expect(container.textContent).not.toMatch(/↓\d/);
  });

  it("shows diff counts on non-active workspaces", () => {
    const ws = makeWorkspace({ git_additions: 42, git_deletions: 7 });
    render(
      <SidebarWorkspaceRow workspace={ws} isActive={false} />,
    );
    expect(screen.getByText("+42")).toBeInTheDocument();
    expect(screen.getByText("−7")).toBeInTheDocument();
  });

  it("shows diff counts on active workspace too", () => {
    const ws = makeWorkspace({ git_additions: 10, git_deletions: 3 });
    render(
      <SidebarWorkspaceRow workspace={ws} isActive={true} />,
    );
    expect(screen.getByText("+10")).toBeInTheDocument();
    expect(screen.getByText("−3")).toBeInTheDocument();
  });

  it("shows linked issue number when linked_issue is present", () => {
    const ws = makeWorkspace({
      linked_issue: { number: 92, title: "Backend endpoints", state: "Open", labels: [] },
    });
    render(
      <SidebarWorkspaceRow workspace={ws} isActive={false} />,
    );
    expect(screen.getByText("#92")).toBeInTheDocument();
  });

  it("does NOT show linked issue when linked_issue is null", () => {
    const ws = makeWorkspace({ linked_issue: null });
    const { container } = render(
      <SidebarWorkspaceRow workspace={ws} isActive={false} />,
    );
    // No issue number should appear
    expect(container.textContent).not.toMatch(/#\d+/);
  });

  it("shows green dot for open issues and muted dot for closed", () => {
    const wsOpen = makeWorkspace({
      linked_issue: { number: 10, title: "Open issue", state: "Open", labels: [] },
    });
    const { container: c1 } = render(
      <SidebarWorkspaceRow workspace={wsOpen} isActive={false} />,
    );
    const openDot = c1.querySelector(".bg-success");
    expect(openDot).toBeInTheDocument();

    const wsClosed = makeWorkspace({
      workspace_id: "ws-closed",
      linked_issue: { number: 11, title: "Closed issue", state: "Closed", labels: [] },
    });
    const { container: c2 } = render(
      <SidebarWorkspaceRow workspace={wsClosed} isActive={false} />,
    );
    const closedDot = c2.querySelector(".bg-muted-foreground");
    expect(closedDot).toBeInTheDocument();
  });

  describe("workspace-row click vs. active draft (Bug 2)", () => {
    beforeEach(() => {
      vi.mocked(activateWorkspace).mockClear();
      useChatDraftStore.setState({
        draftsById: {},
        activeHomeDraftId: null,
        projectDraftIdByPath: {},
        activeDraftId: null,
      });
    });

    it("clicking the row clears any active draft AND activates the workspace", () => {
      // Pre-seed an active draft so WorkspaceMain would otherwise
      // keep the draft surface on screen after activation.
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
      expect(useChatDraftStore.getState().activeDraftId).toBe(draft.draftId);

      const ws = makeWorkspace({ workspace_id: "ws-target" });
      const { container } = render(
        <SidebarWorkspaceRow workspace={ws} isActive={false} />,
      );
      const row = container.querySelector("[role='button']") as HTMLElement;
      fireEvent.click(row);

      expect(useChatDraftStore.getState().activeDraftId).toBeNull();
      expect(activateWorkspace).toHaveBeenCalledWith("ws-target");
    });

    it("clicking the row is a no-op for draft store when no draft is active", () => {
      // No draft pre-seeded; the `setActiveDraft(null)` call is safe
      // (already null) and the click still activates the workspace.
      expect(useChatDraftStore.getState().activeDraftId).toBeNull();

      const ws = makeWorkspace({ workspace_id: "ws-target-2" });
      const { container } = render(
        <SidebarWorkspaceRow workspace={ws} isActive={false} />,
      );
      const row = container.querySelector("[role='button']") as HTMLElement;
      fireEvent.click(row);

      expect(activateWorkspace).toHaveBeenCalledWith("ws-target-2");
      expect(useChatDraftStore.getState().activeDraftId).toBeNull();
    });
  });
});
