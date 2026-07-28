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
  // Probe fallback used by ThreadScopeRow when no workspace row carries
  // the project's `is_git` flag. Defaults to true (git repo) so the
  // checkout/branch controls render as they did pre-probe; individual
  // tests override it to exercise the non-git path.
  checkIsGitRepo: vi.fn().mockResolvedValue(true),
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
  const { draftTarget, checkoutMode, worktreeName, baseBranch, ...rest } =
    overrides;
  const props: ThreadScopeRowProps = {
    target: draftTarget ?? { kind: "project", projectPath: "/projects/foo" },
    onChangeTarget,
    projectPath: "/projects/foo",
    checkoutMode: checkoutMode ?? "current",
    worktreeName: worktreeName ?? "",
    baseBranch: baseBranch ?? "main",
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
    it("renders only the location control — no checkout/branch controls", () => {
      renderRow({ draftTarget: { kind: "home" }, projectPath: null });
      expect(screen.getByText("Home")).toBeInTheDocument();
      expect(screen.queryByText("Current checkout")).toBeNull();
      expect(screen.queryByText("New worktree")).toBeNull();
      expect(screen.queryByText(/^from$/)).toBeNull();
    });
  });

  describe("project target — current checkout", () => {
    it("renders location, checkout, and branch controls", () => {
      renderRow();
      expect(screen.getByText("foo")).toBeInTheDocument();
      expect(screen.getByText("Current checkout")).toBeInTheDocument();
      expect(screen.getByText("main")).toBeInTheDocument();
    });

    it("renders the worktree checkout control when checkoutMode is 'worktree'", () => {
      renderRow({ checkoutMode: "worktree", baseBranch: "develop" });
      expect(screen.getByText("New worktree")).toBeInTheDocument();
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

  describe("location control — type-to-filter", () => {
    /** Three sibling projects, so a query has something to narrow. */
    function seedProjects() {
      currentWorkspaces = [
        makeWs({
          workspace_id: "ws-bar",
          cwd: "/projects/bar",
          project_root: "/projects/bar",
        }),
        makeWs({
          workspace_id: "ws-codemux",
          cwd: "/projects/codemux",
          project_root: "/projects/codemux",
        }),
        makeWs({
          workspace_id: "ws-site",
          cwd: "/projects/codemux-sitev2",
          project_root: "/projects/codemux-sitev2",
        }),
      ];
    }

    async function openPicker() {
      const user = userEvent.setup();
      const rendered = renderRow();
      await user.click(screen.getByText("foo"));
      const input = await screen.findByPlaceholderText("Search projects…");
      return { user, input, ...rendered };
    }

    it("focuses the search input when the popover opens", async () => {
      seedProjects();
      const { input } = await openPicker();
      await waitFor(() => expect(input).toHaveFocus());
    });

    it("narrows the list to fuzzy matches and hides the rest", async () => {
      seedProjects();
      const { user } = await openPicker();
      await user.keyboard("codemux");
      await waitFor(() => expect(screen.queryByText("bar")).toBeNull());
      expect(screen.getByText("codemux")).toBeInTheDocument();
      expect(screen.getByText("codemux-sitev2")).toBeInTheDocument();
      expect(screen.queryByText("Home directory (~)")).toBeNull();
    });

    it("switches to path matching once the query contains a slash", async () => {
      currentWorkspaces = [
        makeWs({
          workspace_id: "ws-a",
          cwd: "/work/alpha/app",
          project_root: "/work/alpha/app",
        }),
        makeWs({
          workspace_id: "ws-b",
          cwd: "/work/beta/app",
          project_root: "/work/beta/app",
        }),
      ];
      const { user, onChangeTarget } = await openPicker();
      await user.keyboard("beta/");
      await user.keyboard("{Enter}");
      expect(onChangeTarget).toHaveBeenCalledWith({
        kind: "project",
        projectPath: "/work/beta/app",
      });
    });

    it("does not let long paths defeat name filtering", async () => {
      currentWorkspaces = [
        makeWs({
          workspace_id: "ws-deep",
          cwd: "/home/user/dev/scratch/bar",
          project_root: "/home/user/dev/scratch/bar",
        }),
        makeWs({
          workspace_id: "ws-vexis",
          cwd: "/projects/vexis",
          project_root: "/projects/vexis",
        }),
      ];
      const { user } = await openPicker();
      // "ve" is a subsequence of `/home/user/dev/…` — name-only
      // matching is what keeps that row out.
      await user.keyboard("ve");
      await waitFor(() => expect(screen.getByText("vexis")).toBeInTheDocument());
      expect(screen.queryByText("bar")).toBeNull();
    });

    it("matches on scattered characters, not just prefixes", async () => {
      seedProjects();
      const { user } = await openPicker();
      await user.keyboard("cdx");
      await waitFor(() => expect(screen.queryByText("bar")).toBeNull());
      expect(screen.getByText("codemux")).toBeInTheDocument();
    });

    it("Enter picks the top match without touching the mouse", async () => {
      seedProjects();
      const { user, onChangeTarget } = await openPicker();
      await user.keyboard("codemux");
      await waitFor(() => expect(screen.queryByText("bar")).toBeNull());
      await user.keyboard("{Enter}");
      expect(onChangeTarget).toHaveBeenCalledWith({
        kind: "project",
        projectPath: "/projects/codemux",
      });
    });

    it("arrow keys move the highlight before Enter commits", async () => {
      seedProjects();
      const { user, onChangeTarget } = await openPicker();
      await user.keyboard("codemux");
      await waitFor(() => expect(screen.queryByText("bar")).toBeNull());
      await user.keyboard("{ArrowDown}{Enter}");
      expect(onChangeTarget).toHaveBeenCalledWith({
        kind: "project",
        projectPath: "/projects/codemux-sitev2",
      });
    });

    it("keeps Home reachable by name", async () => {
      seedProjects();
      const { user, onChangeTarget } = await openPicker();
      await user.keyboard("home");
      await waitFor(() => expect(screen.queryByText("codemux")).toBeNull());
      await user.keyboard("{Enter}");
      expect(onChangeTarget).toHaveBeenCalledWith({ kind: "home" });
    });

    it("shows an empty state but keeps the open-project escape hatch", async () => {
      seedProjects();
      const { user } = await openPicker();
      await user.keyboard("zzzz");
      await waitFor(() =>
        expect(screen.getByText(/No projects match/)).toBeInTheDocument(),
      );
      expect(screen.getByText("Open another project…")).toBeInTheDocument();
    });

    it("resets the query so the next open starts from the full list", async () => {
      seedProjects();
      const { user } = await openPicker();
      await user.keyboard("codemux");
      await waitFor(() => expect(screen.queryByText("bar")).toBeNull());
      await user.keyboard("{Escape}");
      await user.click(screen.getByText("foo"));
      expect(await screen.findByText("bar")).toBeInTheDocument();
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
});
