/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, act, cleanup } from "@testing-library/react";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { WorkspaceSnapshot } from "@/tauri/types";

const setShowNewWorkspaceDialogMock = vi.fn();
const clearExpandProjectRequestMock = vi.fn();
let enableAgentChatFlag = false;
let enableLazyFlag = false;
// Drives the mocked useUIStore's `expandProjectRequest`; tests flip it then
// rerender to simulate the "Needs you" strip asking a group to expand.
let expandProjectRequestValue: string | null = null;

const mockOpenUrl = vi.fn().mockResolvedValue(undefined);
vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: (...args: unknown[]) => mockOpenUrl(...args),
}));

// Mock Tauri commands
vi.mock("@/tauri/commands", () => ({
  activateWorkspace: vi.fn().mockResolvedValue(undefined),
  archiveWorkspace: vi.fn().mockResolvedValue("archive-1"),
  unarchiveWorkspace: vi.fn().mockResolvedValue("ws-1"),
  checkoutDefaultBranchInWorkspace: vi.fn().mockResolvedValue("main"),
  closeWorkspace: vi.fn().mockResolvedValue(undefined),
  closeWorkspaceWithWorktree: vi.fn().mockResolvedValue(undefined),
  renameWorkspace: vi.fn().mockResolvedValue(undefined),
  setWorkspaceMuted: vi.fn().mockResolvedValue(undefined),
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
  // Cloud-push step 2 additions — same shape as the other mock in
  // sidebar-workspace-row.test.tsx.
  hostsList: vi.fn().mockResolvedValue([]),
  setWorkspaceHost: vi.fn().mockResolvedValue(undefined),
  workspacePushToHost: vi
    .fn()
    .mockResolvedValue({ ok: true, message: "", remote_path: null, rsync_summary: null }),
  workspacePullBack: vi
    .fn()
    .mockResolvedValue({ ok: true, message: "", rsync_summary: null }),
}));

// `useDefaultBranch` uses a module-level cache; reset between suites so a
// mock return from a prior test doesn't leak in.
import { __resetDefaultBranchCacheForTests } from "./sidebar-workspace-row.test-utils";
import { useSidebarDensityStore } from "@/stores/sidebar-density-store";
beforeEach(() => {
  __resetDefaultBranchCacheForTests();
  // Row density is now derived from live agent + git state (the old
  // Clean/Branch/Detailed setting is gone). Reset the non-persisted density
  // store so seen/settled timestamps never leak between cases.
  useSidebarDensityStore.setState({
    statusSince: {},
    settledAt: {},
    lastSeenAt: {},
  });
  // No pending expand request by default; the expand-on-request suite opts in.
  expandProjectRequestValue = null;
  clearExpandProjectRequestMock.mockClear();
});

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
      expandProjectRequest: expandProjectRequestValue,
      clearExpandProjectRequest: clearExpandProjectRequestMock,
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
  dbGetUiState,
  dbSetUiState,
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

describe("SidebarProjectGroup — expand on request (Needs-you jump)", () => {
  const PATH = "/home/user/collapsed-proj";

  const flush = async () => {
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });
  };

  const renderGroup = (ws: WorkspaceSnapshot) =>
    render(
      <TooltipProvider>
        <SidebarProjectGroup
          projectName="Collapsed"
          projectPath={PATH}
          workspaces={[ws]}
          activeWorkspaceId=""
        />
      </TooltipProvider>,
    );

  beforeEach(() => {
    vi.mocked(dbGetUiState).mockReset();
    // Group loads collapsed from persisted UI state; other keys resolve null.
    vi.mocked(dbGetUiState).mockImplementation((key: string) =>
      Promise.resolve(key === `collapsed:project:${PATH}` ? "true" : null),
    );
    vi.mocked(dbSetUiState).mockReset();
    vi.mocked(dbSetUiState).mockResolvedValue(undefined);
  });

  it("expands + persists a collapsed group when a matching request arrives, then clears it", async () => {
    const ws = makeWorkspace({ workspace_id: "ws-hidden", title: "Hidden Row" });
    const utils = renderGroup(ws);
    // Persisted collapsed state loads → the row is hidden.
    await flush();
    expect(screen.queryByText("Hidden Row")).not.toBeInTheDocument();

    // A jump targets this group.
    expandProjectRequestValue = PATH;
    await act(async () => {
      utils.rerender(
        <TooltipProvider>
          <SidebarProjectGroup
            projectName="Collapsed"
            projectPath={PATH}
            workspaces={[ws]}
            activeWorkspaceId=""
          />
        </TooltipProvider>,
      );
    });

    // Group expands (row now rendered), persists the new state, and clears the
    // one-shot request.
    expect(screen.getByText("Hidden Row")).toBeInTheDocument();
    expect(dbSetUiState).toHaveBeenCalledWith(
      `collapsed:project:${PATH}`,
      "false",
    );
    expect(clearExpandProjectRequestMock).toHaveBeenCalledWith(PATH);
  });

  it("ignores an expand request that targets a different project path", async () => {
    const ws = makeWorkspace({ workspace_id: "ws-hidden", title: "Hidden Row" });
    const utils = renderGroup(ws);
    await flush();
    expect(screen.queryByText("Hidden Row")).not.toBeInTheDocument();

    // Request names a different group → this one stays collapsed and does not
    // consume (clear) the request.
    expandProjectRequestValue = "/home/user/some-other-proj";
    await act(async () => {
      utils.rerender(
        <TooltipProvider>
          <SidebarProjectGroup
            projectName="Collapsed"
            projectPath={PATH}
            workspaces={[ws]}
            activeWorkspaceId=""
          />
        </TooltipProvider>,
      );
    });

    expect(screen.queryByText("Hidden Row")).not.toBeInTheDocument();
    expect(dbSetUiState).not.toHaveBeenCalledWith(
      `collapsed:project:${PATH}`,
      "false",
    );
    expect(clearExpandProjectRequestMock).not.toHaveBeenCalled();
  });
});

describe("SidebarWorkspaceRow", () => {
  it("shows Laptop icon for primary checkout (no worktree_path)", () => {
    const ws = makeWorkspace({ worktree_path: null });
    const { container } = render(
      <TooltipProvider>
        <SidebarWorkspaceRow workspace={ws} isActive={false} />
      </TooltipProvider>,
    );
    // Laptop icon renders as an SVG — check for the lucide class
    const laptopIcon = container.querySelector("svg.lucide-laptop");
    expect(laptopIcon).toBeInTheDocument();
  });

  it("shows GitBranch icon for worktree checkout", () => {
    const ws = makeWorkspace({ worktree_path: "/home/user/.worktrees/feature" });
    const { container } = render(
      <TooltipProvider>
        <SidebarWorkspaceRow workspace={ws} isActive={false} />
      </TooltipProvider>,
    );
    const branchIcon = container.querySelector("svg.lucide-git-branch");
    expect(branchIcon).toBeInTheDocument();
  });

  // ── PR-state icon replaces the GitBranch icon ──
  //
  // When a worktree workspace has a pull request, the leading icon turns
  // into the PR-state-colored icon so the row carries the open/merged/
  // closed signal at its leading edge. The right cluster keeps just the
  // muted "#39" number.

  it("shows GitPullRequest icon (not GitBranch) when the worktree has an open PR", () => {
    const ws = makeWorkspace({
      worktree_path: "/home/user/.worktrees/feature",
      pr_state: "OPEN",
      pr_number: 39,
      pr_url: "https://github.com/u/r/pull/39",
    });
    const { container } = render(
      <TooltipProvider>
        <SidebarWorkspaceRow workspace={ws} isActive={false} />
      </TooltipProvider>,
    );
    // PrStatusIcon for "open" renders the GitPullRequest lucide icon.
    expect(container.querySelector("svg.lucide-git-pull-request")).toBeInTheDocument();
    // The plain branch icon is replaced.
    expect(container.querySelector("svg.lucide-git-branch")).toBeNull();
  });

  it("shows GitMerge icon when the worktree's PR is merged", () => {
    const ws = makeWorkspace({
      worktree_path: "/home/user/.worktrees/feature",
      pr_state: "MERGED",
      pr_number: 39,
      pr_url: "https://github.com/u/r/pull/39",
    });
    const { container } = render(
      <TooltipProvider>
        <SidebarWorkspaceRow workspace={ws} isActive={false} />
      </TooltipProvider>,
    );
    expect(container.querySelector("svg.lucide-git-merge")).toBeInTheDocument();
    expect(container.querySelector("svg.lucide-git-branch")).toBeNull();
  });

  it("shows the closed PR icon when the worktree's PR is closed", () => {
    const ws = makeWorkspace({
      worktree_path: "/home/user/.worktrees/feature",
      pr_state: "CLOSED",
      pr_number: 39,
      pr_url: "https://github.com/u/r/pull/39",
    });
    const { container } = render(
      <TooltipProvider>
        <SidebarWorkspaceRow workspace={ws} isActive={false} />
      </TooltipProvider>,
    );
    expect(container.querySelector("svg.lucide-git-pull-request-closed")).toBeInTheDocument();
    expect(container.querySelector("svg.lucide-git-branch")).toBeNull();
  });

  it("opens the PR URL when the leading PR icon is clicked (stops propagation, does not activate workspace)", () => {
    mockOpenUrl.mockClear();
    (activateWorkspace as ReturnType<typeof vi.fn>).mockClear();
    const ws = makeWorkspace({
      worktree_path: "/home/user/.worktrees/feature",
      pr_state: "OPEN",
      pr_number: 39,
      pr_url: "https://github.com/u/r/pull/39",
    });
    const { container } = render(
      <TooltipProvider>
        <SidebarWorkspaceRow workspace={ws} isActive={false} />
      </TooltipProvider>,
    );
    const btn = container.querySelector("button[aria-label*='Open PR #39']");
    expect(btn).not.toBeNull();
    fireEvent.click(btn!);
    expect(mockOpenUrl).toHaveBeenCalledWith("https://github.com/u/r/pull/39");
    // The icon click must NOT activate the workspace — that's reserved
    // for the rest of the row.
    expect(activateWorkspace).not.toHaveBeenCalled();
  });

  it("does NOT render the PR number in the trailing cluster (PR signal is fully on the leading icon)", () => {
    const ws = makeWorkspace({
      worktree_path: "/home/user/.worktrees/feature",
      pr_state: "OPEN",
      pr_number: 39,
      pr_url: "https://github.com/u/r/pull/39",
    });
    const { container } = render(
      <TooltipProvider>
        <SidebarWorkspaceRow workspace={ws} isActive={false} />
      </TooltipProvider>,
    );
    // The "#39" duplicate is gone — the leading icon's tooltip already
    // carries the PR number, so showing it again in the row's trailing
    // slot is redundant noise.
    expect(container.textContent).not.toContain("#39");
    // The only PR aria-label belongs to the leading icon button in the
    // icon column — the old colored-pill button is gone.
    const prButtons = container.querySelectorAll("button[aria-label*='Open PR']");
    expect(prButtons.length).toBe(1);
  });

  it("falls back to GitBranch when the worktree has no PR", () => {
    const ws = makeWorkspace({
      worktree_path: "/home/user/.worktrees/feature",
      pr_state: null,
      pr_number: null,
      pr_url: null,
    });
    const { container } = render(
      <TooltipProvider>
        <SidebarWorkspaceRow workspace={ws} isActive={false} />
      </TooltipProvider>,
    );
    expect(container.querySelector("svg.lucide-git-branch")).toBeInTheDocument();
    expect(container.querySelector("svg.lucide-git-pull-request")).toBeNull();
  });

  // The hover-reveal action is the (non-destructive) archive button and
  // renders for EVERY row — primary/protected included — since archiving
  // never touches files. The old delete-only X button is gone.
  it("shows the archive button for primary checkout", () => {
    const ws = makeWorkspace({ worktree_path: null });
    const { container } = render(
      <TooltipProvider>
        <SidebarWorkspaceRow workspace={ws} isActive={false} />
      </TooltipProvider>,
    );
    expect(container.querySelector("[aria-label='Archive workspace']")).not.toBeNull();
  });

  it("shows the archive button for worktree checkout", () => {
    const ws = makeWorkspace({ worktree_path: "/home/user/.worktrees/feature" });
    const { container } = render(
      <TooltipProvider>
        <SidebarWorkspaceRow workspace={ws} isActive={false} />
      </TooltipProvider>,
    );
    expect(container.querySelector("[aria-label='Archive workspace']")).not.toBeNull();
  });

  it("shows ahead/behind indicators when counts > 0", () => {
    const ws = makeWorkspace({ git_ahead: 3, git_behind: 1 });
    render(
      <TooltipProvider>
        <SidebarWorkspaceRow workspace={ws} isActive={false} />
      </TooltipProvider>,
    );
    expect(screen.getByText("↑3")).toBeInTheDocument();
    expect(screen.getByText("↓1")).toBeInTheDocument();
  });

  it("hides ahead/behind when both are 0", () => {
    const ws = makeWorkspace({ git_ahead: 0, git_behind: 0 });
    const { container } = render(
      <TooltipProvider>
        <SidebarWorkspaceRow workspace={ws} isActive={false} />
      </TooltipProvider>,
    );
    expect(container.textContent).not.toMatch(/↑\d/);
    expect(container.textContent).not.toMatch(/↓\d/);
  });

  it("shows diff counts on non-active workspaces", () => {
    const ws = makeWorkspace({ git_additions: 42, git_deletions: 7 });
    render(
      <TooltipProvider>
        <SidebarWorkspaceRow workspace={ws} isActive={false} />
      </TooltipProvider>,
    );
    expect(screen.getByText("+42")).toBeInTheDocument();
    expect(screen.getByText("−7")).toBeInTheDocument();
  });

  it("shows diff counts on active workspace too", () => {
    const ws = makeWorkspace({ git_additions: 10, git_deletions: 3 });
    render(
      <TooltipProvider>
        <SidebarWorkspaceRow workspace={ws} isActive={true} />
      </TooltipProvider>,
    );
    expect(screen.getByText("+10")).toBeInTheDocument();
    expect(screen.getByText("−3")).toBeInTheDocument();
  });

  it("shows linked issue number when linked_issue is present", () => {
    const ws = makeWorkspace({
      linked_issue: { number: 92, title: "Backend endpoints", state: "Open", labels: [] },
    });
    render(
      <TooltipProvider>
        <SidebarWorkspaceRow workspace={ws} isActive={false} />
      </TooltipProvider>,
    );
    expect(screen.getByText("#92")).toBeInTheDocument();
  });

  it("does NOT show linked issue when linked_issue is null", () => {
    const ws = makeWorkspace({ linked_issue: null });
    const { container } = render(
      <TooltipProvider>
        <SidebarWorkspaceRow workspace={ws} isActive={false} />
      </TooltipProvider>,
    );
    // No issue number should appear
    expect(container.textContent).not.toMatch(/#\d+/);
  });

  it("shows green dot for open issues and muted dot for closed", () => {
    const wsOpen = makeWorkspace({
      linked_issue: { number: 10, title: "Open issue", state: "Open", labels: [] },
    });
    const { container: c1 } = render(
      <TooltipProvider>
        <SidebarWorkspaceRow workspace={wsOpen} isActive={false} />
      </TooltipProvider>,
    );
    const openDot = c1.querySelector(".bg-success");
    expect(openDot).toBeInTheDocument();

    const wsClosed = makeWorkspace({
      workspace_id: "ws-closed",
      linked_issue: { number: 11, title: "Closed issue", state: "Closed", labels: [] },
    });
    const { container: c2 } = render(
      <TooltipProvider>
        <SidebarWorkspaceRow workspace={wsClosed} isActive={false} />
      </TooltipProvider>,
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
        <TooltipProvider>
          <SidebarWorkspaceRow workspace={ws} isActive={false} />
        </TooltipProvider>,
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
        <TooltipProvider>
          <SidebarWorkspaceRow workspace={ws} isActive={false} />
        </TooltipProvider>,
      );
      const row = container.querySelector("[role='button']") as HTMLElement;
      fireEvent.click(row);

      expect(activateWorkspace).toHaveBeenCalledWith("ws-target-2");
      expect(useChatDraftStore.getState().activeDraftId).toBeNull();
    });
  });
});
