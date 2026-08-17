/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, cleanup, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

// ── Mock Tauri commands ──

const mockGetPrTimeline = vi.fn();
const mockActivateTab = vi.fn().mockResolvedValue(undefined);

vi.mock("@/tauri/commands", () => ({
  getPrTimeline: (...a: unknown[]) => mockGetPrTimeline(...a),
  activateTab: (...a: unknown[]) => mockActivateTab(...a),
  submitPrReview: vi.fn().mockResolvedValue(undefined),
  submitPrReviewWithComments: vi.fn().mockResolvedValue(undefined),
  mergePullRequest: vi.fn().mockResolvedValue(undefined),
  setPrReady: vi.fn().mockResolvedValue(undefined),
  closePullRequest: vi.fn().mockResolvedValue(undefined),
  updatePullRequest: vi.fn().mockResolvedValue(undefined),
  requestPrReview: vi.fn().mockResolvedValue(undefined),
  getCheckLogExcerpt: vi.fn().mockResolvedValue(""),
  getPrReviewDiff: vi.fn().mockResolvedValue(""),
  checkoutDefaultBranchInWorkspace: vi.fn().mockResolvedValue("main"),
  gitPullChanges: vi.fn().mockResolvedValue(undefined),
  gitStashPush: vi.fn().mockResolvedValue(undefined),
}));

const mockActivateWorkspace = vi.fn().mockResolvedValue(undefined);
vi.mock("@/lib/perf/instrumented-activate", () => ({
  activateWorkspaceInteraction: (...a: unknown[]) => mockActivateWorkspace(...a),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@/lib/toast", () => ({
  toast: { info: vi.fn(), success: vi.fn(), warning: vi.fn(), error: vi.fn() },
}));

vi.mock("@/lib/pr-checkout", () => ({ checkOutPr: vi.fn() }));
vi.mock("@/lib/pr-agent-handoff", () => ({
  handOffToAgent: vi.fn(),
  findWorkspaceForBranch: vi.fn().mockReturnValue(null),
}));

import { ReviewDetail } from "./review-detail";
import { ReviewSubmitSheet } from "./review-submit-sheet";
import { _resetPrDrafts } from "./pr-drafts";
import { resolveProvider } from "@/lib/source-control";
import { ALL_OPERATIONS, NO_OPERATIONS } from "@/lib/provider-auth";
import { usePrAgentRunsStore } from "@/stores/pr-agent-runs-store";
import type { CheckInfo, PrTimelineEvent, PullRequestInfo } from "@/tauri/types";

const T0 = Date.now() - 4 * 60 * 60_000;
const at = (minutes: number) => new Date(T0 + minutes * 60_000).toISOString();

const GITLAB_OPERATIONS = { ...ALL_OPERATIONS, request_changes: false };

function makePr(over: Partial<PullRequestInfo> = {}): PullRequestInfo {
  return {
    number: 172,
    url: "https://github.com/example/codemux/pull/172",
    state: "OPEN",
    title: "feat: keep unsent drafts in the sidebar",
    head_branch: "agent/drafts",
    base_branch: "main",
    is_draft: false,
    mergeable: "MERGEABLE",
    additions: 1180,
    deletions: 33,
    review_decision: null,
    checks_passing: null,
    updated_at: at(120),
    created_at: at(0),
    body: "Body.",
    comments: [],
    totalComments: 0,
    author: "mock-dev",
    head_ref_oid: "abc123",
    head_repository_owner: "example",
    merge_state_status: "CLEAN",
    changed_files: 5,
    merged_by: null,
    merged_at: null,
    review_requests: [],
    latest_reviews: [],
    ...over,
  };
}

const HOST_EVENTS: PrTimelineEvent[] = [
  {
    id: "e1",
    actor: "juliusm",
    created_at: at(30),
    kind: "reviewed",
    verdict: "CHANGES_REQUESTED",
    body: "The discoverability leg is a separate follow-up.",
    anchor: "AGENTS.md:12",
  },
  {
    id: "e2",
    actor: "mock-dev",
    created_at: at(90),
    kind: "committed",
    sha: "a1f9c2e5d41b",
    message: "docs: note the follow-up",
  },
  {
    id: "e3",
    actor: "mock-dev",
    created_at: at(100),
    kind: "head_ref_force_pushed",
    sha: "bb31d70e9c2f",
  },
  // An event type this build has never seen.
  {
    id: "e4",
    actor: "mock-dev",
    created_at: at(110),
    kind: "other",
    label: "automatic base change succeeded",
  },
];

function check(name: string, conclusion: string): CheckInfo {
  return {
    name,
    status: conclusion === "pending" ? "IN_PROGRESS" : "COMPLETED",
    conclusion,
    elapsed_time: "1m",
    detail_url: null,
    started_at: null,
    completed_at: null,
  };
}

const CHECKS = [
  check("build", "pass"),
  check("lint", "pass"),
  check("test", "pass"),
  check("clippy", "pass"),
  check("e2e", "pending"),
];

function seedRun() {
  usePrAgentRunsStore.getState().record({
    id: "run-1",
    prRef: "example/codemux#172",
    projectRoot: "/repo",
    prNumber: 172,
    kind: "review-thread",
    summary: "Added the follow-up note and a pointer to the tracking issue.",
    workspaceId: "ws-agent",
    workspaceTitle: "this workspace",
    threadTabId: "tab-9",
    // Between the review and the push.
    createdAt: T0 + 60 * 60_000,
    files: 1,
    additions: 3,
    deletions: 0,
  });
}

function renderDetail(over: Record<string, unknown> = {}) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, refetchInterval: false } },
  });
  const props = {
    pr: makePr(),
    checks: CHECKS,
    reviews: [],
    inlineComments: [],
    checksLoading: false,
    commentsLoading: false,
    cwd: "/repo",
    workspaceId: "ws-1",
    projectRoot: "/repo",
    provider: resolveProvider("github"),
    operations: ALL_OPERATIONS,
    repoSlug: "example/codemux",
    viewerLogin: "mock-dev",
    checkedOutHere: true,
    gitBehind: 0,
    gitDirtyFiles: 0,
    staleAgeMs: null,
    onRefresh: vi.fn(),
    onOpenChanges: vi.fn(),
    ...over,
  };
  return render(
    <QueryClientProvider client={client}>
      {/* eslint-disable-next-line @typescript-eslint/no-explicit-any */}
      <ReviewDetail {...(props as any)} />
    </QueryClientProvider>,
  );
}

async function openTimeline(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByTestId("review-tab-timeline"));
  await waitFor(() => expect(screen.getByTestId("review-timeline")).toBeInTheDocument());
}

beforeEach(() => {
  _resetPrDrafts();
  usePrAgentRunsStore.getState().clear();
  mockGetPrTimeline.mockReset().mockResolvedValue(HOST_EVENTS);
  mockActivateWorkspace.mockClear();
  mockActivateTab.mockClear();
});

afterEach(cleanup);

describe("the Timeline tab", () => {
  it("renders an unknown host event as a plain one-liner instead of dropping it", async () => {
    const user = userEvent.setup();
    renderDetail();
    await openTimeline(user);

    const row = screen.getByTestId("timeline-entry-other");
    expect(row).toHaveTextContent("automatic base change succeeded");
    // A one-liner: no quoted card, no buttons — just who and what.
    expect(within(row).queryByRole("button")).toBeNull();
  });

  it("draws the force-push that the Code tab's re-anchoring keys on", async () => {
    const user = userEvent.setup();
    renderDetail();
    await openTimeline(user);

    const row = screen.getByTestId("timeline-entry-head_ref_force_pushed");
    expect(row).toHaveTextContent("force-pushed");
    expect(row).toHaveTextContent("bb31d70");
  });

  it("quotes a review comment with its file:line anchor", async () => {
    const user = userEvent.setup();
    renderDetail();
    await openTimeline(user);

    const row = screen.getByTestId("timeline-entry-reviewed");
    expect(row).toHaveTextContent("juliusm");
    expect(row).toHaveTextContent("requested changes");
    expect(row).toHaveTextContent("AGENTS.md:12");
  });

  it("ends on a checks row built from the live checks query", async () => {
    const user = userEvent.setup();
    renderDetail();
    await openTimeline(user);

    const rail = screen.getByTestId("review-timeline");
    const row = screen.getByTestId("timeline-entry-checks");
    expect(row).toHaveTextContent("4 checks passed, 1 running");
    expect(row).toHaveTextContent("now");
    // A running check spins.
    expect(within(row).getByTestId("timeline-dot-spinner")).toBeInTheDocument();
    // And it is the last thing on the rail.
    expect(rail.lastElementChild).toBe(row);
  });

  it("says nothing about checks when the query returned none", async () => {
    const user = userEvent.setup();
    renderDetail({ checks: [] });
    await openTimeline(user);
    expect(screen.queryByTestId("timeline-entry-checks")).toBeNull();
  });
});

describe("agent runs on the rail", () => {
  it("interleaves a local run between the host events it happened between", async () => {
    const user = userEvent.setup();
    seedRun();
    renderDetail();
    await openTimeline(user);

    const rail = screen.getByTestId("review-timeline");
    const order = Array.from(rail.querySelectorAll("[data-testid^='timeline-entry-']")).map(
      (el) => el.getAttribute("data-testid"),
    );
    expect(order).toEqual([
      "timeline-entry-opened",
      "timeline-entry-reviewed",
      "timeline-entry-agent",
      "timeline-entry-committed",
      "timeline-entry-head_ref_force_pushed",
      "timeline-entry-other",
      "timeline-entry-checks",
    ]);

    const run = screen.getByTestId("timeline-entry-agent");
    expect(run).toHaveTextContent("Agent run");
    expect(run).toHaveTextContent("addressed this thread");
    expect(run).toHaveTextContent("1 file · +3 · −0");
    expect(run).toHaveTextContent("in this workspace");
  });

  it("Host only removes the agent entry and moves nothing else", async () => {
    const user = userEvent.setup();
    seedRun();
    renderDetail();
    await openTimeline(user);

    const railOrder = () =>
      Array.from(
        screen.getByTestId("review-timeline").querySelectorAll("[data-testid^='timeline-entry-']"),
      ).map((el) => el.getAttribute("data-testid"));

    const before = railOrder();
    await user.click(screen.getByTestId("timeline-filter"));
    await user.click(await screen.findByText("Host only"));

    await waitFor(() => expect(screen.queryByTestId("timeline-entry-agent")).toBeNull());
    // Everything a teammate would see, in the order it was already in.
    expect(railOrder()).toEqual(before.filter((id) => id !== "timeline-entry-agent"));
  });

  it("Open thread activates the workspace and the tab the run landed in", async () => {
    const user = userEvent.setup();
    seedRun();
    renderDetail();
    await openTimeline(user);

    await user.click(screen.getByTestId("timeline-open-thread"));

    await waitFor(() => expect(mockActivateWorkspace).toHaveBeenCalledWith("ws-agent"));
    expect(mockActivateTab).toHaveBeenCalledWith("ws-agent", "tab-9");
  });

  it("omits the diff line for a run whose stats were never known", async () => {
    const user = userEvent.setup();
    usePrAgentRunsStore.getState().record({
      id: "run-2",
      prRef: "example/codemux#172",
      projectRoot: "/repo",
      prNumber: 172,
      kind: "failing-check",
      summary: "rust (ubuntu-latest)",
      workspaceId: "ws-agent",
      workspaceTitle: "drafts",
      threadTabId: null,
      createdAt: T0 + 60 * 60_000,
    });
    renderDetail();
    await openTimeline(user);

    const run = screen.getByTestId("timeline-entry-agent");
    expect(run).toHaveTextContent("fixed a failing check");
    // Never an invented "0 files".
    expect(run).not.toHaveTextContent("file ·");
  });
});

// ── 4d: controls render from the declaration ──

describe("per-operation capabilities", () => {
  it("hides Request changes on GitLab in the action bar", () => {
    renderDetail({
      provider: resolveProvider("gitlab"),
      operations: GITLAB_OPERATIONS,
      pr: makePr({ author: "someone-else" }),
    });

    expect(screen.getByTestId("verdict-approve")).toBeInTheDocument();
    expect(screen.queryByTestId("verdict-request-changes")).toBeNull();
  });

  it("hides Request changes on GitLab in the verdict sheet too", () => {
    // The sheet is rendered directly rather than driven through a
    // pending note: what is under test is that both surfaces read the
    // same declaration, not the route that opens the sheet (which
    // review-code-tab.test.tsx already walks).
    render(
      <ReviewSubmitSheet
        open
        prNumber={172}
        draftKey="ws-1:172"
        drafts={[]}
        initialBody=""
        initialVerdict="comment"
        canRequestChanges={GITLAB_OPERATIONS.request_changes}
        canApprove={GITLAB_OPERATIONS.approve}
        submitting={false}
        blockedReason={null}
        onReanchor={vi.fn()}
        onCancel={vi.fn()}
        onSubmit={vi.fn()}
      />,
    );

    expect(screen.getByTestId("verdict-option-comment")).toBeInTheDocument();
    expect(screen.getByTestId("verdict-option-approve")).toBeInTheDocument();
    expect(screen.queryByTestId("verdict-option-request-changes")).toBeNull();
  });

  it("draws no Timeline tab for a host that does not declare it", () => {
    renderDetail({ operations: { ...ALL_OPERATIONS, timeline: false } });
    expect(screen.queryByTestId("review-tab-timeline")).toBeNull();
    // The tabs that remain are unaffected.
    expect(screen.getByTestId("review-tab-summary")).toBeInTheDocument();
    expect(screen.getByTestId("review-tab-code")).toBeInTheDocument();
  });

  it("hides merge for a host that does not declare it, keeping the sentence", () => {
    renderDetail({
      operations: { ...ALL_OPERATIONS, merge_with_strategies: false },
      checks: [check("build", "pass")],
    });
    expect(screen.queryByTestId("merge-strategy-picker")).toBeNull();
    expect(screen.getByTestId("review-action-bar")).toBeInTheDocument();
  });

  it("hides draft and close controls where state changes are undeclared", () => {
    renderDetail({
      operations: { ...ALL_OPERATIONS, draft_ready_close_reopen: false },
      pr: makePr({ is_draft: true }),
    });
    expect(screen.queryByText("Ready for review")).toBeNull();
    expect(screen.queryByText("Close")).toBeNull();
  });

  it("gives an undeclared host the read-only sentence and no write control anywhere", () => {
    renderDetail({
      provider: resolveProvider("bitbucket"),
      operations: NO_OPERATIONS,
      pr: makePr({
        url: "https://bitbucket.org/acme/ledger-api/pull-requests/64",
        author: "someone-else",
      }),
    });

    // 4b's sentence, plus the browser.
    const state = screen.getByTestId("empty-unsupported");
    expect(state).toHaveTextContent("Bitbucket, read-only");
    expect(state).toHaveTextContent("can't review or merge it here yet");
    expect(within(state).getByText("Open in browser")).toBeInTheDocument();

    // Not one write control is drawn — not disabled, not present.
    for (const id of [
      "review-action-bar",
      "verdict-approve",
      "verdict-request-changes",
      "review-primary-action",
      "review-composer",
      "review-composer-collapsed",
      "merge-strategy-picker",
      "review-tab-timeline",
      "review-tab-code",
    ]) {
      expect(screen.queryByTestId(id)).toBeNull();
    }
    expect(screen.queryByText("Merge")).toBeNull();
    expect(screen.queryByText("Request review")).toBeNull();
  });
});
