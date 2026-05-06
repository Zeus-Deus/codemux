/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, act, cleanup, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

// Polyfill ResizeObserver for ScrollArea (not available in jsdom)
globalThis.ResizeObserver = class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

// ── Mock Tauri commands ──

const mockGetGitStatus = vi.fn().mockResolvedValue([]);
const mockGetGitBranchInfo = vi.fn().mockResolvedValue({
  branch: "feat/my-feature",
  ahead: 0,
  behind: 0,
  has_upstream: true,
});
const mockGitLogEntries = vi.fn().mockResolvedValue([]);
const mockGetMergeState = vi.fn().mockResolvedValue({
  is_merging: false,
  is_rebasing: false,
  merge_head: null,
  conflicted_files: [],
});
const mockMergeBranch = vi.fn().mockResolvedValue("merged");
const mockGetBaseBranchDiff = vi.fn().mockResolvedValue({
  files: [
    { path: "src/index.ts", status: "modified", is_staged: false, is_unstaged: true, additions: 5, deletions: 2 },
  ],
  merge_base_commit: "abc123",
});
const mockGetDefaultBranch = vi.fn().mockResolvedValue("main");
const mockListBranches = vi.fn().mockResolvedValue(["main", "develop"]);
const mockCheckClaudeAvailable = vi.fn().mockResolvedValue(false);
const mockContinueMerge = vi.fn().mockResolvedValue(undefined);
const mockAbortMerge = vi.fn().mockResolvedValue(undefined);
const mockMergeIntoBase = vi.fn().mockResolvedValue({ status: "merged", source_branch: "feat/my-feature", temp_branch: null, conflicted_files: [] });
const mockCompleteMergeIntoBase = vi.fn().mockResolvedValue(undefined);
const mockAbortMergeIntoBase = vi.fn().mockResolvedValue(undefined);
const mockActivateWorkspace = vi.fn().mockResolvedValue(undefined);
const mockCloseWorkspace = vi.fn().mockResolvedValue("ws-1");
const mockCreateWorkspace = vi.fn().mockResolvedValue("ws-new");
const mockMergePullRequest = vi.fn().mockResolvedValue(undefined);
const mockRefreshWorkspacePr = vi.fn().mockResolvedValue(undefined);
const mockOpenUrl = vi.fn().mockResolvedValue(undefined);
const mockSetAiResolverStrategy = vi.fn().mockResolvedValue(undefined);
const mockSetShowSettings = vi.fn();
const mockStartResolution = vi.fn();

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: (...args: unknown[]) => mockOpenUrl(...args),
}));

vi.mock("@/tauri/commands", () => ({
  getGitStatus: (...args: unknown[]) => mockGetGitStatus(...args),
  getGitDiff: vi.fn().mockResolvedValue(""),
  getGitBranchInfo: (...args: unknown[]) => mockGetGitBranchInfo(...args),
  gitStageFiles: vi.fn().mockResolvedValue(undefined),
  gitUnstageFiles: vi.fn().mockResolvedValue(undefined),
  gitCommitChanges: vi.fn().mockResolvedValue(undefined),
  gitPushChanges: vi.fn().mockResolvedValue(undefined),
  gitPullChanges: vi.fn().mockResolvedValue(undefined),
  gitDiscardFile: vi.fn().mockResolvedValue(undefined),
  gitLogEntries: (...args: unknown[]) => mockGitLogEntries(...args),
  getMergeState: (...args: unknown[]) => mockGetMergeState(...args),
  mergeBranch: (...args: unknown[]) => mockMergeBranch(...args),
  resolveConflictOurs: vi.fn().mockResolvedValue(undefined),
  resolveConflictTheirs: vi.fn().mockResolvedValue(undefined),
  markConflictResolved: vi.fn().mockResolvedValue(undefined),
  abortMerge: (...args: unknown[]) => mockAbortMerge(...args),
  continueMerge: (...args: unknown[]) => mockContinueMerge(...args),
  createTab: vi.fn().mockResolvedValue("tab-1"),
  activateTab: vi.fn().mockResolvedValue(undefined),
  checkClaudeAvailable: (...args: unknown[]) => mockCheckClaudeAvailable(...args),
  getBaseBranchDiff: (...args: unknown[]) => mockGetBaseBranchDiff(...args),
  getDefaultBranch: (...args: unknown[]) => mockGetDefaultBranch(...args),
  listBranches: (...args: unknown[]) => mockListBranches(...args),
  mergeIntoBase: (...args: unknown[]) => mockMergeIntoBase(...args),
  completeMergeIntoBase: (...args: unknown[]) => mockCompleteMergeIntoBase(...args),
  abortMergeIntoBase: (...args: unknown[]) => mockAbortMergeIntoBase(...args),
  activateWorkspace: (...args: unknown[]) => mockActivateWorkspace(...args),
  closeWorkspace: (...args: unknown[]) => mockCloseWorkspace(...args),
  createWorkspace: (...args: unknown[]) => mockCreateWorkspace(...args),
  gitFetchChanges: vi.fn().mockResolvedValue(undefined),
  gitStashPush: vi.fn().mockResolvedValue(undefined),
  gitStashPop: vi.fn().mockResolvedValue(undefined),
  getCommitFiles: vi.fn().mockResolvedValue([]),
  mergePullRequest: (...args: unknown[]) => mockMergePullRequest(...args),
  refreshWorkspacePr: (...args: unknown[]) => mockRefreshWorkspacePr(...args),
  setAiResolverStrategy: (...args: unknown[]) => mockSetAiResolverStrategy(...args),
}));

vi.mock("@/stores/ui-store", () => ({
  useUIStore: vi.fn((selector: (s: Record<string, unknown>) => unknown) => {
    const state = {
      setShowSettings: mockSetShowSettings,
      showSettings: false,
      settingsSection: null,
    };
    return selector(state);
  }),
}));

vi.mock("@/stores/diff-store", () => ({
  useDiffStore: vi.fn((selector: (s: Record<string, unknown>) => unknown) => {
    const state = {
      setFile: vi.fn(),
      setBaseBranch: vi.fn(),
      setSection: vi.fn(),
    };
    return selector(state);
  }),
}));

let mockAppStoreState: Record<string, unknown> = {
  appState: {
    config: { ai_commit_message_enabled: false },
    workspaces: [],
  },
};

vi.mock("@/stores/app-store", () => ({
  useAppStore: vi.fn((selector: (s: Record<string, unknown>) => unknown) => {
    return selector(mockAppStoreState);
  }),
}));

vi.mock("@/lib/toast", () => ({
  toast: { success: vi.fn(), error: vi.fn(), info: vi.fn(), warning: vi.fn() },
}));

vi.mock("@/stores/ai-commit-store", () => ({
  useAiCommitStore: vi.fn((selector: (s: Record<string, unknown>) => unknown) => {
    const state = {
      getGeneration: () => null,
      requestGeneration: vi.fn(),
      consumeMessage: vi.fn(),
      clearGeneration: vi.fn(),
    };
    return selector(state);
  }),
}));

vi.mock("@/stores/ai-merge-store", () => ({
  useAiMergeStore: vi.fn((selector: (s: Record<string, unknown>) => unknown) => {
    const state = {
      getResolver: () => ({ status: "idle", tempBranch: null, originalBranch: null, targetBranch: null, conflictingFiles: [], agentOutput: null, resolutionDiff: null, error: null }),
      startResolution: mockStartResolution,
      approveResolution: vi.fn().mockResolvedValue(undefined),
      rejectResolution: vi.fn().mockResolvedValue(undefined),
    };
    return selector(state);
  }),
}));

import { TooltipProvider } from "@/components/ui/tooltip";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ChangesPanel } from "./changes-panel";
import { toast as mockToast } from "@/lib/toast";
import type { WorkspaceSnapshot } from "@/tauri/types";

function flushPromises() {
  return act(() => new Promise((r) => setTimeout(r, 0)));
}

const mockWorkspace: WorkspaceSnapshot = {
  workspace_id: "ws-1",
  title: "Test",
  workspace_type: "standard",
  cwd: "/home/user/project",
  git_branch: "feat/my-feature",
  git_ahead: 0,
  git_behind: 0,
  git_additions: 5,
  git_deletions: 2,
  git_changed_files: 1,
  notification_count: 0,
  latest_agent_state: null,
  worktree_path: null,
  project_root: "/home/user/project",
  pr_number: null,
  pr_state: null,
  pr_url: null,
  linked_issue: null,
  tabs: [],
  active_tab_id: "tab-1",
  active_surface_id: "surface-1",
  surfaces: [],
};

// Each test gets a fresh QueryClient — required since ChangesPanel
// now calls `useQueryClient` (Phase 3.3) to invalidate Review-tab
// queries on commit / push / pull / merge. Disable retries + auto
// refetch so query state doesn't leak between tests.
function makeTestQueryClient() {
  return new QueryClient({
    defaultOptions: {
      queries: { retry: false, staleTime: Infinity, refetchInterval: false },
    },
  });
}

function renderPanel() {
  return render(
    <QueryClientProvider client={makeTestQueryClient()}>
      <TooltipProvider>
        <ChangesPanel workspace={mockWorkspace} />
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
  mockGetGitStatus.mockResolvedValue([]);
  mockGetGitBranchInfo.mockResolvedValue({
    branch: "feat/my-feature",
    ahead: 0,
    behind: 0,
    has_upstream: true,
  });
  mockGitLogEntries.mockResolvedValue([]);
  mockGetMergeState.mockResolvedValue({
    is_merging: false,
    is_rebasing: false,
    merge_head: null,
    conflicted_files: [],
  });
  mockMergeBranch.mockResolvedValue("merged");
  mockGetBaseBranchDiff.mockResolvedValue({
    files: [
      { path: "src/index.ts", status: "modified", is_staged: false, is_unstaged: true, additions: 5, deletions: 2 },
    ],
    merge_base_commit: "abc123",
  });
  mockGetDefaultBranch.mockResolvedValue("main");
  mockListBranches.mockResolvedValue(["main", "develop"]);
  mockCheckClaudeAvailable.mockResolvedValue(false);
  mockMergePullRequest.mockResolvedValue(undefined);
  mockRefreshWorkspacePr.mockResolvedValue(undefined);
  mockOpenUrl.mockResolvedValue(undefined);
  mockAppStoreState = {
    appState: {
      config: { ai_commit_message_enabled: false },
      workspaces: [],
    },
  };
});

describe("Merge button in Against section", () => {
  it("renders merge button when base branch files exist", async () => {
    renderPanel();
    await flushPromises();
    // The merge button should have a tooltip "Merge main into current branch"
    // Look for the GitMerge icon button in the Against section
    // At minimum, the "Against" section should be visible with a merge button
    expect(screen.getByText("Against main")).toBeInTheDocument();
  });

  it("calls mergeBranch with correct args on click", async () => {
    const user = userEvent.setup();
    renderPanel();
    await flushPromises();

    // Find the merge button — it's in the Against section near the file count
    // The Against section renders when baseBranchFiles.length > 0
    const againstSection = screen.getByText("Against main").closest("div")?.parentElement;
    expect(againstSection).toBeTruthy();

    // Click the merge button — find it by proximity to the file count
    // The merge button is the last button-like element in the section header
    const headerDiv = screen.getByText("Against main").closest(".flex");
    const allMergeButtons = headerDiv?.parentElement?.querySelectorAll("button") ?? [];
    // The merge button is the one that's not the expand/collapse chevron and not the branch selector
    for (const btn of allMergeButtons) {
      if (btn.querySelector(".lucide-git-merge")) {
        await user.click(btn);
        break;
      }
    }

    await waitFor(() => {
      expect(mockMergeBranch).toHaveBeenCalledWith("/home/user/project", "main");
    });
  });

  it("clean merge refreshes status and base diff", async () => {
    const user = userEvent.setup();
    mockMergeBranch.mockResolvedValue("merged"); // no conflicts

    renderPanel();
    await flushPromises();

    // Click merge
    const headerDiv = screen.getByText("Against main").closest(".flex")?.parentElement;
    const allButtons = headerDiv?.querySelectorAll("button") ?? [];
    for (const btn of allButtons) {
      if (btn.querySelector(".lucide-git-merge")) {
        await user.click(btn);
        break;
      }
    }

    await waitFor(() => {
      expect(mockMergeBranch).toHaveBeenCalled();
    });

    // After clean merge, refresh should be called (getGitStatus is called again)
    await waitFor(() => {
      // Initial load + refresh after merge
      expect(mockGetGitStatus.mock.calls.length).toBeGreaterThanOrEqual(2);
    });
  });

  it("merge with conflicts shows merge banner", async () => {
    const user = userEvent.setup();
    mockMergeBranch.mockResolvedValue("conflicts"); // has conflicts

    // After merge, getMergeState will return merging state
    let callCount = 0;
    mockGetMergeState.mockImplementation(() => {
      callCount++;
      if (callCount > 1) {
        return Promise.resolve({
          is_merging: true,
          is_rebasing: false,
          merge_head: "def456",
          conflicted_files: [{ path: "src/index.ts", conflict_type: "both_modified" }],
        });
      }
      return Promise.resolve({
        is_merging: false,
        is_rebasing: false,
        merge_head: null,
        conflicted_files: [],
      });
    });
    mockGetGitStatus.mockImplementation(() => {
      if (callCount > 1) {
        return Promise.resolve([
          { path: "src/index.ts", status: "conflicted", is_staged: false, is_unstaged: true, additions: 0, deletions: 0, conflict_type: "both_modified" },
        ]);
      }
      return Promise.resolve([]);
    });

    renderPanel();
    await flushPromises();

    // Click merge
    const headerDiv = screen.getByText("Against main").closest(".flex")?.parentElement;
    const allButtons = headerDiv?.querySelectorAll("button") ?? [];
    for (const btn of allButtons) {
      if (btn.querySelector(".lucide-git-merge")) {
        await user.click(btn);
        break;
      }
    }

    await waitFor(() => {
      expect(mockMergeBranch).toHaveBeenCalled();
    });

    // After merge with conflicts, the merge banner should appear
    await waitFor(() => {
      expect(screen.getByText(/Merge in progress/)).toBeInTheDocument();
    });
  });

  it("does not show merge button when no base branch files", async () => {
    mockGetBaseBranchDiff.mockResolvedValue({ files: [], merge_base_commit: "abc123" });

    renderPanel();
    await flushPromises();

    // "Against" section should not render
    expect(screen.queryByText("Against main")).not.toBeInTheDocument();
  });

  it("shows error when merge fails (e.g., dirty tree)", async () => {
    const user = userEvent.setup();
    mockMergeBranch.mockRejectedValue("Cannot merge: working tree has uncommitted changes.");

    renderPanel();
    await flushPromises();

    // Click merge
    const headerDiv = screen.getByText("Against main").closest(".flex")?.parentElement;
    const allButtons = headerDiv?.querySelectorAll("button") ?? [];
    for (const btn of allButtons) {
      if (btn.querySelector(".lucide-git-merge")) {
        await user.click(btn);
        break;
      }
    }

    await waitFor(() => {
      expect(screen.getByText(/uncommitted changes/)).toBeInTheDocument();
    });
  });
});

describe("Abort merge flow", () => {
  it("abort merge button calls abortMerge and refreshes", async () => {
    const user = userEvent.setup();

    // Start in merging state
    mockGetMergeState.mockResolvedValue({
      is_merging: true,
      is_rebasing: false,
      merge_head: "def456",
      conflicted_files: [{ path: "src/index.ts", conflict_type: "both_modified" }],
    });
    mockGetGitStatus.mockResolvedValue([
      { path: "src/index.ts", status: "conflicted", is_staged: false, is_unstaged: true, additions: 0, deletions: 0, conflict_type: "both_modified" },
    ]);

    renderPanel();
    await flushPromises();

    // Merge banner should be visible
    expect(screen.getByText(/Merge in progress/)).toBeInTheDocument();

    // Click abort
    await user.click(screen.getByText("Abort"));

    await waitFor(() => {
      expect(mockAbortMerge).toHaveBeenCalledWith("/home/user/project");
    });
  });
});

describe("Complete merge after conflict resolution", () => {
  it("shows Complete Merge when merging with no remaining conflicts", async () => {
    mockGetMergeState.mockResolvedValue({
      is_merging: true,
      is_rebasing: false,
      merge_head: "def456",
      conflicted_files: [],
    });
    // No conflicted files in status
    mockGetGitStatus.mockResolvedValue([
      { path: "src/index.ts", status: "modified", is_staged: true, is_unstaged: false, additions: 5, deletions: 2 },
    ]);

    renderPanel();
    await flushPromises();

    expect(screen.getByText("Complete Merge")).toBeInTheDocument();
  });
});

describe("Against section and merge button visibility on base branch", () => {
  it("hides entire Against section when current branch equals base branch", async () => {
    // Set branchInfo to main (same as baseBranch)
    mockGetGitBranchInfo.mockResolvedValue({
      branch: "main",
      ahead: 0,
      behind: 0,
      has_upstream: true,
    });

    renderPanel();
    await flushPromises();

    // The Against section should NOT render when on the base branch
    expect(screen.queryByText("Against main")).not.toBeInTheDocument();
  });

  it("shows Against section with both merge buttons when on a feature branch", async () => {
    // branchInfo is "feat/my-feature" (default mock), baseBranch is "main"
    renderPanel();
    await flushPromises();

    expect(screen.getByText("Against main")).toBeInTheDocument();
  });
});

// ── Post-merge workspace cleanup tests ──

const mainWorkspace = {
  workspace_id: "ws-main",
  title: "Main",
  workspace_type: "standard" as const,
  cwd: "/home/user/project",
  git_branch: "main",
  git_ahead: 0,
  git_behind: 0,
  git_additions: 0,
  git_deletions: 0,
  git_changed_files: 0,
  notification_count: 0,
  latest_agent_state: null,
  worktree_path: null,
  project_root: "/home/user/project",
  pr_number: null,
  pr_state: null,
  pr_url: null,
  linked_issue: null,
  tabs: [],
  active_tab_id: "tab-1",
  active_surface_id: "surface-1",
  surfaces: [],
};

/** Open the "Merge into base" dialog, check "Delete branch", and confirm. */
async function triggerMergeIntoBaseWithDelete(user: ReturnType<typeof userEvent.setup>) {
  // Find the ArrowUpToLine button (merge-into icon) in the Against section
  const mergeIntoBtn = document.querySelector(".lucide-arrow-up-to-line")?.closest("button");
  expect(mergeIntoBtn).toBeTruthy();
  await user.click(mergeIntoBtn!);

  // Dialog should appear — check the "Delete branch" checkbox
  const checkbox = await screen.findByRole("checkbox");
  await user.click(checkbox);

  // Click the confirm button in the dialog
  const confirmBtn = screen.getByRole("button", { name: /Merge into main/ });
  await user.click(confirmBtn);
}

/** Open the "Merge into base" dialog and confirm WITHOUT checking delete. */
async function triggerMergeIntoBaseNoDelete(user: ReturnType<typeof userEvent.setup>) {
  const mergeIntoBtn = document.querySelector(".lucide-arrow-up-to-line")?.closest("button");
  expect(mergeIntoBtn).toBeTruthy();
  await user.click(mergeIntoBtn!);

  // Do NOT check the delete checkbox — just confirm
  const confirmBtn = await screen.findByRole("button", { name: /Merge into main/ });
  await user.click(confirmBtn);
}

describe("Post-merge workspace cleanup", () => {
  it("after merge+delete, activates main workspace if exists", async () => {
    const user = userEvent.setup();

    mockAppStoreState = {
      appState: {
        config: { ai_commit_message_enabled: false },
        workspaces: [mockWorkspace, mainWorkspace],
      },
    };

    mockMergeIntoBase.mockResolvedValue({
      status: "merged",
      source_branch: "feat/my-feature",
      temp_branch: null,
      conflicted_files: [],
    });

    renderPanel();
    await flushPromises();
    await triggerMergeIntoBaseWithDelete(user);

    await waitFor(() => {
      expect(mockMergeIntoBase).toHaveBeenCalledWith("/home/user/project", "main", true);
    });

    await waitFor(() => {
      expect(mockActivateWorkspace).toHaveBeenCalledWith("ws-main");
    });

    await waitFor(() => {
      expect(mockCloseWorkspace).toHaveBeenCalledWith("ws-1", false);
    });

    expect(mockToast.success).toHaveBeenCalledWith(
      expect.stringContaining("Merged into main"),
    );
  });

  it("after merge+delete, creates main workspace if none exists", async () => {
    const user = userEvent.setup();

    // Only the feature workspace exists — no main workspace
    mockAppStoreState = {
      appState: {
        config: { ai_commit_message_enabled: false },
        workspaces: [mockWorkspace],
      },
    };

    mockMergeIntoBase.mockResolvedValue({
      status: "merged",
      source_branch: "feat/my-feature",
      temp_branch: null,
      conflicted_files: [],
    });
    mockCreateWorkspace.mockResolvedValue("ws-created");

    renderPanel();
    await flushPromises();
    await triggerMergeIntoBaseWithDelete(user);

    await waitFor(() => {
      expect(mockCreateWorkspace).toHaveBeenCalledWith("/home/user/project");
    });

    await waitFor(() => {
      expect(mockActivateWorkspace).toHaveBeenCalledWith("ws-created");
    });
  });

  it("after merge without delete, stays on feature workspace", async () => {
    const user = userEvent.setup();

    mockAppStoreState = {
      appState: {
        config: { ai_commit_message_enabled: false },
        workspaces: [mockWorkspace, mainWorkspace],
      },
    };

    mockMergeIntoBase.mockResolvedValue({
      status: "merged",
      source_branch: "feat/my-feature",
      temp_branch: null,
      conflicted_files: [],
    });

    renderPanel();
    await flushPromises();
    await triggerMergeIntoBaseNoDelete(user);

    await waitFor(() => {
      expect(mockMergeIntoBase).toHaveBeenCalledWith("/home/user/project", "main", false);
    });

    // Should NOT activate or close any workspace
    expect(mockActivateWorkspace).not.toHaveBeenCalled();
    expect(mockCloseWorkspace).not.toHaveBeenCalled();
  });

  it("after merge+delete, handles createWorkspace failure gracefully", async () => {
    const user = userEvent.setup();

    // No main workspace — force createWorkspace path
    mockAppStoreState = {
      appState: {
        config: { ai_commit_message_enabled: false },
        workspaces: [mockWorkspace],
      },
    };

    mockMergeIntoBase.mockResolvedValue({
      status: "merged",
      source_branch: "feat/my-feature",
      temp_branch: null,
      conflicted_files: [],
    });
    mockCreateWorkspace.mockRejectedValue("disk full");

    renderPanel();
    await flushPromises();
    await triggerMergeIntoBaseWithDelete(user);

    await waitFor(() => {
      expect(mockToast.error).toHaveBeenCalledWith(
        expect.stringContaining("Could not create workspace"),
      );
    });

    // Feature workspace should NOT be closed (fallback to detached HEAD)
    expect(mockCloseWorkspace).not.toHaveBeenCalled();
    expect(mockActivateWorkspace).not.toHaveBeenCalled();
  });

  it("after conflict resolution+delete, same cleanup flow", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });

    mockAppStoreState = {
      appState: {
        config: { ai_commit_message_enabled: false },
        workspaces: [mockWorkspace, mainWorkspace],
      },
    };

    // Merge returns conflicts
    mockMergeIntoBase.mockResolvedValue({
      status: "conflicts",
      source_branch: "feat/my-feature",
      temp_branch: "merge/feat-my-feature-into-main-123",
      conflicted_files: [{ path: "shared.txt", conflict_type: "both_modified" }],
    });

    // Start with is_merging: false so the button is enabled,
    // then switch to true after merge is triggered (simulates temp branch merge state)
    let mergeTriggered = false;
    mockGetMergeState.mockImplementation(() => {
      if (!mergeTriggered) {
        return Promise.resolve({ is_merging: false, is_rebasing: false, merge_head: null, conflicted_files: [] });
      }
      return Promise.resolve({ is_merging: true, is_rebasing: false, merge_head: "abc", conflicted_files: [] });
    });

    // No conflicted files in working tree (conflicts already resolved)
    mockGetGitStatus.mockResolvedValue([]);

    renderPanel();
    await flushPromises();
    await triggerMergeIntoBaseWithDelete(user);

    await waitFor(() => {
      expect(mockMergeIntoBase).toHaveBeenCalled();
    });

    // Now merge is triggered — getMergeState should return is_merging: true
    mergeTriggered = true;

    // Advance timer to trigger poll cycle → re-fetch merge state → isMerging becomes true
    await act(async () => { vi.advanceTimersByTime(11000); });
    await flushPromises();

    // Banner: isMerging + mergeIntoBaseState + no conflicts → "Complete Merge" button
    const completeBtn = await screen.findByText(/Complete Merge into main/);
    await user.click(completeBtn);

    await waitFor(() => {
      expect(mockCompleteMergeIntoBase).toHaveBeenCalledWith(
        "/home/user/project",
        "main",
        "merge/feat-my-feature-into-main-123",
        "feat/my-feature",
        true,
      );
    });

    await waitFor(() => {
      expect(mockActivateWorkspace).toHaveBeenCalledWith("ws-main");
    });

    await waitFor(() => {
      expect(mockCloseWorkspace).toHaveBeenCalledWith("ws-1", false);
    });

    vi.useRealTimers();
  });

  it("after merge+delete, selects main workspace from same project only", async () => {
    const user = userEvent.setup();

    // Project A main workspace — different project, also on "main"
    const projectAMainWs = {
      ...mainWorkspace,
      workspace_id: "ws-projectA-main",
      title: "Project A Main",
      cwd: "/home/user/projectA",
      project_root: "/home/user/projectA",
      git_branch: "main",
    };

    // Project B main workspace — same project as current feature workspace
    const projectBMainWs = {
      ...mainWorkspace,
      workspace_id: "ws-projectB-main",
      title: "Project B Main",
      cwd: "/home/user/project",
      project_root: "/home/user/project",
      git_branch: "main",
    };

    // Current feature workspace is in project B
    mockAppStoreState = {
      appState: {
        config: { ai_commit_message_enabled: false },
        // Project A's main appears first — must NOT be selected
        workspaces: [projectAMainWs, mockWorkspace, projectBMainWs],
      },
    };

    mockMergeIntoBase.mockResolvedValue({
      status: "merged",
      source_branch: "feat/my-feature",
      temp_branch: null,
      conflicted_files: [],
    });

    renderPanel();
    await flushPromises();
    await triggerMergeIntoBaseWithDelete(user);

    await waitFor(() => {
      expect(mockMergeIntoBase).toHaveBeenCalledWith("/home/user/project", "main", true);
    });

    await waitFor(() => {
      // Must activate project B's main workspace, NOT project A's
      expect(mockActivateWorkspace).toHaveBeenCalledWith("ws-projectB-main");
    });

    // Project A's workspace must NOT have been activated
    expect(mockActivateWorkspace).not.toHaveBeenCalledWith("ws-projectA-main");
  });
});

// ── Phase 2: PR split-button (open-on-GitHub + merge dropdown) ──

describe("Changes panel PR split-button", () => {
  function renderWithPr(
    overrides: Partial<WorkspaceSnapshot> = { pr_number: 42, pr_state: "OPEN", pr_url: "https://github.com/o/r/pull/42" },
  ) {
    return render(
      <QueryClientProvider client={makeTestQueryClient()}>
        <TooltipProvider>
          <ChangesPanel workspace={{ ...mockWorkspace, ...overrides }} />
        </TooltipProvider>
      </QueryClientProvider>,
    );
  }

  it("renders only the open-on-GitHub button (no merge chevron) for a MERGED PR", async () => {
    renderWithPr({ pr_number: 7, pr_state: "MERGED", pr_url: "https://github.com/o/r/pull/7" });
    await flushPromises();

    expect(screen.getByText("#7")).toBeInTheDocument();
    // The merge-PR chevron only renders for OPEN PRs.
    expect(screen.queryByLabelText("Merge PR")).not.toBeInTheDocument();
  });

  it("renders chevron and dispatches mergePullRequest with the right method per item", async () => {
    const user = userEvent.setup();
    renderWithPr({ pr_number: 42, pr_state: "OPEN", pr_url: "https://github.com/o/r/pull/42" });
    await flushPromises();

    const chevron = screen.getByLabelText("Merge PR");
    expect(chevron).toBeInTheDocument();

    // Squash
    await user.click(chevron);
    await user.click(await screen.findByText("Squash and merge"));
    await waitFor(() => {
      expect(mockMergePullRequest).toHaveBeenLastCalledWith("/home/user/project", 42, "squash");
    });

    // Merge commit
    await user.click(screen.getByLabelText("Merge PR"));
    await user.click(await screen.findByText("Create merge commit"));
    await waitFor(() => {
      expect(mockMergePullRequest).toHaveBeenLastCalledWith("/home/user/project", 42, "merge");
    });

    // Rebase
    await user.click(screen.getByLabelText("Merge PR"));
    await user.click(await screen.findByText("Rebase and merge"));
    await waitFor(() => {
      expect(mockMergePullRequest).toHaveBeenLastCalledWith("/home/user/project", 42, "rebase");
    });
  });

  it("clicking the main button area opens GitHub and does NOT trigger mergePullRequest", async () => {
    const user = userEvent.setup();
    renderWithPr({ pr_number: 42, pr_state: "OPEN", pr_url: "https://github.com/o/r/pull/42" });
    await flushPromises();

    await user.click(screen.getByText("#42"));

    expect(mockOpenUrl).toHaveBeenCalledWith("https://github.com/o/r/pull/42");
    expect(mockMergePullRequest).not.toHaveBeenCalled();
  });

  it("on success: fires success toast and triggers refreshWorkspacePr", async () => {
    const user = userEvent.setup();
    mockMergePullRequest.mockResolvedValueOnce(undefined);
    renderWithPr({ pr_number: 42, pr_state: "OPEN", pr_url: "https://github.com/o/r/pull/42" });
    await flushPromises();

    await user.click(screen.getByLabelText("Merge PR"));
    await user.click(await screen.findByText("Squash and merge"));

    await waitFor(() => {
      expect(mockToast.success).toHaveBeenCalledWith("PR #42 squashed");
    });
    expect(mockRefreshWorkspacePr).toHaveBeenCalledWith("ws-1");
  });

  it("on failure: fires error toast and does NOT trigger refreshWorkspacePr", async () => {
    const user = userEvent.setup();
    mockMergePullRequest.mockRejectedValueOnce(new Error("merge conflict"));
    renderWithPr({ pr_number: 42, pr_state: "OPEN", pr_url: "https://github.com/o/r/pull/42" });
    await flushPromises();

    await user.click(screen.getByLabelText("Merge PR"));
    await user.click(await screen.findByText("Squash and merge"));

    await waitFor(() => {
      expect(mockToast.error).toHaveBeenCalledWith("Couldn't merge PR: merge conflict");
    });
    expect(mockRefreshWorkspacePr).not.toHaveBeenCalled();
  });
});

// ── Labeled "Merge → base" CTA (worktree exit) ──
//
// This is the primary affordance for shipping a worktree's work onto the
// base branch. Before this redesign the same action was a 24px GitMerge
// icon hidden in a toolbar between refresh and view-mode toggles, which
// gave new users no signal that "this is where the workflow exits."

describe("Labeled merge-to-base CTA", () => {
  it("renders the labeled CTA on a feature branch with work vs base", async () => {
    renderPanel();
    await flushPromises();

    // The CTA is identified by its accessible name, not a glyph — that's
    // the whole point of the redesign.
    expect(
      screen.getByRole("button", { name: /Merge into main/i }),
    ).toBeInTheDocument();
    // Micro-copy that surfaces the AI resolver feature without the user
    // needing to open settings or hit a conflict first.
    expect(
      screen.getByText(/AI resolves conflicts automatically/i),
    ).toBeInTheDocument();
  });

  it("hides the CTA when current branch equals base branch", async () => {
    mockGetGitBranchInfo.mockResolvedValue({
      branch: "main",
      ahead: 0,
      behind: 0,
      has_upstream: true,
    });

    renderPanel();
    await flushPromises();

    expect(
      screen.queryByRole("button", { name: /Merge into main/i }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByText(/AI resolves conflicts automatically/i),
    ).not.toBeInTheDocument();
  });

  it("hides the CTA while a merge is in progress (banner takes over)", async () => {
    mockGetMergeState.mockResolvedValue({
      is_merging: true,
      is_rebasing: false,
      merge_head: "deadbeef",
      conflicted_files: [{ path: "src/x.ts", conflict_type: "both_modified" }],
    });
    mockGetGitStatus.mockResolvedValue([
      { path: "src/x.ts", status: "conflicted", is_staged: false, is_unstaged: true, additions: 0, deletions: 0, conflict_type: "both_modified" },
    ]);

    renderPanel();
    await flushPromises();

    // Banner should be visible — the labeled CTA must NOT compete with it.
    expect(screen.getByText(/Merge in progress/)).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /^Merge into main$/i }),
    ).not.toBeInTheDocument();
  });

  it("hides the CTA when there are unresolved conflicts (resolver banner owns the moment)", async () => {
    // Not actively merging, but conflicted files exist (e.g. user resumed
    // after a partial state). The CTA should defer to the conflict UI.
    mockGetGitStatus.mockResolvedValue([
      { path: "src/x.ts", status: "conflicted", is_staged: false, is_unstaged: true, additions: 0, deletions: 0, conflict_type: "both_modified" },
    ]);

    renderPanel();
    await flushPromises();

    expect(
      screen.queryByRole("button", { name: /^Merge into main$/i }),
    ).not.toBeInTheDocument();
  });

  it("primary click opens the merge-into-base confirmation dialog", async () => {
    const user = userEvent.setup();
    renderPanel();
    await flushPromises();

    await user.click(screen.getByRole("button", { name: /Merge into main/i }));

    // Dialog confirm button appears (the dialog reuses existing
    // mergeIntoBase flow — we don't re-test the dialog itself here).
    expect(
      await screen.findByRole("button", { name: /^Merge into main$/i }),
    ).toBeInTheDocument();
  });

  it("caret dropdown exposes 'Merge base into current' as a secondary action", async () => {
    const user = userEvent.setup();
    renderPanel();
    await flushPromises();

    await user.click(screen.getByRole("button", { name: "Merge options" }));
    await user.click(await screen.findByText("Merge main into current"));

    await waitFor(() => {
      expect(mockMergeBranch).toHaveBeenCalledWith("/home/user/project", "main");
    });
  });

  it("caret dropdown 'Resolver settings' opens settings to the git section", async () => {
    const user = userEvent.setup();
    renderPanel();
    await flushPromises();

    await user.click(screen.getByRole("button", { name: "Merge options" }));
    await user.click(await screen.findByText("Resolver settings"));

    expect(mockSetShowSettings).toHaveBeenCalledWith(true, "git");
  });

  it("file count badge reflects baseBranchFiles length", async () => {
    mockGetBaseBranchDiff.mockResolvedValue({
      files: [
        { path: "a.ts", status: "modified", is_staged: false, is_unstaged: true, additions: 1, deletions: 0 },
        { path: "b.ts", status: "modified", is_staged: false, is_unstaged: true, additions: 2, deletions: 0 },
        { path: "c.ts", status: "added", is_staged: false, is_unstaged: true, additions: 5, deletions: 0 },
      ],
      merge_base_commit: "abc",
    });

    renderPanel();
    await flushPromises();

    const cta = screen.getByRole("button", { name: /Merge into main/i });
    // Count badge inside the button.
    expect(cta.textContent).toMatch(/3/);
  });

  it("renders disabled (no count badge) when on feature branch with nothing to merge yet", async () => {
    // Fresh feature branch — no committed diff vs base. The CTA must still
    // RENDER so a new user discovers it; just disabled until they commit.
    // Regression: an earlier version hid the CTA entirely in this state,
    // which left a brand-new user staring at a panel that gave them no
    // signal that a merge action even existed.
    mockGetBaseBranchDiff.mockResolvedValue({ files: [], merge_base_commit: "abc" });

    renderPanel();
    await flushPromises();

    const cta = screen.getByRole("button", { name: /Merge into main/i });
    expect(cta).toBeInTheDocument();
    expect(cta).toBeDisabled();
    // No count badge when there's nothing to merge.
    expect(cta.textContent).not.toMatch(/\d/);
  });
});

// ── Resolver strategy override ──
//
// Before: the resolver was gated by `ai_resolver_enabled` and the strategy
// only existed in settings — users had to open settings before their first
// conflict to make the feature work at all.
// After: always available, strategy override lives inline next to the
// "Resolve with AI" button.

describe("Resolver strategy override", () => {
  beforeEach(() => {
    mockGetGitStatus.mockResolvedValue([
      { path: "src/x.ts", status: "conflicted", is_staged: false, is_unstaged: true, additions: 0, deletions: 0, conflict_type: "both_modified" },
    ]);
    mockAppStoreState = {
      appState: {
        config: {
          ai_commit_message_enabled: false,
          ai_resolver_enabled: false, // explicitly off — must NOT gate the resolver
          ai_resolver_strategy: "smart_merge",
          ai_resolver_cli: "claude",
          ai_resolver_model: "",
        },
        workspaces: [],
      },
    };
  });

  it("renders the strategy selector and 'Resolve with AI' button when conflicts exist", async () => {
    renderPanel();
    await flushPromises();

    expect(screen.getByLabelText("Resolution strategy")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Resolve conflicts with AI" }),
    ).toBeInTheDocument();
  });

  it("'Resolve with AI' is enabled even when ai_resolver_enabled is false", async () => {
    // Regression: previously this button was disabled until the user
    // toggled `ai_resolver_enabled` in settings. The toggle is gone; the
    // button must always be clickable.
    renderPanel();
    await flushPromises();

    const btn = screen.getByRole("button", { name: "Resolve conflicts with AI" });
    expect(btn).not.toBeDisabled();
  });

  it("clicking 'Resolve with AI' invokes startResolution with the current strategy", async () => {
    const user = userEvent.setup();
    renderPanel();
    await flushPromises();

    await user.click(screen.getByRole("button", { name: "Resolve conflicts with AI" }));

    await waitFor(() => {
      expect(mockStartResolution).toHaveBeenCalled();
    });
    // 6th positional arg is the strategy. The handler reads from
    // config.ai_resolver_strategy, which we seeded as "smart_merge".
    const [, , , , , strategy] = mockStartResolution.mock.calls[0];
    expect(strategy).toBe("smart_merge");
  });

  it("default strategy persists if config.ai_resolver_strategy is missing", async () => {
    // No strategy set in config — falls back to "smart_merge".
    mockAppStoreState = {
      appState: {
        config: { ai_commit_message_enabled: false, ai_resolver_enabled: false },
        workspaces: [],
      },
    };
    const user = userEvent.setup();
    renderPanel();
    await flushPromises();

    await user.click(screen.getByRole("button", { name: "Resolve conflicts with AI" }));

    await waitFor(() => {
      expect(mockStartResolution).toHaveBeenCalled();
    });
    const [, , , , , strategy] = mockStartResolution.mock.calls[0];
    expect(strategy).toBe("smart_merge");
  });
});
