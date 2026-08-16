/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, act, cleanup, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";

// ── Mock Tauri commands ──

const mockSubmitPrReview = vi.fn().mockResolvedValue(undefined);
const mockMergePullRequest = vi.fn().mockResolvedValue(undefined);
const mockSetPrReady = vi.fn().mockResolvedValue(undefined);
const mockClosePullRequest = vi.fn().mockResolvedValue(undefined);
const mockUpdatePullRequest = vi.fn().mockResolvedValue(undefined);
const mockRequestPrReview = vi.fn().mockResolvedValue(undefined);
const mockGetCheckLogExcerpt = vi.fn().mockResolvedValue("");
const mockCheckoutDefaultBranch = vi.fn().mockResolvedValue("main");
const mockGitPullChanges = vi.fn().mockResolvedValue(undefined);
const mockGitStashPush = vi.fn().mockResolvedValue(undefined);

vi.mock("@/tauri/commands", () => ({
  submitPrReview: (...a: unknown[]) => mockSubmitPrReview(...a),
  mergePullRequest: (...a: unknown[]) => mockMergePullRequest(...a),
  setPrReady: (...a: unknown[]) => mockSetPrReady(...a),
  closePullRequest: (...a: unknown[]) => mockClosePullRequest(...a),
  updatePullRequest: (...a: unknown[]) => mockUpdatePullRequest(...a),
  requestPrReview: (...a: unknown[]) => mockRequestPrReview(...a),
  getCheckLogExcerpt: (...a: unknown[]) => mockGetCheckLogExcerpt(...a),
  checkoutDefaultBranchInWorkspace: (...a: unknown[]) => mockCheckoutDefaultBranch(...a),
  gitPullChanges: (...a: unknown[]) => mockGitPullChanges(...a),
  gitStashPush: (...a: unknown[]) => mockGitStashPush(...a),
}));

const mockOpenUrl = vi.fn().mockResolvedValue(undefined);
vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: (...a: unknown[]) => mockOpenUrl(...a),
}));

vi.mock("@/lib/toast", () => ({
  toast: {
    info: vi.fn(),
    success: vi.fn(),
    warning: vi.fn(),
    error: vi.fn(),
  },
}));

const mockCheckOutPr = vi.fn().mockResolvedValue(undefined);
vi.mock("@/lib/pr-checkout", () => ({
  checkOutPr: (...a: unknown[]) => mockCheckOutPr(...a),
}));

const mockHandOff = vi.fn().mockResolvedValue({ route: "current", workspaceId: "ws-1" });
const mockFindWorkspaceForBranch = vi.fn().mockReturnValue(null);
vi.mock("@/lib/pr-agent-handoff", () => ({
  handOffToAgent: (...a: unknown[]) => mockHandOff(...a),
  findWorkspaceForBranch: (...a: unknown[]) => mockFindWorkspaceForBranch(...a),
}));

import { ReviewDetail, _resetHeadOidTracking } from "./review-detail";
import { _resetPrDrafts } from "./pr-drafts";
import { resolveProvider } from "@/lib/source-control";
import type { CheckInfo, PullRequestInfo } from "@/tauri/types";

const VIEWER = "mock-dev";

function makePr(over: Partial<PullRequestInfo> = {}): PullRequestInfo {
  return {
    number: 172,
    url: "https://github.com/example/codemux/pull/172",
    state: "OPEN",
    title: "feat: keep unsent drafts in the sidebar",
    head_branch: "agent/investigate-draft-mode",
    base_branch: "main",
    is_draft: false,
    mergeable: "MERGEABLE",
    additions: 1180,
    deletions: 33,
    review_decision: null,
    checks_passing: null,
    updated_at: new Date(Date.now() - 38 * 60_000).toISOString(),
    body: "Keeps invested drafts as independent sessions.",
    comments: [],
    totalComments: 0,
    author: VIEWER,
    head_ref_oid: "abc123",
    head_repository_owner: "example",
    merge_state_status: "CLEAN",
    changed_files: 8,
    merged_by: null,
    merged_at: null,
    review_requests: [],
    latest_reviews: [],
    ...over,
  };
}

function check(name: string, conclusion: string): CheckInfo {
  return {
    name,
    status: conclusion === "pending" ? "IN_PROGRESS" : "COMPLETED",
    conclusion,
    elapsed_time: "1m 02s",
    detail_url: `https://github.com/example/codemux/actions/${name}`,
    started_at: null,
    completed_at: null,
  };
}

const GREEN_CHECKS = [check("build", "pass"), check("lint", "pass")];

function renderDetail(over: Partial<Parameters<typeof ReviewDetail>[0]> = {}) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, refetchInterval: false } },
  });
  const props = {
    pr: makePr(),
    checks: GREEN_CHECKS,
    reviews: [],
    inlineComments: [],
    checksLoading: false,
    commentsLoading: false,
    cwd: "/repo",
    workspaceId: "ws-1",
    projectRoot: "/repo",
    provider: resolveProvider("github"),
    repoSlug: "example/codemux",
    viewerLogin: VIEWER,
    checkedOutHere: true,
    gitBehind: 0,
    gitDirtyFiles: 0,
    staleAgeMs: null,
    onRefresh: vi.fn(),
    onOpenChanges: vi.fn(),
    ...over,
  };
  const utils = render(
    <QueryClientProvider client={client}>
      <ReviewDetail {...props} />
    </QueryClientProvider>,
  );
  const rerender = (next: Partial<typeof props>) =>
    utils.rerender(
      <QueryClientProvider client={client}>
        <ReviewDetail {...props} {...next} />
      </QueryClientProvider>,
    );
  return { ...utils, rerender, props };
}

function flush() {
  return act(() => new Promise((r) => setTimeout(r, 0)));
}

/**
 * Geometry-affecting classes on the primary slot.
 *
 * Binding rule 1 says these must not change between states — only
 * colour may. `text-[#…]` is a colour and is filtered out; `text-[11px]`
 * is a font size and therefore geometry, so it stays.
 */
function primaryGeometry(): string {
  const el = screen.getByTestId("review-primary-action");
  return Array.from(el.classList)
    .filter(
      (c) =>
        /^(h-|px-|py-|w-|min-w-|rounded-|gap-|text-\[)/.test(c) && !c.startsWith("text-[#"),
    )
    .sort()
    .join(" ");
}

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
  _resetPrDrafts();
  _resetHeadOidTracking();
});

afterEach(() => {
  cleanup();
});

// ── 1. Action bar state machine ──

describe("action bar state machine", () => {
  it("gives a reviewer the verdict controls, not a merge button", async () => {
    renderDetail({ pr: makePr({ author: "someone-else" }) });
    await flush();

    expect(screen.getByTestId("review-action-bar")).toHaveAttribute(
      "data-bar-state",
      "reviewer",
    );
    expect(screen.getByTestId("verdict-approve")).toBeInTheDocument();
    expect(screen.getByTestId("verdict-request-changes")).toBeInTheDocument();
    expect(screen.queryByText("Merge")).not.toBeInTheDocument();
  });

  it("gives the author a green merge bar when nothing blocks", async () => {
    renderDetail();
    await flush();

    expect(screen.getByTestId("review-action-bar")).toHaveAttribute(
      "data-bar-state",
      "author-green",
    );
    expect(screen.getByTestId("review-primary-action")).toHaveTextContent("Merge");
    expect(screen.getByTestId("bar-sentence")).toHaveTextContent("2 checks passed");
    expect(screen.getByTestId("bar-sentence")).toHaveTextContent("no conflicts");
  });

  it("names the blocking reason in words and keeps Merge in place", async () => {
    renderDetail({
      pr: makePr({ mergeable: "CONFLICTING", merge_state_status: "DIRTY" }),
    });
    await flush();

    expect(screen.getByTestId("review-action-bar")).toHaveAttribute(
      "data-bar-state",
      "author-blocked",
    );
    // Not "Merge unavailable" — the reason, in words, beside the button.
    expect(screen.getByTestId("bar-sentence")).toHaveTextContent("Conflicts with main");
    expect(screen.getByTestId("review-primary-action")).toHaveTextContent("Merge");
  });

  it("names a running check as the reason when CI hasn't finished", async () => {
    renderDetail({ checks: [check("build", "pass"), check("e2e", "pending")] });
    await flush();

    expect(screen.getByTestId("bar-sentence")).toHaveTextContent("1 check still running");
  });

  it("offers Close and Ready for review on a draft", async () => {
    renderDetail({ pr: makePr({ is_draft: true }) });
    await flush();

    expect(screen.getByTestId("review-action-bar")).toHaveAttribute(
      "data-bar-state",
      "author-draft",
    );
    expect(screen.getByText("Draft · reviewers aren't notified")).toBeInTheDocument();
    expect(screen.getByTestId("review-primary-action")).toHaveTextContent(
      "Ready for review",
    );
  });

  it("keeps the primary slot the same size and position across states", async () => {
    // Binding rule 1: a state change repaints the primary; it never
    // moves or resizes it. If Merge lands where Approve was, someone
    // merges by accident.
    const { unmount } = renderDetail();
    await flush();
    const green = primaryGeometry();
    unmount();

    renderDetail({
      pr: makePr({ mergeable: "CONFLICTING", merge_state_status: "DIRTY" }),
    });
    await flush();
    const blocked = primaryGeometry();
    cleanup();

    renderDetail({ pr: makePr({ is_draft: true }) });
    await flush();
    const draft = primaryGeometry();

    expect(blocked).toBe(green);
    expect(draft).toBe(green);
    expect(green).not.toBe("");
  });
});

// ── 2. Verdict submit ──

describe("submitting a verdict", () => {
  it("sends the typed text with the chosen verdict", async () => {
    const user = userEvent.setup();
    renderDetail({ pr: makePr({ author: "someone-else" }) });
    await flush();

    await user.click(screen.getByTestId("review-composer-collapsed"));
    await user.type(screen.getByTestId("review-composer"), "Looks good");
    await user.click(screen.getByTestId("verdict-approve"));

    await waitFor(() => {
      expect(mockSubmitPrReview).toHaveBeenCalledWith("/repo", 172, "approve", "Looks good");
    });
  });

  it("keeps the text and offers Retry when the submit fails", async () => {
    const user = userEvent.setup();
    mockSubmitPrReview.mockRejectedValueOnce("host unreachable");
    renderDetail({ pr: makePr({ author: "someone-else" }) });
    await flush();

    await user.click(screen.getByTestId("review-composer-collapsed"));
    await user.type(screen.getByTestId("review-composer"), "Needs a test");
    await user.click(screen.getByTestId("verdict-approve"));

    const notice = await screen.findByTestId("drift-notice");
    expect(notice).toHaveAttribute("data-drift-kind", "submit-failed");
    expect(notice).toHaveTextContent("host unreachable");
    expect(notice).toHaveTextContent("your notes are still here");
    // The text is never the casualty.
    expect(screen.getByTestId("review-composer")).toHaveValue("Needs a test");

    // Retry resubmits the same verdict and the same text.
    mockSubmitPrReview.mockResolvedValueOnce(undefined);
    await user.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() => {
      expect(mockSubmitPrReview).toHaveBeenLastCalledWith(
        "/repo",
        172,
        "approve",
        "Needs a test",
      );
    });
  });
});

// ── 3. Merge sheet ──

describe("merge sheet", () => {
  it("prefills the commit title, follows the strategy, and merges", async () => {
    const user = userEvent.setup();
    renderDetail();
    await flush();

    await user.click(screen.getByTestId("review-primary-action"));

    const titleField = await screen.findByTestId("merge-commit-title");
    expect(titleField).toHaveValue("feat: keep unsent drafts in the sidebar (#172)");
    expect(screen.getByTestId("merge-confirm")).toHaveTextContent("Squash and merge");

    // Switching strategy rewrites the confirm label — the button says
    // what it will do.
    await user.click(screen.getByTestId("merge-strategy-rebase"));
    expect(screen.getByTestId("merge-confirm")).toHaveTextContent("Rebase and merge");

    await user.click(screen.getByTestId("merge-strategy-squash"));
    await user.click(screen.getByTestId("merge-confirm"));

    await waitFor(() => {
      expect(mockMergePullRequest).toHaveBeenCalledWith(
        "/repo",
        172,
        "squash",
        true,
        "feat: keep unsent drafts in the sidebar (#172)",
        "",
      );
    });
  });

  it("passes delete_branch = false when the box is unchecked", async () => {
    const user = userEvent.setup();
    renderDetail();
    await flush();

    await user.click(screen.getByTestId("review-primary-action"));
    await user.click(await screen.findByTestId("merge-delete-branch"));
    await user.click(screen.getByTestId("merge-confirm"));

    await waitFor(() => {
      expect(mockMergePullRequest).toHaveBeenCalledWith(
        "/repo",
        172,
        "squash",
        false,
        expect.any(String),
        "",
      );
    });
  });

  it("shows Merging in the same slot while the merge is in flight", async () => {
    const user = userEvent.setup();
    let release: () => void = () => {};
    mockMergePullRequest.mockImplementationOnce(
      () => new Promise<void>((resolve) => { release = () => resolve(); }),
    );
    renderDetail();
    await flush();

    const before = primaryGeometry();
    await user.click(screen.getByTestId("review-primary-action"));
    await user.click(await screen.findByTestId("merge-confirm"));

    await waitFor(() => {
      expect(screen.getByTestId("merge-confirm")).toHaveTextContent("Merging");
    });
    // The bar's own primary keeps its geometry throughout.
    expect(primaryGeometry()).toBe(before);

    await act(async () => {
      release();
      await Promise.resolve();
    });
  });
});

// ── 4. Drift notices ──

describe("drift notices", () => {
  it("turns the panel into a record when the PR is merged elsewhere", async () => {
    const { rerender } = renderDetail();
    await flush();
    expect(screen.getByTestId("review-action-bar")).toHaveAttribute(
      "data-bar-state",
      "author-green",
    );

    // A poll lands with the PR merged by someone else.
    rerender({
      pr: makePr({
        state: "MERGED",
        merged_by: "juliusm",
        merged_at: new Date(Date.now() - 40_000).toISOString(),
      }),
    });
    await flush();

    const notice = screen.getByTestId("drift-notice");
    expect(notice).toHaveAttribute("data-drift-kind", "merged");
    expect(notice).toHaveTextContent("Merged by juliusm");
    expect(screen.getByRole("button", { name: "Switch to main" })).toBeInTheDocument();
    // A record, not a form.
    expect(screen.getByTestId("review-action-bar")).toHaveAttribute(
      "data-bar-state",
      "record",
    );
    expect(screen.queryByTestId("review-primary-action")).not.toBeInTheDocument();
  });

  it("labels stale data without blanking what is on screen", async () => {
    renderDetail({ staleAgeMs: 4 * 60_000 });
    await flush();

    const notice = screen.getByTestId("drift-notice");
    expect(notice).toHaveAttribute("data-drift-kind", "stale-data");
    expect(notice).toHaveTextContent("Showing data from 4m ago");
    expect(screen.getByRole("button", { name: "Retry now" })).toBeInTheDocument();

    // Binding rule 2: the content being read is still there.
    expect(
      screen.getByText("feat: keep unsent drafts in the sidebar"),
    ).toBeInTheDocument();
    expect(screen.getByTestId("review-checks")).toBeInTheDocument();
  });

  it("does not offer Pull blind over uncommitted work", async () => {
    renderDetail({ gitBehind: 2, gitDirtyFiles: 3 });
    await flush();

    const notice = screen.getByTestId("drift-notice");
    expect(notice).toHaveAttribute("data-drift-kind", "remote-ahead-dirty");
    expect(notice).toHaveTextContent("2 commits behind, and 3 files modified here");
    expect(screen.queryByRole("button", { name: "Pull" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Stash and pull" })).toBeInTheDocument();
  });

  it("shows only the most severe notice", async () => {
    // Merged outranks a stale poll and a moved remote.
    renderDetail({
      pr: makePr({ state: "MERGED", merged_by: "juliusm" }),
      staleAgeMs: 60_000,
      gitBehind: 2,
    });
    await flush();

    expect(screen.getAllByTestId("drift-notice")).toHaveLength(1);
    expect(screen.getByTestId("drift-notice")).toHaveAttribute(
      "data-drift-kind",
      "merged",
    );
  });
});

// ── 5. Description edit ──

describe("description editing", () => {
  it("survives a refetch and saves through update_pull_request", async () => {
    const user = userEvent.setup();
    const { rerender } = renderDetail();
    await flush();

    await user.click(screen.getByTestId("edit-description"));
    const editor = screen.getByTestId("description-editor");
    await user.clear(editor);
    await user.type(editor, "Rewritten body");

    // A 2.5s poll lands with a fresh PR object while you are typing.
    rerender({ pr: makePr({ updated_at: new Date().toISOString() }) });
    await flush();

    // Binding rule 4: nothing typed is discarded.
    expect(screen.getByTestId("description-editor")).toHaveValue("Rewritten body");

    await user.click(screen.getByTestId("description-save"));
    await waitFor(() => {
      expect(mockUpdatePullRequest).toHaveBeenCalledWith(
        "/repo",
        172,
        null,
        "Rewritten body",
      );
    });
  });

  it("offers no edit affordance on a merged PR", async () => {
    renderDetail({ pr: makePr({ state: "MERGED", merged_by: "juliusm" }) });
    await flush();
    expect(screen.queryByTestId("edit-description")).not.toBeInTheDocument();
  });
});

// ── Reviewers line ──

describe("reviewers line", () => {
  it("turns absence into an action", async () => {
    const user = userEvent.setup();
    renderDetail();
    await flush();

    expect(screen.getByText("Nobody is reviewing this yet")).toBeInTheDocument();
    await user.click(screen.getByTestId("request-review"));
    await user.type(screen.getByTestId("request-review-input"), "juliusm");
    await user.click(screen.getByTestId("request-review-submit"));

    await waitFor(() => {
      expect(mockRequestPrReview).toHaveBeenCalledWith("/repo", 172, "juliusm");
    });
  });

  it("shows verdict chips once reviews exist", async () => {
    renderDetail({
      pr: makePr({ latest_reviews: [{ author: "juliusm", state: "APPROVED" }] }),
    });
    await flush();

    expect(screen.queryByText("Nobody is reviewing this yet")).not.toBeInTheDocument();
    expect(screen.getByText("juliusm")).toBeInTheDocument();
    expect(screen.getByText("approved")).toBeInTheDocument();
  });
});

// ── Checks ──

describe("checks", () => {
  it("shows rows only for checks that need reading", async () => {
    renderDetail({
      checks: [check("build", "pass"), check("lint", "pass"), check("rust", "fail")],
    });
    await flush();

    expect(screen.getByText("2 passed")).toBeInTheDocument();
    expect(screen.getByTestId("check-row-rust")).toBeInTheDocument();
    // Green checks are the ones you don't have to read.
    expect(screen.queryByTestId("check-row-build")).not.toBeInTheDocument();
  });

  it("renders a failing check's log excerpt when one is available", async () => {
    mockGetCheckLogExcerpt.mockResolvedValue("error[E0432]: unresolved import");
    renderDetail({ checks: [check("rust", "fail")] });
    await flush();

    await waitFor(() => {
      expect(screen.getByText(/error\[E0432\]/)).toBeInTheDocument();
    });
  });

  it("renders the failing card without an excerpt when none is available", async () => {
    mockGetCheckLogExcerpt.mockResolvedValue("");
    renderDetail({ checks: [check("rust", "fail")] });
    await flush();

    expect(screen.getByTestId("check-row-rust")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Full log" })).toBeInTheDocument();
  });
});

// ── GitLab capability ──

describe("gitlab", () => {
  it("does not draw a request-changes verdict GitLab cannot record", async () => {
    renderDetail({
      provider: resolveProvider("gitlab"),
      pr: makePr({ author: "someone-else" }),
    });
    await flush();

    expect(screen.getByTestId("verdict-approve")).toBeInTheDocument();
    expect(screen.queryByTestId("verdict-request-changes")).not.toBeInTheDocument();
  });
});

// ── Agent handoffs ──

describe("agent handoffs", () => {
  const FAILING = check("web-checks (windows-latest)", "fail");

  it("sends the failing check with its PR context and log excerpt", async () => {
    mockGetCheckLogExcerpt.mockResolvedValue("AssertionError: expected 2 calls");
    renderDetail({ checks: [FAILING] });
    await waitFor(() =>
      expect(screen.getByText(/AssertionError/)).toBeInTheDocument(),
    );

    await userEvent.click(screen.getByTestId(`fix-with-agent-${FAILING.name}`));
    await flush();

    expect(mockHandOff).toHaveBeenCalledTimes(1);
    const arg = mockHandOff.mock.calls[0][0];
    expect(arg.task).toEqual({
      kind: "failing-check",
      checkName: FAILING.name,
      logExcerpt: "AssertionError: expected 2 calls",
      detailUrl: FAILING.detail_url,
    });
    expect(arg.prRef).toBe("example/codemux#172");
    expect(arg.pr.head_branch).toBe("agent/investigate-draft-mode");
    expect(arg.projectRoot).toBe("/repo");
    expect(arg.cli).toBe("gh");
    // Checked out here ⇒ the thread opens in this workspace, no worktree.
    expect(arg.currentWorkspaceId).toBe("ws-1");
  });

  it("says what the button does *here*, not what the mock says elsewhere", async () => {
    renderDetail({ checks: [FAILING] });
    await flush();
    expect(screen.getByText("opens a thread in this workspace")).toBeInTheDocument();

    cleanup();
    renderDetail({ checks: [FAILING], checkedOutHere: false });
    await flush();
    expect(
      screen.getByText("checks out the branch into a worktree, opens a thread"),
    ).toBeInTheDocument();
  });

  it("sends a review comment with its anchor, reviewer and body", async () => {
    renderDetail({
      inlineComments: [
        {
          id: 9,
          author: "juliusm",
          body: "This needs a line about the follow-up.",
          path: "AGENTS.md",
          line: 12,
          created_at: new Date().toISOString(),
          in_reply_to_id: null,
          pull_request_review_id: null,
        },
      ],
    });
    await flush();

    await userEvent.click(screen.getByTestId("send-thread-to-agent"));
    await flush();

    const arg = mockHandOff.mock.calls[0][0];
    expect(arg.task.kind).toBe("review-thread");
    expect(arg.task.reviewer).toBe("juliusm");
    expect(arg.task.path).toBe("AGENTS.md");
    expect(arg.task.line).toBe(12);
    expect(arg.task.body).toBe("This needs a line about the follow-up.");
  });

  it("offers Resolve with agent on the conflicted bar, left of Merge", async () => {
    renderDetail({
      pr: makePr({ mergeable: "CONFLICTING", merge_state_status: "DIRTY" }),
    });
    await flush();

    const button = screen.getByTestId("resolve-conflicts-with-agent");
    expect(button).toBeInTheDocument();
    // Rule 1: the primary slot still belongs to Merge.
    expect(screen.getByTestId("review-primary-action")).toHaveTextContent("Merge");
    expect(
      button.compareDocumentPosition(screen.getByTestId("review-primary-action")) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();

    await userEvent.click(button);
    await flush();
    expect(mockHandOff.mock.calls[0][0].task).toEqual({ kind: "conflicts" });
  });

  it("offers no handoff on a merged pull request", async () => {
    renderDetail({
      pr: makePr({ state: "MERGED", merged_by: "juliusm" }),
      checks: [FAILING],
      inlineComments: [
        {
          id: 9,
          author: "juliusm",
          body: "nit",
          path: "a.ts",
          line: 1,
          created_at: new Date().toISOString(),
          in_reply_to_id: null,
          pull_request_review_id: null,
        },
      ],
    });
    await flush();

    expect(screen.queryByText("Fix with agent")).not.toBeInTheDocument();
    expect(screen.queryByTestId("send-thread-to-agent")).not.toBeInTheDocument();
  });
});

export {};

// Keeps `ReactNode` referenced for the JSX pragma-free config.
export type _Node = ReactNode;

/**
 * The same component, given a project path and a number instead of a
 * workspace — which is all the Pull Requests page has.
 */
describe("without a workspace (the Pull Requests page)", () => {
  const pageProps = {
    workspaceId: null,
    checkedOutHere: false,
    showCheckout: true,
    onOpenChanges: undefined,
  };

  it("renders from (projectPath, prNumber) with a Check out button", () => {
    renderDetail({ ...pageProps, pr: makePr({ number: 285 }) });

    expect(screen.getByTestId("review-detail")).toBeInTheDocument();
    expect(screen.getByTestId("review-header")).toHaveTextContent("#285");
    expect(screen.getByTestId("detail-checkout")).toHaveTextContent("Check out");
    // Nothing here is standing in the branch, so the panel's tag is gone.
    expect(screen.queryByTestId("checked-out-here")).toBeNull();
  });

  it("offers Switch, and says so, when a workspace already has the branch", () => {
    renderDetail({ ...pageProps, existingWorkspaceId: "ws-7" });

    expect(screen.getByTestId("detail-checkout")).toHaveTextContent("Switch");
    expect(screen.getByTestId("checked-out-elsewhere")).toHaveTextContent("checked out");
  });

  it("checks the branch out into a worktree when there is nowhere to go", async () => {
    renderDetail({ ...pageProps });
    await userEvent.click(screen.getByTestId("detail-checkout"));

    expect(mockCheckOutPr).toHaveBeenCalledWith({
      projectRoot: "/repo",
      headBranch: "agent/investigate-draft-mode",
      prNumber: 172,
      existingWorkspaceId: undefined,
    });
  });

  it("switches to the workspace that has the branch instead of cutting a second one", async () => {
    renderDetail({ ...pageProps, existingWorkspaceId: "ws-7" });
    await userEvent.click(screen.getByTestId("detail-checkout"));

    expect(mockCheckOutPr).toHaveBeenCalledWith(
      expect.objectContaining({ existingWorkspaceId: "ws-7" }),
    );
  });

  it("keeps the action bar and its handoffs working", () => {
    renderDetail({
      ...pageProps,
      pr: makePr({ author: "juliusm" }),
      viewerLogin: VIEWER,
    });
    // Reviewer state: the verdict bar, not the merge bar.
    expect(screen.getByTestId("review-primary-action")).toBeInTheDocument();
  });

  it("does not offer the panel's branch controls back in the panel", () => {
    renderDetail({ workspaceId: "ws-1", checkedOutHere: true });
    expect(screen.queryByTestId("detail-checkout")).toBeNull();
    expect(screen.getByTestId("checked-out-here")).toBeInTheDocument();
  });
});
