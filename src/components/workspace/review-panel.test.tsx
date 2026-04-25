/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, act, cleanup, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";

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
  listBranches: (...args: unknown[]) => mockListBranches(...args),
  createPullRequest: (...args: unknown[]) => mockCreatePullRequest(...args),
  getDefaultBranch: (...args: unknown[]) => mockGetDefaultBranch(...args),
}));

// Mock sub-components to keep tests focused on ReviewPanel logic
vi.mock("./review/review-header", () => ({ ReviewHeader: () => <div data-testid="pr-header" /> }));
vi.mock("./review/review-checks", () => ({ ReviewChecks: () => <div data-testid="pr-checks" /> }));
vi.mock("./review/review-threads", () => ({ ReviewThreads: () => <div data-testid="pr-reviews" /> }));
vi.mock("./review/incoming-prs-view", () => ({ IncomingPrsView: () => <div data-testid="incoming-prs-view" /> }));

import { ReviewPanel } from "./review-panel";
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
} from "./review-panel";

function flushPromises() {
  return act(() => new Promise((r) => setTimeout(r, 0)));
}

// Each test gets a fresh QueryClient so cached data + retries don't
// leak between cases. Retries off so query errors surface immediately.
function renderPanel(node: ReactNode) {
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false, staleTime: Infinity, refetchInterval: false },
    },
  });
  return render(<QueryClientProvider client={client}>{node}</QueryClientProvider>);
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
  mockGetDefaultBranch.mockResolvedValue("main");
});

afterEach(() => {
  vi.useRealTimers();
});

// The refresh button was removed in the visual-match PR — auto-poll
// (2.5s for PR detail + checks, 30s for comments) handles freshness
// now. The discovery / fetch behaviors these tests covered are still
// exercised indirectly through the React Query hooks that fire on
// mount and on workspace switch.

describe("auto-fetch on mount", () => {
  it("calls getBranchPullRequest when PR exists on mount", async () => {
    mockGetBranchPullRequest.mockResolvedValue(mockPr);

    renderPanel(<ReviewPanel workspace={makeWorkspace({ pr_number: 42, pr_state: "OPEN" })} />);
    await flushPromises();

    expect(mockGetBranchPullRequest).toHaveBeenCalledWith("/home/user/project");
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
    setCachedGhStatus({ status: "Authenticated", username: "alice" });

    vi.advanceTimersByTime(CACHE_TTL_MS - 1000);
    expect(getCachedGhStatus()).toEqual({ status: "Authenticated", username: "alice" });
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
    setCachedRepoCheck("/path/b", true);

    // Expire only the first entry
    vi.advanceTimersByTime(CACHE_TTL_MS / 2 + 1);
    expect(getCachedRepoCheck("/path/a")).toBeUndefined();
    expect(getCachedRepoCheck("/path/b")).toBe(true);
  });

  it("warm caches skip the auth + repo init calls", async () => {
    // Pre-populate caches so auth init won't call checkGhStatus/checkGithubRepo
    setCachedGhStatus({ status: "Authenticated", username: "test" });
    setCachedRepoCheck("/home/user/project", true);

    renderPanel(<ReviewPanel workspace={makeWorkspace()} />);
    await flushPromises();

    // Auth init should NOT have called these (cache was warm)
    expect(mockCheckGhStatus).not.toHaveBeenCalled();
    expect(mockCheckGithubRepo).not.toHaveBeenCalled();
  });

  it("failures are not cached — recovery is immediate, no TTL wait", async () => {
    // First render: checkGithubRepo returns false. The buggy old behavior
    // cached this for 60s; the fix drops failure caching entirely so the
    // user sees the recovery on the very next render.
    mockCheckGithubRepo.mockResolvedValue(false);

    const { unmount } = renderPanel(<ReviewPanel workspace={makeWorkspace()} />);
    await flushPromises();

    expect(screen.getByText("Not a GitHub repository")).toBeInTheDocument();
    expect(getCachedRepoCheck("/home/user/project")).toBeUndefined();

    unmount();

    // No TTL advance — fix the underlying issue and re-render immediately.
    mockCheckGithubRepo.mockResolvedValue(true);

    renderPanel(<ReviewPanel workspace={makeWorkspace()} />);
    await flushPromises();

    expect(screen.queryByText("Not a GitHub repository")).not.toBeInTheDocument();
  });

  it("setCachedGhStatus drops non-Authenticated values", () => {
    setCachedGhStatus({ status: "NotAuthenticated" });
    expect(getCachedGhStatus()).toBeNull();

    setCachedGhStatus({ status: "NotInstalled" });
    expect(getCachedGhStatus()).toBeNull();

    setCachedGhStatus({ status: "Authenticated", username: "test" });
    expect(getCachedGhStatus()).toEqual({ status: "Authenticated", username: "test" });
  });

  it("setCachedRepoCheck drops false values", () => {
    setCachedRepoCheck("/path/x", false);
    expect(getCachedRepoCheck("/path/x")).toBeUndefined();

    setCachedRepoCheck("/path/x", true);
    expect(getCachedRepoCheck("/path/x")).toBe(true);
  });
});

// ── Bug 3: Error state display ──

describe("error state", () => {
  it("shows error when fetchDetails fails", async () => {
    mockGetBranchPullRequest.mockRejectedValue(new Error("gh CLI error"));

    renderPanel(<ReviewPanel workspace={makeWorkspace({ pr_number: 42, pr_state: "OPEN" })} />);
    await flushPromises();

    await waitFor(() => {
      expect(screen.getByText(/gh CLI error/)).toBeInTheDocument();
    });
  });

  it("shows NoPrView (not error) when genuinely no PR", async () => {
    renderPanel(<ReviewPanel workspace={makeWorkspace()} />);
    await flushPromises();

    expect(screen.getByText("No pull request for this branch")).toBeInTheDocument();
    // No error banner should be present
    expect(screen.queryByText(/Failed/)).not.toBeInTheDocument();
    expect(screen.queryByText(/error/i)).not.toBeInTheDocument();
  });
});

// ── Base branch detection → IncomingPrsView ──

describe("incoming PRs on base branch", () => {
  it("renders IncomingPrsView when on default branch with no PR", async () => {
    mockGetDefaultBranch.mockResolvedValue("main");
    renderPanel(<ReviewPanel workspace={makeWorkspace({ git_branch: "main" })} />);
    await flushPromises();

    await waitFor(() => {
      expect(screen.getByTestId("incoming-prs-view")).toBeInTheDocument();
    });
    expect(screen.queryByText("No pull request for this branch")).not.toBeInTheDocument();
  });

  it("renders NoPrView when on feature branch (not default)", async () => {
    mockGetDefaultBranch.mockResolvedValue("main");
    renderPanel(<ReviewPanel workspace={makeWorkspace({ git_branch: "feat/my-feature" })} />);
    await flushPromises();

    await waitFor(() => {
      expect(screen.getByText("No pull request for this branch")).toBeInTheDocument();
    });
    expect(screen.queryByTestId("incoming-prs-view")).not.toBeInTheDocument();
  });

  it("renders NoPrView when git_branch is null (detached HEAD)", async () => {
    mockGetDefaultBranch.mockResolvedValue("main");
    renderPanel(<ReviewPanel workspace={makeWorkspace({ git_branch: null })} />);
    await flushPromises();

    await waitFor(() => {
      expect(screen.getByText("No pull request for this branch")).toBeInTheDocument();
    });
    expect(screen.queryByTestId("incoming-prs-view")).not.toBeInTheDocument();
  });

  it("renders NoPrView when getDefaultBranch fails", async () => {
    mockGetDefaultBranch.mockRejectedValue(new Error("git error"));
    renderPanel(<ReviewPanel workspace={makeWorkspace({ git_branch: "main" })} />);
    await flushPromises();

    await waitFor(() => {
      expect(screen.getByText("No pull request for this branch")).toBeInTheDocument();
    });
  });

  it("still renders PrView when on default branch with existing PR", async () => {
    mockGetDefaultBranch.mockResolvedValue("main");
    mockGetBranchPullRequest.mockResolvedValue(mockPr);
    renderPanel(<ReviewPanel workspace={makeWorkspace({ git_branch: "main", pr_number: 42, pr_state: "OPEN" })} />);
    await flushPromises();

    await waitFor(() => {
      expect(screen.getByTestId("pr-header")).toBeInTheDocument();
    });
    expect(screen.queryByTestId("incoming-prs-view")).not.toBeInTheDocument();
  });
});
