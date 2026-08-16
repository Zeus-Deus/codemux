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

const mockCheckProviderAuth = vi.fn().mockResolvedValue({
  kind: "github",
  supported: true,
  installed: true,
  authenticated: true,
  username: "test",
});
const mockCheckGithubRepo = vi.fn().mockResolvedValue(true);
const mockGetBranchPullRequest = vi.fn().mockResolvedValue(null);
const mockRefreshWorkspacePr = vi.fn().mockResolvedValue(undefined);
const mockGetPullRequestChecks = vi.fn().mockResolvedValue([]);
const mockGetPrReviewComments = vi.fn().mockResolvedValue([]);
const mockGetPrInlineComments = vi.fn().mockResolvedValue([]);
const mockListBranches = vi.fn().mockResolvedValue([]);
const mockCreatePullRequest = vi.fn().mockResolvedValue(undefined);
const mockGetDefaultBranch = vi.fn().mockResolvedValue("main");
// The two no-PR empty states differ by whether the branch is on the
// remote at all, so the panel asks. Default to "pushed" — a branch with
// commits and no PR is the ordinary case.
const mockGetGitBranchInfo = vi
  .fn()
  .mockResolvedValue({ branch: "feat/my-feature", ahead: 2, behind: 0, has_upstream: true });
const mockGitPushChanges = vi.fn().mockResolvedValue(undefined);
vi.mock("@/tauri/commands", () => ({
  checkProviderAuth: (...args: unknown[]) => mockCheckProviderAuth(...args),
  checkGithubRepo: (...args: unknown[]) => mockCheckGithubRepo(...args),
  getBranchPullRequest: (...args: unknown[]) => mockGetBranchPullRequest(...args),
  refreshWorkspacePr: (...args: unknown[]) => mockRefreshWorkspacePr(...args),
  getPullRequestChecks: (...args: unknown[]) => mockGetPullRequestChecks(...args),
  getPrReviewComments: (...args: unknown[]) => mockGetPrReviewComments(...args),
  getPrInlineComments: (...args: unknown[]) => mockGetPrInlineComments(...args),
  listBranches: (...args: unknown[]) => mockListBranches(...args),
  createPullRequest: (...args: unknown[]) => mockCreatePullRequest(...args),
  getDefaultBranch: (...args: unknown[]) => mockGetDefaultBranch(...args),
  getGitBranchInfo: (...args: unknown[]) => mockGetGitBranchInfo(...args),
  gitPushChanges: (...args: unknown[]) => mockGitPushChanges(...args),
}));

// Mock sub-components to keep these tests about ReviewPanel's gate.
// The detail surface has its own test file.
vi.mock("./review/review-detail", () => ({
  ReviewDetail: () => <div data-testid="pr-header" />,
}));
vi.mock("./review/review-threads", () => ({ ReviewThreads: () => <div data-testid="pr-reviews" /> }));
vi.mock("./review/incoming-prs-view", () => ({ IncomingPrsView: () => <div data-testid="incoming-prs-view" /> }));

import { ReviewPanel } from "./review-panel";
import type { WorkspaceSnapshot, PullRequestInfo } from "@/tauri/types";
// Access the cache helpers exported at module level for cache TTL tests
import {
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
    notifications_muted: false,
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
  body: null,
  comments: [],
  totalComments: 0,
  author: null,
  head_ref_oid: null,
  head_repository_owner: null,
  merge_state_status: null,
  changed_files: null,
  merged_by: null,
  merged_at: null,
  review_requests: [],
  latest_reviews: [],
};

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
  _resetCaches();
  mockCheckProviderAuth.mockResolvedValue({
    kind: "github",
    supported: true,
    installed: true,
    authenticated: true,
    username: "test",
  });
  mockCheckGithubRepo.mockResolvedValue(true);
  mockGetBranchPullRequest.mockResolvedValue(null);
  mockRefreshWorkspacePr.mockResolvedValue(undefined);
  mockGetPullRequestChecks.mockResolvedValue([]);
  mockGetPrReviewComments.mockResolvedValue([]);
  mockGetPrInlineComments.mockResolvedValue([]);
  mockGetDefaultBranch.mockResolvedValue("main");
  mockGetGitBranchInfo.mockResolvedValue({
    branch: "feat/my-feature",
    ahead: 2,
    behind: 0,
    has_upstream: true,
  });
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
  // The auth cache's own TTL and never-cache-a-failure rules are covered
  // where it lives, in `src/lib/provider-auth.test.ts`.

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
    // One render populates both caches; the second must reach neither
    // backend command.
    const { unmount } = renderPanel(<ReviewPanel workspace={makeWorkspace()} />);
    await flushPromises();
    unmount();
    vi.clearAllMocks();

    renderPanel(<ReviewPanel workspace={makeWorkspace()} />);
    await flushPromises();

    expect(mockCheckProviderAuth).not.toHaveBeenCalled();
    expect(mockCheckGithubRepo).not.toHaveBeenCalled();
  });

  /// The auth slot is keyed by path as well as product: two checkouts on
  /// two self-hosted instances of the same product are two separate
  /// logins, and one must never answer for the other.
  it("does not reuse one checkout's auth verdict for another path", async () => {
    const { unmount } = renderPanel(<ReviewPanel workspace={makeWorkspace()} />);
    await flushPromises();
    unmount();
    vi.clearAllMocks();

    renderPanel(
      <ReviewPanel workspace={makeWorkspace({ cwd: "/home/user/elsewhere" })} />,
    );
    await flushPromises();
    expect(mockCheckProviderAuth).toHaveBeenCalledWith("/home/user/elsewhere");
  });

  it("failures are not cached — recovery is immediate, no TTL wait", async () => {
    // First render: checkGithubRepo returns false. The buggy old behavior
    // cached this for 60s; the fix drops failure caching entirely so the
    // user sees the recovery on the very next render.
    mockCheckGithubRepo.mockResolvedValue(false);

    const { unmount } = renderPanel(<ReviewPanel workspace={makeWorkspace()} />);
    await flushPromises();

    expect(screen.getByTestId("empty-unsupported")).toBeInTheDocument();
    expect(getCachedRepoCheck("/home/user/project")).toBeUndefined();

    unmount();

    // No TTL advance — fix the underlying issue and re-render immediately.
    mockCheckGithubRepo.mockResolvedValue(true);

    renderPanel(<ReviewPanel workspace={makeWorkspace()} />);
    await flushPromises();

    expect(screen.queryByTestId("empty-unsupported")).not.toBeInTheDocument();
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

    // Never a blank panel: a PR that has never loaded and whose fetch
    // fails is a reachability state with a way out, not an error dump.
    await waitFor(() => {
      expect(screen.getByTestId("empty-unreachable")).toBeInTheDocument();
    });
  });

  it("shows NoPrView (not error) when genuinely no PR", async () => {
    renderPanel(<ReviewPanel workspace={makeWorkspace()} />);
    await flushPromises();

    await waitFor(() => {
      expect(screen.getByTestId("empty-no-pr")).toBeInTheDocument();
    });
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
    expect(screen.queryByTestId("empty-no-pr")).not.toBeInTheDocument();
  });

  it("renders NoPrView when on feature branch (not default)", async () => {
    mockGetDefaultBranch.mockResolvedValue("main");
    renderPanel(<ReviewPanel workspace={makeWorkspace({ git_branch: "feat/my-feature" })} />);
    await flushPromises();

    await waitFor(() => {
      expect(screen.getByTestId("empty-no-pr")).toBeInTheDocument();
    });
    expect(screen.queryByTestId("incoming-prs-view")).not.toBeInTheDocument();
  });

  it("renders NoPrView when git_branch is null (detached HEAD)", async () => {
    mockGetDefaultBranch.mockResolvedValue("main");
    renderPanel(<ReviewPanel workspace={makeWorkspace({ git_branch: null })} />);
    await flushPromises();

    await waitFor(() => {
      expect(screen.getByTestId("empty-no-pr")).toBeInTheDocument();
    });
    expect(screen.queryByTestId("incoming-prs-view")).not.toBeInTheDocument();
  });

  it("renders NoPrView when getDefaultBranch fails", async () => {
    mockGetDefaultBranch.mockRejectedValue(new Error("git error"));
    renderPanel(<ReviewPanel workspace={makeWorkspace({ git_branch: "main" })} />);
    await flushPromises();

    await waitFor(() => {
      expect(screen.getByTestId("empty-no-pr")).toBeInTheDocument();
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

// ── Provider-aware copy ──
//
// Two properties matter here, and they pull in opposite directions:
// a GitHub workspace must render byte-identical copy to what it rendered
// before detection existed, and a GitLab workspace must not borrow
// GitHub's nouns, CLI, or install URL.

describe("provider-aware copy", () => {
  function auth(over: Record<string, unknown> = {}) {
    return {
      kind: "github",
      supported: true,
      installed: true,
      authenticated: true,
      username: "test",
      ...over,
    };
  }

  beforeEach(() => {
    _resetCaches();
    mockCheckProviderAuth.mockResolvedValue(auth());
  });

  /// The whole point of the host-scoped probe: the panel asks about the
  /// checkout in front of it, not about `gh`'s global login and not by
  /// sweeping every CLI on the machine.
  it("probes the workspace path rather than a product-wide login", async () => {
    renderPanel(<ReviewPanel workspace={makeWorkspace({ provider_kind: "gitlab" })} />);
    await flushPromises();
    expect(mockCheckProviderAuth).toHaveBeenCalledWith("/home/user/project");
  });

  it("does not let one product's cached auth answer for another", async () => {
    mockCheckProviderAuth.mockResolvedValue(auth({ kind: "gitlab" }));
    renderPanel(<ReviewPanel workspace={makeWorkspace({ provider_kind: "gitlab" })} />);
    await flushPromises();
    expect(mockCheckProviderAuth).toHaveBeenCalled();
    mockCheckProviderAuth.mockClear();
    cleanup();

    // A different workspace on a different product re-probes rather than
    // reusing the warm slot.
    renderPanel(
      <ReviewPanel
        workspace={makeWorkspace({ provider_kind: "github", cwd: "/home/user/other" })}
      />,
    );
    await flushPromises();
    expect(mockCheckProviderAuth).toHaveBeenCalledWith("/home/user/other");
  });

  it("names glab in the not-installed and signed-out states", async () => {
    mockCheckProviderAuth.mockResolvedValue(
      auth({ kind: "gitlab", installed: false, authenticated: false, username: null }),
    );
    const { unmount } = renderPanel(
      <ReviewPanel workspace={makeWorkspace({ provider_kind: "gitlab" })} />,
    );
    await flushPromises();
    expect(screen.getByText("GitLab CLI (glab) isn't installed")).toBeInTheDocument();
    expect(screen.getByText("glab", { selector: "span" })).toBeInTheDocument();
    unmount();
    _resetCaches();

    mockCheckProviderAuth.mockResolvedValue(
      auth({ kind: "gitlab", authenticated: false, username: null }),
    );
    renderPanel(<ReviewPanel workspace={makeWorkspace({ provider_kind: "gitlab" })} />);
    await flushPromises();
    expect(screen.getByText("Sign in to GitLab")).toBeInTheDocument();
    expect(screen.getByText("glab auth login")).toBeInTheDocument();
  });

  it("keeps the exact GitHub wording it had before detection existed", async () => {
    mockCheckProviderAuth.mockResolvedValue(
      auth({ installed: false, authenticated: false, username: null }),
    );
    const { unmount } = renderPanel(<ReviewPanel workspace={makeWorkspace()} />);
    await flushPromises();
    expect(screen.getByText("GitHub CLI (gh) isn't installed")).toBeInTheDocument();
    unmount();
    _resetCaches();

    mockCheckProviderAuth.mockResolvedValue(
      auth({ authenticated: false, username: null }),
    );
    renderPanel(<ReviewPanel workspace={makeWorkspace()} />);
    await flushPromises();
    expect(screen.getByText("Sign in to GitHub")).toBeInTheDocument();
    expect(screen.getByText("gh auth login")).toBeInTheDocument();
  });

  it("says merge request, not pull request, on the GitLab create affordance", async () => {
    mockCheckGithubRepo.mockResolvedValue(true);
    renderPanel(<ReviewPanel workspace={makeWorkspace({ provider_kind: "gitlab" })} />);
    await flushPromises();
    await waitFor(() => {
      expect(screen.getByText("No merge request yet")).toBeInTheDocument();
    });
    expect(
      screen.getByRole("button", { name: "Open a merge request" }),
    ).toBeInTheDocument();
  });

  it("keeps the GitHub create affordance in title case", async () => {
    mockCheckGithubRepo.mockResolvedValue(true);
    renderPanel(<ReviewPanel workspace={makeWorkspace()} />);
    await flushPromises();
    await waitFor(() => {
      expect(screen.getByText("No pull request yet")).toBeInTheDocument();
    });
    expect(
      screen.getByRole("button", { name: "Open a pull request" }),
    ).toBeInTheDocument();
  });

  /// A host Codemux cannot serve is not a missing-CLI problem: `gh` may
  /// well be installed and working. Sending that user to cli.github.com
  /// to fix nothing was the pre-fix behavior — the empty state must say
  /// what is actually true, that this checkout is not one Codemux acts
  /// on.
  it("does not blame a missing CLI for a host it cannot serve", async () => {
    mockCheckProviderAuth.mockResolvedValue(
      auth({ supported: false, installed: true, authenticated: true }),
    );
    renderPanel(<ReviewPanel workspace={makeWorkspace()} />);
    await flushPromises();
    expect(
      screen.getByText("No supported source control host for this repository"),
    ).toBeInTheDocument();
    expect(screen.queryByText(/isn't installed/)).not.toBeInTheDocument();
    expect(screen.queryByText(/cli\.github\.com/)).not.toBeInTheDocument();
  });

  /// Detection classified nothing, so there is no product to name —
  /// borrowing one would put a vendor's name on a checkout it does not
  /// serve.
  it("stays generic when the host classified as nothing at all", async () => {
    mockCheckProviderAuth.mockResolvedValue(
      auth({ kind: "unknown", supported: false, installed: true, authenticated: true }),
    );
    renderPanel(<ReviewPanel workspace={makeWorkspace({ provider_kind: "unknown" })} />);
    await flushPromises();
    expect(
      screen.getByText("No supported source control host for this repository"),
    ).toBeInTheDocument();
  });

  it("reports an unsupported product without inventing a CLI to install", async () => {
    mockCheckProviderAuth.mockResolvedValue(
      auth({ kind: "bitbucket", supported: false, installed: false, authenticated: false, username: null }),
    );
    renderPanel(<ReviewPanel workspace={makeWorkspace({ provider_kind: "bitbucket" })} />);
    await flushPromises();
    expect(screen.getByText("Bitbucket, read-only")).toBeInTheDocument();
    expect(screen.queryByText(/isn't installed/)).not.toBeInTheDocument();
  });
});
