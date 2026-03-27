/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach, type Mock } from "vitest";
import { render, screen, waitFor, fireEvent } from "@testing-library/react";
import { NewWorkspaceDialog } from "./new-workspace-dialog";
import { useAppStore } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import type { AppStateSnapshot } from "@/tauri/types";

// ── Mock Tauri commands ──

vi.mock("@/tauri/commands", () => ({
  listBranches: vi.fn().mockResolvedValue([]),
  checkIsGitRepo: vi.fn().mockResolvedValue(true),
  listWorktrees: vi.fn().mockResolvedValue([]),
  getGitBranchInfo: vi.fn().mockResolvedValue({ branch: "main", ahead: 0, behind: 0 }),
  checkGhAvailable: vi.fn().mockResolvedValue(false),
  checkGithubRepo: vi.fn().mockResolvedValue(false),
  listPullRequests: vi.fn().mockResolvedValue([]),
  getPresets: vi.fn().mockResolvedValue({ presets: [] }),
  pickFolderDialog: vi.fn().mockResolvedValue(null),
  createWorkspace: vi.fn().mockResolvedValue("ws-new"),
  createWorktreeWorkspace: vi.fn().mockResolvedValue("ws-new"),
  importWorktreeWorkspace: vi.fn().mockResolvedValue("ws-new"),
  activateWorkspace: vi.fn().mockResolvedValue(undefined),
  applyPreset: vi.fn().mockResolvedValue(undefined),
  dbAddRecentProject: vi.fn().mockResolvedValue(undefined),
  initGitRepo: vi.fn().mockResolvedValue(undefined),
}));

import {
  listBranches,
  checkIsGitRepo,
} from "@/tauri/commands";

// ── Helpers ──

interface WsOverrides {
  workspace_id?: string;
  cwd?: string;
  git_branch?: string;
  project_root?: string | null;
  worktree_path?: string | null;
}

function makeWs(overrides: WsOverrides = {}) {
  return {
    workspace_id: overrides.workspace_id ?? "ws-1",
    title: "Test",
    workspace_type: "standard" as const,
    cwd: overrides.cwd ?? "/path/to/project",
    git_branch: overrides.git_branch ?? "main",
    git_ahead: 0,
    git_behind: 0,
    git_additions: 0,
    git_deletions: 0,
    git_changed_files: 0,
    notification_count: 0,
    latest_agent_state: null,
    worktree_path: overrides.worktree_path ?? null,
    project_root: overrides.project_root ?? null,
    pr_number: null,
    pr_state: null,
    pr_url: null,
    tabs: [],
    active_tab_id: "",
    active_surface_id: "",
    surfaces: [],
  };
}

function setAppState(cwd: string, extraWorkspaces: WsOverrides[] = []) {
  const primary = makeWs({ workspace_id: "ws-1", cwd, project_root: cwd });
  const extras = extraWorkspaces.map((o, i) => makeWs({ workspace_id: `ws-extra-${i}`, ...o }));
  useAppStore.setState({
    appState: {
      schema_version: 1,
      active_workspace_id: "ws-1",
      workspaces: [primary, ...extras],
      terminal_sessions: [],
      browser_sessions: [],
      notifications: [],
      detected_ports: [],
      persistence: { schema_version: 1, stores_layout_metadata: true, stores_terminal_metadata: true, stores_live_process_state: false },
      config: {} as AppStateSnapshot["config"],
    },
  });
}

// Radix Dialog uses portals — provide container
function renderDialog(open: boolean, onOpenChange = vi.fn()) {
  return render(
    <NewWorkspaceDialog open={open} onOpenChange={onOpenChange} />,
  );
}

// ── Tests ──

beforeEach(() => {
  vi.clearAllMocks();
  (checkIsGitRepo as Mock).mockResolvedValue(true);
  (listBranches as Mock).mockResolvedValue([]);
  // Reset store state so tests don't bleed
  useUIStore.setState({ newWorkspaceProjectDir: null });
});

describe("NewWorkspaceDialog branch fetching", () => {
  it("fetches branches for the selected project directory", async () => {
    setAppState("/path/to/projectA");
    (listBranches as Mock).mockResolvedValue(["main", "dev"]);

    renderDialog(true);

    await waitFor(() => {
      expect(listBranches).toHaveBeenCalledWith("/path/to/projectA", false);
      expect(listBranches).toHaveBeenCalledWith("/path/to/projectA", true);
    });

    // Branches should appear in the UI
    await waitFor(() => {
      expect(screen.getByText("main")).toBeInTheDocument();
      expect(screen.getByText("dev")).toBeInTheDocument();
    });
  });

  it("resets state and re-fetches when dialog reopens", async () => {
    setAppState("/path/to/projectA");
    (listBranches as Mock).mockResolvedValue(["main"]);

    const { rerender } = render(
      <NewWorkspaceDialog open={true} onOpenChange={vi.fn()} />,
    );

    await waitFor(() => {
      expect(listBranches).toHaveBeenCalledTimes(2); // local + remote
    });

    // Close dialog
    rerender(
      <NewWorkspaceDialog open={false} onOpenChange={vi.fn()} />,
    );

    vi.clearAllMocks();
    (checkIsGitRepo as Mock).mockResolvedValue(true);
    (listBranches as Mock).mockResolvedValue(["main", "new-branch"]);

    // Reopen dialog
    rerender(
      <NewWorkspaceDialog open={true} onOpenChange={vi.fn()} />,
    );

    // Fresh fetch — not cached
    await waitFor(() => {
      expect(listBranches).toHaveBeenCalledTimes(2);
    });
  });

  it("updates branches when project directory changes", async () => {
    setAppState("/path/to/projectA");
    (listBranches as Mock).mockImplementation((path: string) => {
      if (path === "/path/to/projectA") return Promise.resolve(["alpha"]);
      if (path === "/path/to/projectB") return Promise.resolve(["beta"]);
      return Promise.resolve([]);
    });

    renderDialog(true);

    // Initial fetch for project A
    await waitFor(() => {
      expect(listBranches).toHaveBeenCalledWith("/path/to/projectA", false);
    });
    await waitFor(() => {
      expect(screen.getByText("alpha")).toBeInTheDocument();
    });

    // Change project directory via the input (Radix renders aria-hidden copies, use getAllBy)
    const inputs = screen.getAllByDisplayValue("/path/to/projectA");
    fireEvent.change(inputs[0], { target: { value: "/path/to/projectB" } });

    // New fetch for project B
    await waitFor(() => {
      expect(listBranches).toHaveBeenCalledWith("/path/to/projectB", false);
    });
    await waitFor(() => {
      expect(screen.getByText("beta")).toBeInTheDocument();
    });
  });

  it("does not show stale data from previous dialog session", async () => {
    // Open dialog with project A
    setAppState("/path/to/projectA");
    (listBranches as Mock).mockResolvedValue(["alpha-branch"]);

    const { rerender } = render(
      <NewWorkspaceDialog open={true} onOpenChange={vi.fn()} />,
    );

    await waitFor(() => {
      expect(screen.getByText("alpha-branch")).toBeInTheDocument();
    });

    // Close dialog
    rerender(
      <NewWorkspaceDialog open={false} onOpenChange={vi.fn()} />,
    );

    // Switch to project B
    vi.clearAllMocks();
    (checkIsGitRepo as Mock).mockResolvedValue(true);
    (listBranches as Mock).mockResolvedValue(["beta-branch"]);
    setAppState("/path/to/projectB");

    // Reopen dialog — should fetch for B, not show stale A data
    rerender(
      <NewWorkspaceDialog open={true} onOpenChange={vi.fn()} />,
    );

    // Verify it fetched for the NEW project, not the old one
    await waitFor(() => {
      expect(listBranches).toHaveBeenCalledWith("/path/to/projectB", false);
      expect(listBranches).toHaveBeenCalledWith("/path/to/projectB", true);
    });

    // Should NOT have fetched for the old project
    expect(listBranches).not.toHaveBeenCalledWith("/path/to/projectA", false);

    // New branches should be visible
    await waitFor(() => {
      expect(screen.getByText("beta-branch")).toBeInTheDocument();
    });
    expect(screen.queryByText("alpha-branch")).not.toBeInTheDocument();
  });
});

import { activateWorkspace } from "@/tauri/commands";

describe("Branch collision scoping", () => {
  it("same branch in different projects — Open Workspace does not redirect", async () => {
    // Project A (active) is on "dev". Project B has "feature" open.
    // Selecting "feature" in project A should NOT redirect to project B's workspace.
    useAppStore.setState({
      appState: {
        schema_version: 1,
        active_workspace_id: "ws-1",
        workspaces: [
          makeWs({ workspace_id: "ws-1", cwd: "/path/to/projectA", git_branch: "dev", project_root: "/path/to/projectA" }),
          makeWs({ workspace_id: "ws-b", cwd: "/path/to/projectB", git_branch: "feature", project_root: "/path/to/projectB" }),
        ],
        terminal_sessions: [],
        browser_sessions: [],
        notifications: [],
        detected_ports: [],
        persistence: { schema_version: 1, stores_layout_metadata: true, stores_terminal_metadata: true, stores_live_process_state: false },
        config: {} as AppStateSnapshot["config"],
      },
    });
    (listBranches as Mock).mockResolvedValue(["dev", "feature"]);

    renderDialog(true);

    // Wait for branches to load in the existing tab
    await waitFor(() => {
      expect(screen.getByText("feature")).toBeInTheDocument();
    });

    // Select "feature" branch — it exists in project B but NOT in project A
    fireEvent.click(screen.getByText("feature"));

    // Click the "Open Workspace" button
    const openBtn = await screen.findByRole("button", { name: /Open Workspace/i });
    fireEvent.click(openBtn);

    // Should NOT redirect — different projects can share branch names
    await waitFor(() => {
      expect(activateWorkspace).not.toHaveBeenCalled();
    });
  });

  it("same branch in same project — Open Workspace redirects", async () => {
    // Project A (active) already has "feature-x" open with same project_root
    setAppState("/path/to/projectA", [
      { cwd: "/path/to/projectA/wt", git_branch: "feature-x", project_root: "/path/to/projectA" },
    ]);
    (listBranches as Mock).mockResolvedValue(["main", "feature-x"]);

    renderDialog(true);

    await waitFor(() => {
      expect(screen.getByText("feature-x")).toBeInTheDocument();
    });

    // Select "feature-x" branch
    fireEvent.click(screen.getByText("feature-x"));

    // Click Open Workspace
    const openBtn = await screen.findByRole("button", { name: /Open Workspace/i });
    fireEvent.click(openBtn);

    // Should redirect to existing workspace
    await waitFor(() => {
      expect(activateWorkspace).toHaveBeenCalledWith("ws-extra-0");
    });
  });
});

describe("Project directory auto-fill", () => {
  it("auto-fills project root from + button, not active workspace cwd", async () => {
    // Active workspace is at a worktree path, but + button passes the project root
    useAppStore.setState({
      appState: {
        schema_version: 1,
        active_workspace_id: "ws-wt",
        workspaces: [
          makeWs({
            workspace_id: "ws-wt",
            cwd: "/home/user/.codemux/worktrees/myapp/feature-1",
            git_branch: "feature-1",
            project_root: "/home/user/myapp",
            worktree_path: "/home/user/.codemux/worktrees/myapp/feature-1",
          }),
        ],
        terminal_sessions: [],
        browser_sessions: [],
        notifications: [],
        detected_ports: [],
        persistence: { schema_version: 1, stores_layout_metadata: true, stores_terminal_metadata: true, stores_live_process_state: false },
        config: {} as AppStateSnapshot["config"],
      },
    });

    // Simulate the "+" button passing the project root
    useUIStore.setState({ newWorkspaceProjectDir: "/home/user/myapp" });

    renderDialog(true);

    // The directory input should show the project root, not the worktree path
    await waitFor(() => {
      const inputs = screen.getAllByDisplayValue("/home/user/myapp");
      expect(inputs.length).toBeGreaterThan(0);
    });
    // Should NOT contain the worktree path
    expect(screen.queryByDisplayValue(/\.codemux\/worktrees/)).toBeNull();
  });

  it("falls back to project_root when no + button context", async () => {
    // Active workspace is at a worktree path, no store project dir set
    useAppStore.setState({
      appState: {
        schema_version: 1,
        active_workspace_id: "ws-wt",
        workspaces: [
          makeWs({
            workspace_id: "ws-wt",
            cwd: "/home/user/.codemux/worktrees/myapp/feature-1",
            git_branch: "feature-1",
            project_root: "/home/user/myapp",
            worktree_path: "/home/user/.codemux/worktrees/myapp/feature-1",
          }),
        ],
        terminal_sessions: [],
        browser_sessions: [],
        notifications: [],
        detected_ports: [],
        persistence: { schema_version: 1, stores_layout_metadata: true, stores_terminal_metadata: true, stores_live_process_state: false },
        config: {} as AppStateSnapshot["config"],
      },
    });

    // No store project dir — should fall back to project_root
    useUIStore.setState({ newWorkspaceProjectDir: null });

    renderDialog(true);

    // Should show project root, not the worktree cwd
    await waitFor(() => {
      const inputs = screen.getAllByDisplayValue("/home/user/myapp");
      expect(inputs.length).toBeGreaterThan(0);
    });
  });
});
