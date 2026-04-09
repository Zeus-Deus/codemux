/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, act, cleanup, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

// Polyfill ResizeObserver for ScrollArea (not available in jsdom)
globalThis.ResizeObserver = class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

// ── Mock Tauri commands ──

const mockCheckGhStatus = vi.fn().mockResolvedValue({ status: "Authenticated", username: "test" });
const mockCheckGithubRepo = vi.fn().mockResolvedValue(true);
const mockGetBranchPullRequest = vi.fn().mockResolvedValue(null);
const mockRefreshWorkspacePr = vi.fn().mockResolvedValue(undefined);
const mockGetPullRequestChecks = vi.fn().mockResolvedValue([]);
const mockGetPrReviewComments = vi.fn().mockResolvedValue([]);
const mockGetPrInlineComments = vi.fn().mockResolvedValue([]);
const mockGetPrDeployments = vi.fn().mockResolvedValue([]);
const mockListBranches = vi.fn().mockResolvedValue([]);
const mockCreatePullRequest = vi.fn().mockResolvedValue(undefined);
const mockGetDefaultBranch = vi.fn().mockResolvedValue("main");

vi.mock("@/tauri/commands", () => ({
  checkGhStatus: (...args: unknown[]) => mockCheckGhStatus(...args),
  checkGithubRepo: (...args: unknown[]) => mockCheckGithubRepo(...args),
  getBranchPullRequest: (...args: unknown[]) => mockGetBranchPullRequest(...args),
  refreshWorkspacePr: (...args: unknown[]) => mockRefreshWorkspacePr(...args),
  getPullRequestChecks: (...args: unknown[]) => mockGetPullRequestChecks(...args),
  getPrReviewComments: (...args: unknown[]) => mockGetPrReviewComments(...args),
  getPrInlineComments: (...args: unknown[]) => mockGetPrInlineComments(...args),
  getPrDeployments: (...args: unknown[]) => mockGetPrDeployments(...args),
  listBranches: (...args: unknown[]) => mockListBranches(...args),
  createPullRequest: (...args: unknown[]) => mockCreatePullRequest(...args),
  getDefaultBranch: (...args: unknown[]) => mockGetDefaultBranch(...args),
}));

// Mock sub-components to keep tests focused on PrPanel logic
vi.mock("./pr/pr-header", () => ({ PrHeader: () => <div data-testid="pr-header" /> }));
vi.mock("./pr/pr-checks", () => ({ PrChecks: () => <div data-testid="pr-checks" /> }));
vi.mock("./pr/pr-reviews", () => ({ PrReviews: () => <div data-testid="pr-reviews" /> }));
vi.mock("./pr/pr-review-actions", () => ({ PrReviewActions: () => <div data-testid="pr-review-actions" /> }));
vi.mock("./pr/pr-deployments", () => ({ PrDeployments: () => <div data-testid="pr-deployments" /> }));
vi.mock("./pr/pr-merge-controls", () => ({ PrMergeControls: () => <div data-testid="pr-merge-controls" /> }));
vi.mock("./pr/incoming-prs-view", () => ({ IncomingPrsView: () => <div data-testid="incoming-prs-view" /> }));

import { PrPanel } from "./pr-panel";
import type { WorkspaceSnapshot, PullRequestInfo } from "@/tauri/types";
// Access the cache helpers exported at module level for cache TTL tests
import {
  getCachedGhStatus,
  setCachedGhStatus,
  getCachedRepoCheck,
  setCachedRepoCheck,
  CACHE_TTL_MS,
  // For direct mutation in tests:
  _resetCaches,
} from "./pr-panel";

function flushPromises() {
  return act(() => new Promise((r) => setTimeout(r, 0)));
}

function makeWorkspace(overrides: Partial<WorkspaceSnapshot> = {}): WorkspaceSnapshot {
  return {
    workspace_id: "ws-1",
    title: "Test",
    workspace_type: "standard",
    cwd: "/home/user/project",
    git_branch: "feat/my-feature",
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
    ...overrides,
  };
}

const mockPr: PullRequestInfo = {
  number: 42,
  url: "https://github.com/test/repo/pull/42",
  state: "OPEN",
  title: "Test PR",
  head_branch: "feat/my-feature",
  base_branch: "main",
  is_draft: false,
  mergeable: "MERGEABLE",
  additions: 10,
  deletions: 5,
  review_decision: null,
  checks_passing: null,
  updated_at: "2026-04-09T00:00:00Z",
};

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
  _resetCaches();
  mockCheckGhStatus.mockResolvedValue({ status: "Authenticated", username: "test" });
  mockCheckGithubRepo.mockResolvedValue(true);
  mockGetBranchPullRequest.mockResolvedValue(null);
  mockRefreshWorkspacePr.mockResolvedValue(undefined);
  mockGetPullRequestChecks.mockResolvedValue([]);
  mockGetPrReviewComments.mockResolvedValue([]);
  mockGetPrInlineComments.mockResolvedValue([]);
  mockGetPrDeployments.mockResolvedValue([]);
  mockGetDefaultBranch.mockResolvedValue("main");
});

afterEach(() => {
  vi.useRealTimers();
});

// ── Bug 1: Refresh triggers PR discovery when no PR ──

describe("refresh button", () => {
  it("calls refreshWorkspacePr when no PR exists", async () => {
    const user = userEvent.setup();
    render(<PrPanel workspace={makeWorkspace()} />);
    await flushPromises();

    const refreshBtn = screen.getByTitle("Refresh");
    await user.click(refreshBtn);

    expect(mockRefreshWorkspacePr).toHaveBeenCalledWith("ws-1");
  });

  it("calls getBranchPullRequest (fetchDetails) when PR exists", async () => {
    const user = userEvent.setup();
    mockGetBranchPullRequest.mockResolvedValue(mockPr);

    render(<PrPanel workspace={makeWorkspace({ pr_number: 42, pr_state: "OPEN" })} />);
    await flushPromises();

    // fetchDetails is called on mount; clear to isolate the refresh click
    mockGetBranchPullRequest.mockClear();

    const refreshBtn = screen.getByTitle("Refresh");
    await user.click(refreshBtn);
    await flushPromises();

    expect(mockGetBranchPullRequest).toHaveBeenCalledWith("/home/user/project");
    expect(mockRefreshWorkspacePr).not.toHaveBeenCalled();
  });
});

// ── Bug 2: Cache TTL expiry ──

describe("cache TTL", () => {
  it("ghStatusCache expires after TTL", () => {
    vi.useFakeTimers();
    setCachedGhStatus({ status: "Authenticated", username: "test" });

    // Before expiry
    expect(getCachedGhStatus()).toEqual({ status: "Authenticated", username: "test" });

    // After expiry
    vi.advanceTimersByTime(CACHE_TTL_MS + 1);
    expect(getCachedGhStatus()).toBeNull();
  });

  it("ghStatusCache returns value before TTL", () => {
    vi.useFakeTimers();
    setCachedGhStatus({ status: "NotAuthenticated" });

    vi.advanceTimersByTime(CACHE_TTL_MS - 1000);
    expect(getCachedGhStatus()).toEqual({ status: "NotAuthenticated" });
  });

  it("repoCheckCache expires after TTL", () => {
    vi.useFakeTimers();
    setCachedRepoCheck("/path/a", true);

    expect(getCachedRepoCheck("/path/a")).toBe(true);

    vi.advanceTimersByTime(CACHE_TTL_MS + 1);
    expect(getCachedRepoCheck("/path/a")).toBeUndefined();
  });

  it("repoCheckCache entries are independent per cwd", () => {
    vi.useFakeTimers();
    setCachedRepoCheck("/path/a", true);

    vi.advanceTimersByTime(CACHE_TTL_MS / 2);
    setCachedRepoCheck("/path/b", false);

    // Expire first entry
    vi.advanceTimersByTime(CACHE_TTL_MS / 2 + 1);
    expect(getCachedRepoCheck("/path/a")).toBeUndefined();
    expect(getCachedRepoCheck("/path/b")).toBe(false);
  });

  it("manual refresh bypasses caches", async () => {
    const user = userEvent.setup();

    // Pre-populate caches so auth init won't call checkGhStatus/checkGithubRepo
    setCachedGhStatus({ status: "Authenticated", username: "test" });
    setCachedRepoCheck("/home/user/project", true);

    render(<PrPanel workspace={makeWorkspace()} />);
    await flushPromises();

    // Auth init should NOT have called these (cache was warm)
    expect(mockCheckGhStatus).not.toHaveBeenCalled();
    expect(mockCheckGithubRepo).not.toHaveBeenCalled();

    // Click refresh — should bust caches
    const refreshBtn = screen.getByTitle("Refresh");
    await user.click(refreshBtn);
    await flushPromises();

    // Caches were busted; verify they're empty
    expect(getCachedGhStatus()).toBeNull();
    expect(getCachedRepoCheck("/home/user/project")).toBeUndefined();
  });

  it("stale failure recovers after TTL", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });

    // First render: checkGithubRepo fails (cached as false)
    mockCheckGithubRepo.mockResolvedValue(false);

    const { unmount } = render(<PrPanel workspace={makeWorkspace()} />);
    await flushPromises();

    expect(screen.getByText("Not a GitHub repository")).toBeInTheDocument();

    unmount();

    // Advance past TTL so cache expires
    vi.advanceTimersByTime(CACHE_TTL_MS + 1);

    // Now mock success
    mockCheckGithubRepo.mockResolvedValue(true);

    render(<PrPanel workspace={makeWorkspace()} />);
    await flushPromises();

    // Should re-check (cache expired) and now see the PR panel content
    expect(screen.queryByText("Not a GitHub repository")).not.toBeInTheDocument();
  });
});

// ── Bug 3: Error state display ──

describe("error state", () => {
  it("shows error when fetchDetails fails", async () => {
    mockGetBranchPullRequest.mockRejectedValue(new Error("gh CLI error"));

    render(<PrPanel workspace={makeWorkspace({ pr_number: 42, pr_state: "OPEN" })} />);
    await flushPromises();

    await waitFor(() => {
      expect(screen.getByText(/gh CLI error/)).toBeInTheDocument();
    });
  });

  it("clears error on successful retry", async () => {
    const user = userEvent.setup();
    // First: fail
    mockGetBranchPullRequest.mockRejectedValue(new Error("network error"));

    render(<PrPanel workspace={makeWorkspace({ pr_number: 42, pr_state: "OPEN" })} />);
    await flushPromises();

    await waitFor(() => {
      expect(screen.getByText(/network error/)).toBeInTheDocument();
    });

    // Now succeed
    mockGetBranchPullRequest.mockResolvedValue(mockPr);

    const refreshBtn = screen.getByTitle("Refresh");
    await user.click(refreshBtn);
    await flushPromises();

    await waitFor(() => {
      expect(screen.queryByText(/network error/)).not.toBeInTheDocument();
    });
  });

  it("shows NoPrView (not error) when genuinely no PR", async () => {
    render(<PrPanel workspace={makeWorkspace()} />);
    await flushPromises();

    expect(screen.getByText("No pull request for this branch")).toBeInTheDocument();
    // No error banner should be present
    expect(screen.queryByText(/Failed/)).not.toBeInTheDocument();
    expect(screen.queryByText(/error/i)).not.toBeInTheDocument();
  });

  it("shows error when refresh discovery fails", async () => {
    const user = userEvent.setup();
    mockRefreshWorkspacePr.mockRejectedValue(new Error("gh pr view failed"));

    render(<PrPanel workspace={makeWorkspace()} />);
    await flushPromises();

    const refreshBtn = screen.getByTitle("Refresh");
    await user.click(refreshBtn);
    await flushPromises();

    await waitFor(() => {
      expect(screen.getByText(/gh pr view failed/)).toBeInTheDocument();
    });
  });
});

// ── Base branch detection → IncomingPrsView ──

describe("incoming PRs on base branch", () => {
  it("renders IncomingPrsView when on default branch with no PR", async () => {
    mockGetDefaultBranch.mockResolvedValue("main");
    render(<PrPanel workspace={makeWorkspace({ git_branch: "main" })} />);
    await flushPromises();

    await waitFor(() => {
      expect(screen.getByTestId("incoming-prs-view")).toBeInTheDocument();
    });
    expect(screen.queryByText("No pull request for this branch")).not.toBeInTheDocument();
  });

  it("renders NoPrView when on feature branch (not default)", async () => {
    mockGetDefaultBranch.mockResolvedValue("main");
    render(<PrPanel workspace={makeWorkspace({ git_branch: "feat/my-feature" })} />);
    await flushPromises();

    await waitFor(() => {
      expect(screen.getByText("No pull request for this branch")).toBeInTheDocument();
    });
    expect(screen.queryByTestId("incoming-prs-view")).not.toBeInTheDocument();
  });

  it("renders NoPrView when git_branch is null (detached HEAD)", async () => {
    mockGetDefaultBranch.mockResolvedValue("main");
    render(<PrPanel workspace={makeWorkspace({ git_branch: null })} />);
    await flushPromises();

    await waitFor(() => {
      expect(screen.getByText("No pull request for this branch")).toBeInTheDocument();
    });
    expect(screen.queryByTestId("incoming-prs-view")).not.toBeInTheDocument();
  });

  it("renders NoPrView when getDefaultBranch fails", async () => {
    mockGetDefaultBranch.mockRejectedValue(new Error("git error"));
    render(<PrPanel workspace={makeWorkspace({ git_branch: "main" })} />);
    await flushPromises();

    await waitFor(() => {
      expect(screen.getByText("No pull request for this branch")).toBeInTheDocument();
    });
  });

  it("still renders PrView when on default branch with existing PR", async () => {
    mockGetDefaultBranch.mockResolvedValue("main");
    mockGetBranchPullRequest.mockResolvedValue(mockPr);
    render(<PrPanel workspace={makeWorkspace({ git_branch: "main", pr_number: 42, pr_state: "OPEN" })} />);
    await flushPromises();

    await waitFor(() => {
      expect(screen.getByTestId("pr-header")).toBeInTheDocument();
    });
    expect(screen.queryByTestId("incoming-prs-view")).not.toBeInTheDocument();
  });
});
