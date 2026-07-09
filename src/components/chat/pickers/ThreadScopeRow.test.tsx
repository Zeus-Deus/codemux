/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type { BranchDetail, WorkspaceSnapshot } from "@/tauri/types";

// ── App-store mock — keep
// the real grouping helper, stub the store hooks against a
// test-controlled workspace list. ──
let currentWorkspaces: WorkspaceSnapshot[] = [];

function makeWs(overrides: Partial<WorkspaceSnapshot>): WorkspaceSnapshot {
  return {
    workspace_id: "ws-default",
    title: "ws",
    workspace_type: "standard",
    cwd: "/projects/foo",
    git_branch: null,
    git_ahead: 0,
    git_behind: 0,
    git_additions: 0,
    git_deletions: 0,
    git_changed_files: 0,
    notification_count: 0,
    notifications_muted: false,
    latest_agent_state: null,
    worktree_path: null,
    project_root: "/projects/foo",
    pr_number: null,
    pr_state: null,
    pr_url: null,
    linked_issue: null,
    tabs: [],
    active_tab_id: "",
    active_surface_id: "",
    surfaces: [],
    ...overrides,
  } as WorkspaceSnapshot;
}

vi.mock("@/stores/app-store", async () => {
  const actual =
    await vi.importActual<typeof import("@/stores/app-store")>(
      "@/stores/app-store",
    );
  return {
    ...actual,
    useAppStore: vi.fn((selector: (s: unknown) => unknown) =>
      selector({ appState: { workspaces: currentWorkspaces } }),
    ),
    useHomeDir: () => "/home/user",
    useProjectGroupedWorkspaces: actual.useProjectGroupedWorkspaces,
    resolveProjectRoot: actual.resolveProjectRoot,
  };
});

vi.mock("@/stores/hosts-store", () => ({
  useHosts: () => [],
}));

const mockOpenProject = vi.fn();
vi.mock("@/hooks/use-project-actions", () => ({
  useProjectActions: () => ({ openProject: mockOpenProject }),
}));

vi.mock("@/tauri/commands", () => ({
  dbGetUiState: vi.fn().mockResolvedValue(null),
  listBranchesDetailed: vi.fn(),
}));

import { ThreadScopeRow, type ThreadScopeRowProps } from "./ThreadScopeRow";
import { listBranchesDetailed } from "@/tauri/commands";

afterEach(() => cleanup());

const NOW = Math.floor(Date.now() / 1000);

function branch(name: string, overrides: Partial<BranchDetail> = {}): BranchDetail {
  return {
    name,
    last_commit_unix: NOW - 3600,
    is_local: true,
    is_remote: false,
    ...overrides,
  };
}

function renderRow(
  overrides: Partial<ThreadScopeRowProps> & {
    draftTarget?: import("@/stores/chat-draft-store").DraftTarget;
  } = {},
) {
  const onChangeTarget = vi.fn();
  const onChangeCheckoutMode = vi.fn();
  const onChangeWorktreeName = vi.fn();
  const onChangeBaseBranch = vi.fn();
  const { draftTarget, ...rest } = overrides;
  const props: ThreadScopeRowProps = {
    location: {
      kind: "draft",
      target: draftTarget ?? { kind: "project", projectPath: "/projects/foo" },
      onChangeTarget,
    },
    projectPath: "/projects/foo",
    checkoutMode: "current",
    worktreeName: "",
    baseBranch: "main",
    onChangeCheckoutMode,
    onChangeWorktreeName,
    onChangeBaseBranch,
    ...rest,
  };
  const utils = render(<ThreadScopeRow {...props} />);
  return {
    ...utils,
    onChangeTarget,
    onChangeCheckoutMode,
    onChangeWorktreeName,
    onChangeBaseBranch,
  };
}

/** Workspace-mode variant — `AgentChatPane`'s new-thread empty state. */
function renderWorkspaceRow(
  overrides: Partial<ThreadScopeRowProps> & { isHome?: boolean } = {},
) {
  const onSelectHomeWorkspace = vi.fn();
  const onSelectProject = vi.fn();
  const onChangeCheckoutMode = vi.fn();
  const onChangeWorktreeName = vi.fn();
  const onChangeBaseBranch = vi.fn();
  const { isHome, ...rest } = overrides;
  const props: ThreadScopeRowProps = {
    location: {
      kind: "workspace",
      isHome: isHome ?? false,
      onSelectHomeWorkspace,
      onSelectProject,
    },
    projectPath: isHome ? null : "/projects/foo",
    checkoutMode: "current",
    worktreeName: "",
    baseBranch: "main",
    onChangeCheckoutMode,
    onChangeWorktreeName,
    onChangeBaseBranch,
    ...rest,
  };
  const utils = render(<ThreadScopeRow {...props} />);
  return {
    ...utils,
    onSelectHomeWorkspace,
    onSelectProject,
    onChangeCheckoutMode,
    onChangeWorktreeName,
    onChangeBaseBranch,
  };
}

describe("ThreadScopeRow", () => {
  beforeEach(() => {
    currentWorkspaces = [];
    mockOpenProject.mockReset().mockResolvedValue({ success: false });
    vi.mocked(listBranchesDetailed).mockReset().mockResolvedValue([
      branch("main", { last_commit_unix: NOW - 3600 }),
      branch("develop", { last_commit_unix: NOW - 86400 }),
    ]);
  });

  describe("home target", () => {
    it("renders only the location control and the home scope hint — no checkout/branch controls", () => {
      renderRow({ draftTarget: { kind: "home" }, projectPath: null });
      expect(screen.getByText("Home")).toBeInTheDocument();
      expect(screen.queryByText("Current checkout")).toBeNull();
      expect(screen.queryByText("New worktree")).toBeNull();
      expect(screen.queryByText(/^from$/)).toBeNull();
      expect(
        screen.getByText(/runs on your machine in the home directory/i),
      ).toBeInTheDocument();
    });
  });

  describe("project target — current checkout", () => {
    it("renders location, checkout, and branch controls with the current-checkout hint", () => {
      renderRow();
      expect(screen.getByText("foo")).toBeInTheDocument();
      expect(screen.getByText("Current checkout")).toBeInTheDocument();
      expect(screen.getByText("main")).toBeInTheDocument();
      expect(
        screen.getByText(/current checkout on main/i),
      ).toBeInTheDocument();
    });

    it("checkout mode renders the deferred-worktree hint when checkoutMode is 'worktree'", () => {
      renderRow({ checkoutMode: "worktree", baseBranch: "develop" });
      expect(screen.getByText("New worktree")).toBeInTheDocument();
      expect(
        screen.getByText(/isolated worktree off develop in foo/i),
      ).toBeInTheDocument();
    });

    it("hides checkout/branch controls when projectPath hasn't resolved yet", () => {
      renderRow({
        draftTarget: {
          kind: "existing_workspace",
          workspaceId: "ws-not-here",
        },
        projectPath: null,
      });
      expect(screen.queryByText("Current checkout")).toBeNull();
      expect(screen.queryByText(/^from$/)).toBeNull();
    });
  });

  describe("location control", () => {
    it("selecting Home from the location popover calls onChangeTarget({kind: 'home'})", async () => {
      const user = userEvent.setup();
      const { onChangeTarget } = renderRow();
      await user.click(screen.getByText("foo"));
      await screen.findByText("Run in");
      await user.click(screen.getByText("Home directory (~)"));
      expect(onChangeTarget).toHaveBeenCalledWith({ kind: "home" });
    });

    it("lists known projects and selecting one calls onChangeTarget({kind:'project', projectPath})", async () => {
      currentWorkspaces = [
        makeWs({
          workspace_id: "ws-bar",
          cwd: "/projects/bar",
          project_root: "/projects/bar",
        }),
      ];
      const user = userEvent.setup();
      const { onChangeTarget } = renderRow();
      await user.click(screen.getByText("foo"));
      await screen.findByText("Run in");
      await user.click(screen.getByText("bar"));
      expect(onChangeTarget).toHaveBeenCalledWith({
        kind: "project",
        projectPath: "/projects/bar",
      });
    });

    it("'Open another project…' calls openProject and forwards the picked path", async () => {
      mockOpenProject.mockResolvedValue({
        success: true,
        path: "/projects/opened",
        name: "opened",
      });
      const user = userEvent.setup();
      const { onChangeTarget } = renderRow();
      await user.click(screen.getByText("foo"));
      await screen.findByText("Open another project…");
      await user.click(screen.getByText("Open another project…"));
      await waitFor(() => expect(mockOpenProject).toHaveBeenCalled());
      expect(onChangeTarget).toHaveBeenCalledWith({
        kind: "project",
        projectPath: "/projects/opened",
      });
    });
  });

  describe("checkout control", () => {
    it("selecting 'New worktree' calls onChangeCheckoutMode('worktree')", async () => {
      const user = userEvent.setup();
      const { onChangeCheckoutMode } = renderRow();
      await user.click(screen.getByText("Current checkout"));
      await screen.findByText("Where should the agent work?");
      await user.click(screen.getByText("New worktree"));
      expect(onChangeCheckoutMode).toHaveBeenCalledWith("worktree");
    });

    it("shows the name input + hint only when checkoutMode is 'worktree', and typing calls onChangeWorktreeName", async () => {
      const user = userEvent.setup();
      const { onChangeWorktreeName } = renderRow({ checkoutMode: "worktree" });
      await user.click(screen.getByText("New worktree"));
      const input = await screen.findByPlaceholderText(
        "name — leave empty to auto-name",
      );
      await user.type(input, "x");
      expect(onChangeWorktreeName).toHaveBeenCalledWith("x");
      expect(
        screen.getByText(/CodeMux names it from your first message/i),
      ).toBeInTheDocument();
    });
  });

  describe("branch control", () => {
    it("picking a DIFFERENT branch while on 'current' checkout flips to 'worktree' with that branch as base", async () => {
      const user = userEvent.setup();
      const { onChangeCheckoutMode, onChangeBaseBranch } = renderRow({
        checkoutMode: "current",
        baseBranch: "main",
      });
      await user.click(screen.getByText("main"));
      const developRow = await screen.findByText("develop");
      await user.click(developRow);
      expect(onChangeCheckoutMode).toHaveBeenCalledWith("worktree");
      expect(onChangeBaseBranch).toHaveBeenCalledWith("develop");
    });

    it("picking the SAME branch while on 'current' checkout does not flip checkoutMode", async () => {
      const user = userEvent.setup();
      const { onChangeCheckoutMode, onChangeBaseBranch } = renderRow({
        checkoutMode: "current",
        baseBranch: "main",
      });
      await user.click(screen.getByText("main"));
      const rows = await screen.findAllByText("main");
      // Click the row inside the popover list (not the trigger).
      await user.click(rows[rows.length - 1]);
      expect(onChangeCheckoutMode).not.toHaveBeenCalled();
      expect(onChangeBaseBranch).toHaveBeenCalledWith("main");
    });

    it("picking a branch while already on 'worktree' checkout just updates the base branch", async () => {
      const user = userEvent.setup();
      const { onChangeCheckoutMode, onChangeBaseBranch } = renderRow({
        checkoutMode: "worktree",
        baseBranch: "main",
      });
      await user.click(screen.getByText("main"));
      const developRow = await screen.findByText("develop");
      await user.click(developRow);
      expect(onChangeCheckoutMode).not.toHaveBeenCalled();
      expect(onChangeBaseBranch).toHaveBeenCalledWith("develop");
    });

    it("shows a WORKTREE badge on branches that have a worktree on this device", async () => {
      currentWorkspaces = [
        makeWs({
          workspace_id: "ws-foo-main",
          cwd: "/projects/foo",
          project_root: "/projects/foo",
          git_branch: "main",
        }),
      ];
      const user = userEvent.setup();
      renderRow();
      await user.click(screen.getByText("main"));
      await waitFor(() => {
        expect(screen.getByText("WORKTREE")).toBeInTheDocument();
      });
    });
  });

  describe("workspace mode (AgentChatPane empty state)", () => {
    it("project pane renders location + checkout + branch controls with the project scope hint", () => {
      renderWorkspaceRow();
      expect(screen.getByText("foo")).toBeInTheDocument();
      expect(screen.getByText("Current checkout")).toBeInTheDocument();
      expect(
        screen.getByText(/current checkout on main/i),
      ).toBeInTheDocument();
    });

    it("home-rooted pane renders only the location control + home hint", () => {
      renderWorkspaceRow({ isHome: true });
      expect(screen.getByText("Home")).toBeInTheDocument();
      expect(screen.queryByText("Current checkout")).toBeNull();
      expect(
        screen.getByText(/runs on your machine in the home directory/i),
      ).toBeInTheDocument();
    });

    it("picking a DIFFERENT project calls onSelectProject with its path", async () => {
      currentWorkspaces = [
        makeWs({
          workspace_id: "ws-bar",
          cwd: "/projects/bar",
          project_root: "/projects/bar",
        }),
      ];
      const user = userEvent.setup();
      const { onSelectProject } = renderWorkspaceRow();
      await user.click(screen.getByText("foo"));
      await screen.findByText("Run in");
      await user.click(screen.getByText("bar"));
      expect(onSelectProject).toHaveBeenCalledWith("/projects/bar");
    });

    it("picking the ALREADY-ACTIVE project is a no-op (no navigation)", async () => {
      currentWorkspaces = [
        makeWs({
          workspace_id: "ws-foo",
          cwd: "/projects/foo",
          project_root: "/projects/foo",
        }),
      ];
      const user = userEvent.setup();
      const { onSelectProject } = renderWorkspaceRow();
      await user.click(screen.getByText("foo"));
      await screen.findByText("Run in");
      // The popover row for the active project — click it.
      const rows = screen.getAllByText("foo");
      await user.click(rows[rows.length - 1]);
      expect(onSelectProject).not.toHaveBeenCalled();
    });

    it("hides the Home option when no home-rooted workspace exists (never creates hidden workspaces)", async () => {
      const user = userEvent.setup();
      renderWorkspaceRow();
      await user.click(screen.getByText("foo"));
      await screen.findByText("Run in");
      expect(screen.queryByText("Home directory (~)")).toBeNull();
    });

    it("offers Home when a home-rooted workspace exists, and selecting it activates that workspace", async () => {
      currentWorkspaces = [
        makeWs({
          workspace_id: "ws-home",
          cwd: "/home/user",
          project_root: "/home/user",
        }),
      ];
      const user = userEvent.setup();
      const { onSelectHomeWorkspace } = renderWorkspaceRow();
      await user.click(screen.getByText("foo"));
      await screen.findByText("Run in");
      await user.click(screen.getByText("Home directory (~)"));
      expect(onSelectHomeWorkspace).toHaveBeenCalledWith("ws-home");
    });

    it("'Open another project…' routes through onSelectProject", async () => {
      mockOpenProject.mockResolvedValue({
        success: true,
        path: "/projects/opened",
        name: "opened",
      });
      const user = userEvent.setup();
      const { onSelectProject } = renderWorkspaceRow();
      await user.click(screen.getByText("foo"));
      await screen.findByText("Open another project…");
      await user.click(screen.getByText("Open another project…"));
      await waitFor(() => expect(mockOpenProject).toHaveBeenCalled());
      expect(onSelectProject).toHaveBeenCalledWith("/projects/opened");
    });
  });
});
