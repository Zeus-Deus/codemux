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
  gitFetchPrune: vi.fn().mockResolvedValue(undefined),
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
  pasteClipboardImageToFile: vi.fn(),
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
  // Added in step 2b: the new-workspace dialog now embeds the
  // DevicePicker, which reads from hostsList. The submit flow
  // calls setWorkspaceHost when a non-local host is chosen.
  hostsList: vi.fn().mockResolvedValue([]),
  setWorkspaceHost: vi.fn().mockResolvedValue(undefined),
  // Added when the dialog started seeding `baseBranch` from
  // `useDefaultBranch(projectDir)`. The hook calls `getDefaultBranch`
  // unconditionally on every project change — without a mock here, the
  // module-level promise rejects with the Tauri `invoke` bridge being
  // undefined under jsdom and crashes every dialog test.
  getDefaultBranch: vi.fn().mockResolvedValue("main"),
}));

import {
  listBranches,
  checkIsGitRepo,
  gitFetchPrune,
  getGitBranchInfo,
  createWorktreeWorkspace,
  activateWorkspace,
  generateBranchName,
  generateRandomBranchName,
  pasteClipboardImageToFile,
  getDefaultBranch,
} from "@/tauri/commands";
import {
  _defaultBranchCache,
  _defaultBranchInFlight,
} from "@/components/layout/default-branch-cache";

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
    notifications_muted: false,
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
  // useDefaultBranch keeps a module-level cache so the same project_root
  // only fetches once per session. Clear it between tests so a previous
  // case's resolved value doesn't pre-populate the next dialog and
  // short-circuit the auto-adoption effect we want to exercise.
  _defaultBranchCache.clear();
  _defaultBranchInFlight.clear();
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

// ── Fetch before branch listing tests ──

describe("Fetch before branch listing", () => {
  it("calls gitFetchPrune before listing branches", async () => {
    setAppState("/path/to/project");
    (listBranches as Mock).mockResolvedValue(["main"]);

    renderDialog(true);

    await waitFor(() => {
      expect(gitFetchPrune).toHaveBeenCalledWith("/path/to/project");
    });

    // Branches should still be listed after fetch
    await waitFor(() => {
      expect(listBranches).toHaveBeenCalledWith("/path/to/project", false);
      expect(listBranches).toHaveBeenCalledWith("/path/to/project", true);
    });
  });

  it("still lists branches when fetch fails (no network)", async () => {
    setAppState("/path/to/project");
    (gitFetchPrune as Mock).mockRejectedValue(new Error("network error"));
    (listBranches as Mock).mockResolvedValue(["main", "dev"]);

    renderDialog(true);

    // Even though fetch failed, branches should load from local refs
    await waitFor(() => {
      expect(listBranches).toHaveBeenCalledWith("/path/to/project", false);
      expect(listBranches).toHaveBeenCalledWith("/path/to/project", true);
    });
  });

  it("still lists branches when fetch times out", async () => {
    setAppState("/path/to/project");
    (gitFetchPrune as Mock).mockRejectedValue(
      new Error("git fetch timed out after 10 seconds"),
    );
    (listBranches as Mock).mockResolvedValue(["main"]);

    renderDialog(true);

    await waitFor(() => {
      expect(listBranches).toHaveBeenCalledWith("/path/to/project", false);
    });
  });
});

describe("Default base branch", () => {
  it("defaults to main even when repo checkout is on a PR branch", async () => {
    setAppState("/path/to/project");
    (getGitBranchInfo as Mock).mockResolvedValue({
      branch: "feature-pr-branch",
      ahead: 2,
      behind: 0,
    });

    renderDialog(true);

    // Wait for branch data to load
    await waitFor(() => {
      expect(getGitBranchInfo).toHaveBeenCalledWith("/path/to/project");
    });

    // The branch picker button should show "main", not the repo's current branch
    const dialog = await screen.findByRole("dialog");
    const branchPicker = within(dialog).getByText("main");
    expect(branchPicker).toBeInTheDocument();
    expect(within(dialog).queryByText("feature-pr-branch")).not.toBeInTheDocument();
  });

  // Regression: the user reported that opening the dialog on a repo whose
  // default branch is `master` (not `main`) still showed `main` in the pill
  // and then errored out on create. The fix is to seed `baseBranch` from
  // `useDefaultBranch(projectDir)` instead of hardcoding "main". This test
  // verifies the pill adopts whatever the backend detection returns.
  it("seeds the base-branch pill from getDefaultBranch (master, not hardcoded main)", async () => {
    setAppState("/path/to/project");
    (getDefaultBranch as Mock).mockResolvedValue("master");

    renderDialog(true);

    // Wait for the async useDefaultBranch fetch to resolve and the effect
    // to swap "main" → "master" on the pill.
    const dialog = await screen.findByRole("dialog");
    await waitFor(() => {
      expect(within(dialog).getByText("master")).toBeInTheDocument();
    });
    // And the placeholder "main" should be gone — otherwise the effect
    // didn't actually run and the pill is just showing both somehow.
    expect(within(dialog).queryByText("main")).not.toBeInTheDocument();
  });

  // Regression: same fix for any default branch name. Repos can have
  // arbitrary defaults (e.g. `develop`, `trunk`). The hook returns
  // whatever `git symbolic-ref refs/remotes/origin/HEAD` resolves to;
  // the dialog must adopt it verbatim.
  it("seeds the base-branch pill from getDefaultBranch (arbitrary name)", async () => {
    setAppState("/path/to/project");
    (getDefaultBranch as Mock).mockResolvedValue("develop");

    renderDialog(true);

    const dialog = await screen.findByRole("dialog");
    await waitFor(() => {
      expect(within(dialog).getByText("develop")).toBeInTheDocument();
    });
  });

  // Regression: the detected default must not override an explicit user
  // pick. If the user clicks the pill and picks `feature-x`, then the
  // useDefaultBranch fetch resolves later, the picker should NOT clobber
  // their pick back to the detected default.
  it("does not override the pill once the user has manually picked a branch", async () => {
    setAppState("/path/to/project");
    // Make getDefaultBranch resolve slowly so we can pick first
    let resolveDefault!: (v: string) => void;
    (getDefaultBranch as Mock).mockReturnValue(
      new Promise<string>((resolve) => {
        resolveDefault = resolve;
      }),
    );
    (listBranches as Mock).mockResolvedValue(["main", "master", "feature-x"]);
    // Also seed the detailed list so the BranchPicker popover has options
    const { listBranchesDetailed } = await import("@/tauri/commands");
    (listBranchesDetailed as Mock).mockResolvedValue([
      { name: "main", last_commit_unix: 1, is_local: true, is_remote: true },
      { name: "master", last_commit_unix: 2, is_local: true, is_remote: true },
      { name: "feature-x", last_commit_unix: 3, is_local: true, is_remote: false },
    ]);

    renderDialog(true);

    const dialog = await screen.findByRole("dialog");

    // Open the pill and pick `feature-x`
    const pillButton = within(dialog).getByText("main").closest("button")!;
    fireEvent.click(pillButton);
    const featureRow = await screen.findByText("feature-x");
    fireEvent.click(featureRow);

    // Confirm the pill switched to feature-x
    await waitFor(() => {
      expect(within(dialog).getByText("feature-x")).toBeInTheDocument();
    });

    // NOW let the detected default resolve. It must NOT clobber the user's pick.
    resolveDefault("master");
    // Give React a tick to settle any pending effects.
    await new Promise((r) => setTimeout(r, 50));

    expect(within(dialog).getByText("feature-x")).toBeInTheDocument();
    expect(within(dialog).queryByText("master")).not.toBeInTheDocument();
  });
});

// ── Clipboard image paste ─────────────────────────────────────────
//
// The dialog delegates the entire paste flow to a single Rust
// command (`paste_clipboard_image_to_file`) so the image bytes
// never cross the JS IPC boundary. These tests verify the wiring:
// the command returns a path, the path lands as an attachment chip,
// and a clipboard miss (no image) is silent so plain-text paste
// still works through the default browser path.

describe("Clipboard image paste", () => {
  function firePaste(textarea: Element) {
    // The handler ignores clipboardData entirely — the Rust command
    // does the read. We still need to dispatch a real `paste` event
    // so React's synthetic event fires; the payload doesn't matter.
    fireEvent.paste(textarea, {
      clipboardData: {
        items: [],
        files: [],
        types: [],
        getData: () => "",
      },
    });
  }

  it("calls pasteClipboardImageToFile when paste fires", async () => {
    setAppState("/path/to/project");
    (pasteClipboardImageToFile as Mock).mockResolvedValue(
      "/tmp/codemux-clipboard-images/paste-abc.png",
    );

    renderDialog(true);

    const dialog = await screen.findByRole("dialog");
    const textarea = within(dialog).getByPlaceholderText(
      "What do you want to do?",
    );
    firePaste(textarea);

    await waitFor(() => {
      expect(pasteClipboardImageToFile).toHaveBeenCalledTimes(1);
    });
  });

  it("adds the returned path as an attachment chip", async () => {
    setAppState("/path/to/project");
    (pasteClipboardImageToFile as Mock).mockResolvedValue(
      "/tmp/codemux-clipboard-images/paste-xyz.png",
    );

    renderDialog(true);

    const dialog = await screen.findByRole("dialog");
    const textarea = within(dialog).getByPlaceholderText(
      "What do you want to do?",
    );
    firePaste(textarea);

    // The chip renders the trailing filename component (the existing
    // attachment-strip logic lifts it via `.split("/").pop()`).
    await waitFor(() => {
      const chips = within(dialog).getAllByText("paste-xyz.png");
      expect(chips.length).toBeGreaterThan(0);
    });
  });

  it("stays silent when the OS clipboard has no image", async () => {
    // The Rust command rejects when the clipboard does not hold an
    // image. The handler must treat that as "let default paste
    // behaviour run" — no chip, no error toast, no crash.
    setAppState("/path/to/project");
    (pasteClipboardImageToFile as Mock).mockRejectedValue(
      new Error("clipboard read_image failed"),
    );

    renderDialog(true);

    const dialog = await screen.findByRole("dialog");
    const textarea = within(dialog).getByPlaceholderText(
      "What do you want to do?",
    );
    firePaste(textarea);

    await waitFor(() => {
      expect(pasteClipboardImageToFile).toHaveBeenCalled();
    });

    // No chip should appear. Probe by the paste-* filename pattern
    // so the test isn't sensitive to other chip text.
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(within(dialog).queryByText(/paste-.*\.png/)).toBeNull();
  });

  it("does not double-add the same path on repeated paste", async () => {
    // Defensive: if the same image ends up at the same path (e.g.
    // a deterministic temp filename collision), the chip strip must
    // de-dupe so the user doesn't see two identical chips for one
    // pasted image.
    setAppState("/path/to/project");
    (pasteClipboardImageToFile as Mock).mockResolvedValue(
      "/tmp/codemux-clipboard-images/paste-dupe.png",
    );

    renderDialog(true);

    const dialog = await screen.findByRole("dialog");
    const textarea = within(dialog).getByPlaceholderText(
      "What do you want to do?",
    );
    firePaste(textarea);
    await waitFor(() => {
      expect(within(dialog).getAllByText("paste-dupe.png").length).toBe(1);
    });

    firePaste(textarea);
    await waitFor(() => {
      expect(pasteClipboardImageToFile).toHaveBeenCalledTimes(2);
    });
    // Still exactly one chip.
    expect(within(dialog).getAllByText("paste-dupe.png").length).toBe(1);
  });
});
