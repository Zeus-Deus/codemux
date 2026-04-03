/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach, type Mock } from "vitest";
import { render, screen, waitFor, fireEvent, within } from "@testing-library/react";
import { NewWorkspaceDialog } from "./new-workspace-dialog";
import { TooltipProvider } from "@/components/ui/tooltip";
import { useAppStore } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import type { AppStateSnapshot } from "@/tauri/types";

// ── Mock Tauri commands ──

vi.mock("@/tauri/commands", () => ({
  listBranches: vi.fn().mockResolvedValue([]),
  listBranchesDetailed: vi.fn().mockResolvedValue([]),
  checkIsGitRepo: vi.fn().mockResolvedValue(true),
  listWorktrees: vi.fn().mockResolvedValue([]),
  getGitBranchInfo: vi
    .fn()
    .mockResolvedValue({ branch: "main", ahead: 0, behind: 0 }),
  getPresets: vi.fn().mockResolvedValue({
    presets: [
      {
        id: "builtin-claude",
        name: "Claude",
        description: null,
        commands: ["claude --dangerously-skip-permissions"],
        working_directory: null,
        launch_mode: "NewTab",
        icon: null,
        pinned: true,
        is_builtin: true,
        auto_run_on_workspace: false,
        auto_run_on_new_tab: false,
      },
    ],
  }),
  pickFolderDialog: vi.fn().mockResolvedValue(null),
  createWorkspace: vi.fn().mockResolvedValue("ws-new"),
  createWorktreeWorkspace: vi.fn().mockResolvedValue("ws-new"),
  importWorktreeWorkspace: vi.fn().mockResolvedValue("ws-new"),
  activateWorkspace: vi.fn().mockResolvedValue(undefined),
  applyPreset: vi.fn().mockResolvedValue(undefined),
  dbAddRecentProject: vi.fn().mockResolvedValue(undefined),
  dbGetRecentProjects: vi.fn().mockResolvedValue([]),
  generateBranchName: vi.fn().mockResolvedValue("fix-login-bug"),
  generateRandomBranchName: vi.fn().mockResolvedValue("swift-bolt"),
  checkGhAvailable: vi.fn().mockResolvedValue(false),
  checkGithubRepo: vi.fn().mockResolvedValue(false),
  listPullRequests: vi.fn().mockResolvedValue([]),
  pickFilesDialog: vi.fn().mockResolvedValue([]),
  suggestIssueBranchName: vi.fn().mockResolvedValue("feature/92-backend-endpoints"),
  linkWorkspaceIssue: vi.fn().mockResolvedValue(undefined),
  listGithubIssues: vi.fn().mockResolvedValue([]),
  getGithubIssue: vi.fn().mockResolvedValue({
    number: 92, title: "Backend endpoints", state: "Open",
    labels: ["enhancement"], assignees: ["zeus"],
    url: "https://github.com/u/r/issues/92",
    body: "Implement the backend endpoints.",
  }),
  getGithubIssueByPath: vi.fn().mockResolvedValue({
    number: 92, title: "Backend endpoints", state: "Open",
    labels: ["enhancement"], assignees: ["zeus"],
    url: "https://github.com/u/r/issues/92",
    body: "Implement the backend endpoints.",
  }),
}));

import {
  listBranches,
  checkIsGitRepo,
  createWorktreeWorkspace,
  activateWorkspace,
  generateBranchName,
  generateRandomBranchName,
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
    linked_issue: null,
    tabs: [],
    active_tab_id: "",
    active_surface_id: "",
    surfaces: [],
  };
}

function setAppState(cwd: string, extraWorkspaces: WsOverrides[] = []) {
  const primary = makeWs({ workspace_id: "ws-1", cwd, project_root: cwd });
  const extras = extraWorkspaces.map((o, i) =>
    makeWs({ workspace_id: `ws-extra-${i}`, ...o }),
  );
  useAppStore.setState({
    appState: {
      schema_version: 1,
      active_workspace_id: "ws-1",
      workspaces: [primary, ...extras],
      terminal_sessions: [],
      browser_sessions: [],
      agent_browser_sessions: [],
      notifications: [],
      detected_ports: [],
      pane_statuses: {},
      persistence: {
        schema_version: 1,
        stores_layout_metadata: true,
        stores_terminal_metadata: true,
        stores_live_process_state: false,
      },
      config: {} as AppStateSnapshot["config"],
    },
  });
}

function renderDialog(open: boolean, onOpenChange = vi.fn()) {
  return render(
    <TooltipProvider>
      <NewWorkspaceDialog open={open} onOpenChange={onOpenChange} />
    </TooltipProvider>,
  );
}

// ── Tests ──

beforeEach(() => {
  vi.clearAllMocks();
  (checkIsGitRepo as Mock).mockResolvedValue(true);
  (listBranches as Mock).mockResolvedValue([]);
  useUIStore.setState({
    newWorkspaceProjectDir: null,
    pendingWorkspaces: [],
    lastSelectedAgentId: null,
  });
});

describe("NewWorkspaceDialog", () => {
  it("renders prompt textarea as the main element", async () => {
    setAppState("/path/to/project");
    renderDialog(true);

    await waitFor(() => {
      // Radix Dialog renders two copies — check at least one exists
      const els = screen.getAllByPlaceholderText("What do you want to do?");
      expect(els.length).toBeGreaterThan(0);
    });
  });

  it("renders workspace name and branch name inputs", async () => {
    setAppState("/path/to/project");
    renderDialog(true);

    await waitFor(() => {
      expect(
        screen.getAllByPlaceholderText("Workspace name (optional)").length,
      ).toBeGreaterThan(0);
      expect(
        screen.getAllByPlaceholderText("branch name").length,
      ).toBeGreaterThan(0);
    });
  });

  it("renders Create button", async () => {
    setAppState("/path/to/project");
    renderDialog(true);

    await waitFor(() => {
      const buttons = screen.getAllByRole("button", { name: /Create/i });
      expect(buttons.length).toBeGreaterThan(0);
    });
  });

  it("shows Ctrl+Enter hint", async () => {
    setAppState("/path/to/project");
    renderDialog(true);

    await waitFor(() => {
      const hints = screen.getAllByText("Ctrl+Enter to create");
      expect(hints.length).toBeGreaterThan(0);
    });
  });

  it("fetches branches for the project directory", async () => {
    setAppState("/path/to/projectA");
    (listBranches as Mock).mockResolvedValue(["main", "dev"]);

    renderDialog(true);

    await waitFor(() => {
      expect(listBranches).toHaveBeenCalledWith("/path/to/projectA", false);
      expect(listBranches).toHaveBeenCalledWith("/path/to/projectA", true);
    });
  });

  it("re-fetches branches when dialog reopens", async () => {
    setAppState("/path/to/projectA");
    (listBranches as Mock).mockResolvedValue(["main"]);

    const { rerender } = render(
      <TooltipProvider>
        <NewWorkspaceDialog open={true} onOpenChange={vi.fn()} />
      </TooltipProvider>,
    );

    await waitFor(() => {
      expect(listBranches).toHaveBeenCalledTimes(2);
    });

    rerender(
      <TooltipProvider>
        <NewWorkspaceDialog open={false} onOpenChange={vi.fn()} />
      </TooltipProvider>,
    );

    vi.clearAllMocks();
    (checkIsGitRepo as Mock).mockResolvedValue(true);
    (listBranches as Mock).mockResolvedValue(["main", "new-branch"]);

    rerender(
      <TooltipProvider>
        <NewWorkspaceDialog open={true} onOpenChange={vi.fn()} />
      </TooltipProvider>,
    );

    await waitFor(() => {
      expect(listBranches).toHaveBeenCalledTimes(2);
    });
  });
});

describe("Submit flow", () => {
  it("closes dialog immediately on submit (optimistic)", async () => {
    setAppState("/path/to/project");
    const onOpenChange = vi.fn();
    renderDialog(true, onOpenChange);

    await waitFor(() => {
      expect(
        screen.getAllByRole("button", { name: /Create/i }).length,
      ).toBeGreaterThan(0);
    });

    // Click the first Create button (Radix renders duplicates)
    fireEvent.click(screen.getAllByRole("button", { name: /Create/i })[0]);

    await waitFor(() => {
      expect(onOpenChange).toHaveBeenCalledWith(false);
    });
  });

  it("generates random branch name when no prompt or branch provided", async () => {
    setAppState("/path/to/project");
    renderDialog(true);

    await waitFor(() => {
      expect(
        screen.getAllByRole("button", { name: /Create/i }).length,
      ).toBeGreaterThan(0);
    });

    fireEvent.click(screen.getAllByRole("button", { name: /Create/i })[0]);

    await waitFor(() => {
      expect(generateRandomBranchName).toHaveBeenCalledWith("/path/to/project");
    });
  });

  it("generates AI branch name when prompt is provided", async () => {
    setAppState("/path/to/project");
    renderDialog(true);

    // Scope to the real dialog element (not Radix's aria-hidden copy)
    const dialog = await screen.findByRole("dialog");
    const textarea = within(dialog).getByPlaceholderText(
      "What do you want to do?",
    );
    fireEvent.change(textarea, { target: { value: "Fix the login bug" } });

    fireEvent.click(within(dialog).getByRole("button", { name: /Create/i }));

    await waitFor(() => {
      expect(generateBranchName).toHaveBeenCalledWith(
        "Fix the login bug",
        "/path/to/project",
      );
    });
  });

  it("uses explicit branch name when provided", async () => {
    setAppState("/path/to/project");
    renderDialog(true);

    const dialog = await screen.findByRole("dialog");
    const branchInput = within(dialog).getByPlaceholderText("branch name");
    fireEvent.change(branchInput, { target: { value: "my-feature" } });

    fireEvent.click(within(dialog).getByRole("button", { name: /Create/i }));

    await waitFor(() => {
      expect(createWorktreeWorkspace).toHaveBeenCalledWith(
        "/path/to/project",
        "my-feature",
        true,
        "single",
        expect.any(String),
        null,
        "builtin-claude",
      );
    });

    expect(generateBranchName).not.toHaveBeenCalled();
    expect(generateRandomBranchName).not.toHaveBeenCalled();
  });

  it("activates existing workspace when branch already has one", async () => {
    setAppState("/path/to/project", [
      {
        cwd: "/path/to/project/wt",
        git_branch: "fix-login-bug",
        project_root: "/path/to/project",
      },
    ]);
    renderDialog(true);

    const dialog = await screen.findByRole("dialog");
    const textarea = within(dialog).getByPlaceholderText(
      "What do you want to do?",
    );
    fireEvent.change(textarea, {
      target: { value: "Fix the login bug" },
    });

    fireEvent.click(within(dialog).getByRole("button", { name: /Create/i }));

    await waitFor(() => {
      expect(activateWorkspace).toHaveBeenCalledWith("ws-extra-0");
    });

    expect(createWorktreeWorkspace).not.toHaveBeenCalled();
  });
});

describe("Project directory auto-fill", () => {
  it("auto-fills project root from + button context", async () => {
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
            worktree_path:
              "/home/user/.codemux/worktrees/myapp/feature-1",
          }),
        ],
        terminal_sessions: [],
        browser_sessions: [],
        agent_browser_sessions: [],
        notifications: [],
        detected_ports: [],
      pane_statuses: {},
        persistence: {
          schema_version: 1,
          stores_layout_metadata: true,
          stores_terminal_metadata: true,
          stores_live_process_state: false,
        },
        config: {} as AppStateSnapshot["config"],
      },
    });

    useUIStore.setState({ newWorkspaceProjectDir: "/home/user/myapp" });

    renderDialog(true);

    await waitFor(() => {
      expect(checkIsGitRepo).toHaveBeenCalledWith("/home/user/myapp");
    });
  });

  it("falls back to project_root when no + button context", async () => {
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
            worktree_path:
              "/home/user/.codemux/worktrees/myapp/feature-1",
          }),
        ],
        terminal_sessions: [],
        browser_sessions: [],
        agent_browser_sessions: [],
        notifications: [],
        detected_ports: [],
      pane_statuses: {},
        persistence: {
          schema_version: 1,
          stores_layout_metadata: true,
          stores_terminal_metadata: true,
          stores_live_process_state: false,
        },
        config: {} as AppStateSnapshot["config"],
      },
    });

    useUIStore.setState({ newWorkspaceProjectDir: null });

    renderDialog(true);

    await waitFor(() => {
      expect(checkIsGitRepo).toHaveBeenCalledWith("/home/user/myapp");
    });
  });
});

// ── Prompt injection tests ──

import { buildPromptWithIssueContext } from "./new-workspace-dialog";

describe("buildPromptWithIssueContext", () => {
  const issue = { number: 92, title: "Backend endpoints", state: "Open" as const, labels: ["enhancement", "backend"] };

  it("prepends issue context when issue is provided", () => {
    const result = buildPromptWithIssueContext("fix the bug", issue, "Full issue description here.");
    expect(result).toContain("Issue #92: Backend endpoints");
    expect(result).toContain("Status: Open");
    expect(result).toContain("Labels: enhancement, backend");
    expect(result).toContain("Full issue description here.");
    expect(result).toContain("fix the bug");
    // Context comes before user prompt
    expect(result.indexOf("Issue #92")).toBeLessThan(result.indexOf("fix the bug"));
  });

  it("returns raw prompt when no issue", () => {
    const result = buildPromptWithIssueContext("fix the bug", null, null);
    expect(result).toBe("fix the bug");
  });

  it("includes title/number/labels but omits body when body is null", () => {
    const result = buildPromptWithIssueContext("fix it", issue, null);
    expect(result).toContain("Issue #92: Backend endpoints");
    expect(result).toContain("Labels: enhancement, backend");
    expect(result).not.toContain("Description:");
    expect(result).toContain("fix it");
  });

  it("truncates body at 10000 chars with [truncated] marker", () => {
    const longBody = "x".repeat(15000);
    const result = buildPromptWithIssueContext("task", issue, longBody);
    expect(result).toContain("...[truncated]");
    // The body portion should be at most 10000 chars + marker
    expect(result).not.toContain("x".repeat(10001));
  });

  it("omits labels line when labels array is empty", () => {
    const noLabels = { ...issue, labels: [] as string[] };
    const result = buildPromptWithIssueContext("task", noLabels, "body");
    expect(result).not.toContain("Labels:");
  });
});
