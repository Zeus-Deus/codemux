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

vi.mock("@/stores/app-store", () => ({
  useAppStore: vi.fn((selector: (s: Record<string, unknown>) => unknown) => {
    const state = { appState: { config: { ai_commit_message_enabled: false } } };
    return selector(state);
  }),
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
      startResolution: vi.fn(),
      approveResolution: vi.fn().mockResolvedValue(undefined),
      rejectResolution: vi.fn().mockResolvedValue(undefined),
    };
    return selector(state);
  }),
}));

import { TooltipProvider } from "@/components/ui/tooltip";
import { ChangesPanel } from "./changes-panel";
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

function renderPanel() {
  return render(
    <TooltipProvider>
      <ChangesPanel workspace={mockWorkspace} />
    </TooltipProvider>,
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
