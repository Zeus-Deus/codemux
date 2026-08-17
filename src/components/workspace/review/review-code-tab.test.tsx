/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, cleanup, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

// ── Mock Tauri commands ──

const mockGetPrReviewDiff = vi.fn();
const mockSubmitWithComments = vi.fn().mockResolvedValue(undefined);
const mockAddInlineComment = vi.fn().mockResolvedValue(undefined);

vi.mock("@/tauri/commands", () => ({
  getPrReviewDiff: (...a: unknown[]) => mockGetPrReviewDiff(...a),
  submitPrReviewWithComments: (...a: unknown[]) => mockSubmitWithComments(...a),
  addPrInlineComment: (...a: unknown[]) => mockAddInlineComment(...a),
  submitPrReview: vi.fn().mockResolvedValue(undefined),
  mergePullRequest: vi.fn().mockResolvedValue(undefined),
  setPrReady: vi.fn().mockResolvedValue(undefined),
  closePullRequest: vi.fn().mockResolvedValue(undefined),
  updatePullRequest: vi.fn().mockResolvedValue(undefined),
  requestPrReview: vi.fn().mockResolvedValue(undefined),
  getCheckLogExcerpt: vi.fn().mockResolvedValue(""),
  checkoutDefaultBranchInWorkspace: vi.fn().mockResolvedValue("main"),
  gitPullChanges: vi.fn().mockResolvedValue(undefined),
  gitStashPush: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@/lib/toast", () => ({
  toast: { info: vi.fn(), success: vi.fn(), warning: vi.fn(), error: vi.fn() },
}));

vi.mock("@/lib/pr-checkout", () => ({ checkOutPr: vi.fn().mockResolvedValue(undefined) }));
vi.mock("@/lib/pr-agent-handoff", () => ({
  handOffToAgent: vi.fn().mockResolvedValue({ route: "current", workspaceId: "ws-1" }),
  findWorkspaceForBranch: vi.fn().mockReturnValue(null),
}));

import { ReviewDetail, _resetHeadOidTracking } from "./review-detail";
import { _resetPrDrafts } from "./pr-drafts";
import { resolveProvider } from "@/lib/source-control";
import { ALL_OPERATIONS } from "@/lib/provider-auth";

/** GitLab's declaration: everything but the verdict it does not have. */
const GITLAB_OPERATIONS = { ...ALL_OPERATIONS, request_changes: false };
/** GitLab has no line-comment route this build can use yet. */
const GITLAB_NO_LINE_COMMENTS = { ...GITLAB_OPERATIONS, line_comments: false };
import type { PullRequestInfo } from "@/tauri/types";

// ── Fixture diff ──
//
// RIGHT: 10 `const a = 1;`, 11 `const b = 3;`, 12 `const c = 4;`, 13 `const d = 5;`
// LEFT:  10 `const a = 1;`, 11 `const b = 2;`, 12 `const d = 5;`
const SMALL_FILE = [
  "diff --git a/src/a.ts b/src/a.ts",
  "index 111..222 100644",
  "--- a/src/a.ts",
  "+++ b/src/a.ts",
  "@@ -10,3 +10,4 @@ header",
  " const a = 1;",
  "-const b = 2;",
  "+const b = 3;",
  "+const c = 4;",
  " const d = 5;",
].join("\n");

const BINARY_FILE = [
  "diff --git a/icons/app.png b/icons/app.png",
  "index 333..444 100644",
  "Binary files a/icons/app.png and b/icons/app.png differ",
].join("\n");

function bigFile(): string {
  const rows: string[] = [];
  for (let i = 0; i < 1_200; i++) rows.push(`-const x${i} = ${i};`);
  for (let i = 0; i < 1_200; i++) rows.push(`+const x${i} = ${i + 1};`);
  return [
    "diff --git a/src/big.ts b/src/big.ts",
    "--- a/src/big.ts",
    "+++ b/src/big.ts",
    "@@ -1,1200 +1,1200 @@",
    ...rows,
  ].join("\n");
}

const DIFF = [SMALL_FILE, bigFile(), BINARY_FILE].join("\n");

/** The same PR force-pushed: two lines inserted, `const c` rewritten. */
const DIFF_AFTER = [
  [
    "diff --git a/src/a.ts b/src/a.ts",
    "index 111..999 100644",
    "--- a/src/a.ts",
    "+++ b/src/a.ts",
    "@@ -10,3 +10,6 @@ header",
    " const a = 1;",
    "+const zero = 0;",
    "+const half = 0.5;",
    "-const b = 2;",
    "+const b = 3;",
    "+const c = 4 as const;",
    " const d = 5;",
  ].join("\n"),
  bigFile(),
  BINARY_FILE,
].join("\n");

const VIEWER = "mock-dev";

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
    updated_at: new Date().toISOString(),
    created_at: null,
    body: "Body.",
    comments: [],
    totalComments: 0,
    author: "someone-else",
    head_ref_oid: "head-one",
    head_repository_owner: "example",
    merge_state_status: "CLEAN",
    changed_files: 3,
    merged_by: null,
    merged_at: null,
    review_requests: [],
    latest_reviews: [],
    ...over,
  };
}

function renderDetail(over: Record<string, unknown> = {}) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, refetchInterval: false } },
  });
  const props = {
    pr: makePr(),
    checks: [],
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
      <ReviewDetail {...(props as Parameters<typeof ReviewDetail>[0])} />
    </QueryClientProvider>,
  );
  return {
    ...utils,
    rerenderWith: (next: Record<string, unknown>) =>
      utils.rerender(
        <QueryClientProvider client={client}>
          <ReviewDetail {...({ ...props, ...next } as Parameters<typeof ReviewDetail>[0])} />
        </QueryClientProvider>,
      ),
  };
}

async function openCodeTab(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByTestId("review-tab-code"));
  await screen.findAllByTestId("code-file");
}

const gutter = (id: string) =>
  document.querySelector<HTMLElement>(`[data-diff-line="${id}"]`)!;

beforeEach(() => {
  _resetPrDrafts();
  _resetHeadOidTracking();
  mockGetPrReviewDiff.mockReset().mockResolvedValue(DIFF);
  mockSubmitWithComments.mockReset().mockResolvedValue(undefined);
  mockAddInlineComment.mockReset().mockResolvedValue(undefined);
});

afterEach(cleanup);

describe("the Code tab", () => {
  it("counts the changed files on the strip and lists every file", async () => {
    const user = userEvent.setup();
    renderDetail();
    expect(screen.getByTestId("review-tab-code")).toHaveTextContent("Code3");
    await openCodeTab(user);
    const paths = screen
      .getAllByTestId("code-file")
      .map((el) => el.getAttribute("data-file-path"));
    expect(paths).toEqual(["src/a.ts", "src/big.ts", "icons/app.png"]);
  });

  it("lists a binary file without rendering it", async () => {
    const user = userEvent.setup();
    renderDetail();
    await openCodeTab(user);
    const binary = screen
      .getAllByTestId("code-file")
      .find((el) => el.getAttribute("data-file-path") === "icons/app.png")!;
    expect(within(binary).getByTestId("file-not-rendered")).toHaveTextContent("Binary file");
    expect(binary.querySelectorAll("[data-diff-line]")).toHaveLength(0);
  });

  it("holds back a 2,000-line file behind Load anyway", async () => {
    const user = userEvent.setup();
    renderDetail();
    await openCodeTab(user);
    const big = screen
      .getAllByTestId("code-file")
      .find((el) => el.getAttribute("data-file-path") === "src/big.ts")!;
    expect(within(big).getByTestId("file-too-large")).toHaveTextContent("2,400 changed lines");
    expect(big.querySelectorAll("[data-diff-line]")).toHaveLength(0);

    await user.click(within(big).getByTestId("load-anyway"));
    expect(big.querySelectorAll("[data-diff-line]").length).toBeGreaterThan(100);
  });

  it("collapses a file when it is marked viewed", async () => {
    const user = userEvent.setup();
    renderDetail();
    await openCodeTab(user);
    const file = screen.getAllByTestId("code-file")[0];
    expect(file.querySelectorAll("[data-diff-line]").length).toBeGreaterThan(0);
    await user.click(within(file).getByTestId("viewed-toggle"));
    await waitFor(() =>
      expect(file.querySelectorAll("[data-diff-line]")).toHaveLength(0),
    );
    expect(file).toHaveAttribute("data-viewed", "true");
  });
});

describe("selecting lines", () => {
  it("opens the composer on a single line", async () => {
    const user = userEvent.setup();
    renderDetail();
    await openCodeTab(user);
    await user.click(gutter("RIGHT:11"));
    expect(screen.getByTestId("line-composer")).toHaveTextContent("line 11");
    expect(document.querySelectorAll('[data-selected="true"]')).toHaveLength(1);
  });

  it("extends to a range on shift-click and records start and end", async () => {
    const user = userEvent.setup();
    renderDetail();
    await openCodeTab(user);
    await user.click(gutter("RIGHT:11"));
    await user.keyboard("{Shift>}");
    await user.click(gutter("RIGHT:12"));
    await user.keyboard("{/Shift}");

    expect(screen.getByTestId("line-composer")).toHaveTextContent("lines 11–12");
    expect(document.querySelectorAll('[data-selected="true"]')).toHaveLength(2);

    await user.type(screen.getByTestId("line-composer-body"), "Both of these.");
    await user.click(screen.getByTestId("add-to-review"));

    await user.click(screen.getByTestId("open-submit-sheet"));
    await user.type(screen.getByTestId("submit-sheet-body"), "Summary.");
    await user.click(screen.getByTestId("submit-review-confirm"));
    await waitFor(() => expect(mockSubmitWithComments).toHaveBeenCalled());
    const [, , , , comments] = mockSubmitWithComments.mock.calls[0];
    expect(comments[0]).toMatchObject({
      file: "src/a.ts",
      side: "RIGHT",
      line: 12,
      start_line: 11,
    });
  });

  it("keeps what you typed when shift-click widens the range under it", async () => {
    // The composer is mounted beneath the *end* of the selection, so
    // extending the range relocates it — and React remounts a relocated
    // component. Everything typed used to go with it, which is the one
    // thing this surface promises never happens.
    const user = userEvent.setup();
    renderDetail();
    await openCodeTab(user);
    await user.click(gutter("RIGHT:11"));
    await user.type(screen.getByTestId("line-composer-body"), "Half a thought");

    await user.keyboard("{Shift>}");
    await user.click(gutter("RIGHT:12"));
    await user.keyboard("{/Shift}");

    expect(screen.getByTestId("line-composer")).toHaveTextContent("lines 11–12");
    expect(screen.getByTestId("line-composer-body")).toHaveValue("Half a thought");
  });

  it("starts empty on a different line, rather than carrying the last note over", async () => {
    const user = userEvent.setup();
    renderDetail();
    await openCodeTab(user);
    await user.click(gutter("RIGHT:11"));
    await user.type(screen.getByTestId("line-composer-body"), "About eleven");

    // A plain click somewhere else is a different note.
    await user.click(gutter("RIGHT:13"));
    expect(screen.getByTestId("line-composer-body")).toHaveValue("");
  });

  it("anchors a note on a deleted line to the LEFT side", async () => {
    const user = userEvent.setup();
    renderDetail();
    await openCodeTab(user);
    await user.click(gutter("LEFT:11"));
    await user.type(screen.getByTestId("line-composer-body"), "Why did this go?");
    await user.click(screen.getByTestId("add-to-review"));

    await user.click(screen.getByTestId("open-submit-sheet"));
    await user.type(screen.getByTestId("submit-sheet-body"), "Summary.");
    await user.click(screen.getByTestId("submit-review-confirm"));
    await waitFor(() => expect(mockSubmitWithComments).toHaveBeenCalled());
    const [, , , , comments] = mockSubmitWithComments.mock.calls[0];
    expect(comments[0]).toMatchObject({ side: "LEFT", line: 11, start_line: null });
  });

  it("still shows a LEFT context-line note after switching to unified", async () => {
    // Split draws a context line in both columns and anchors the left
    // one as (LEFT, oldLine); unified draws it once and calls it
    // (RIGHT, newLine). Matching a note on the passed side alone made a
    // note written in split disappear entirely in unified — while still
    // submitting against the host perfectly well.
    const user = userEvent.setup();
    renderDetail();
    await openCodeTab(user);
    await user.click(screen.getByTestId("diff-layout-split"));

    // `const a = 1;` is context: old 10 on the left, new 10 on the right.
    await user.click(gutter("LEFT:10"));
    await user.type(screen.getByTestId("line-composer-body"), "Context note.");
    await user.click(screen.getByTestId("add-to-review"));
    expect(screen.getByTestId("pending-note")).toHaveTextContent("Context note.");

    await user.click(screen.getByTestId("diff-layout-unified"));
    expect(screen.getByTestId("pending-note")).toHaveTextContent("Context note.");
  });

  it("escape drops the selection", async () => {
    const user = userEvent.setup();
    renderDetail();
    await openCodeTab(user);
    await user.click(gutter("RIGHT:11"));
    expect(screen.getByTestId("line-composer")).toBeInTheDocument();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByTestId("line-composer")).toBeNull());
  });

  it("posts one comment immediately when asked to", async () => {
    const user = userEvent.setup();
    renderDetail();
    await openCodeTab(user);
    await user.click(gutter("RIGHT:11"));
    await user.type(screen.getByTestId("line-composer-body"), "Ship it.");
    await user.click(screen.getByTestId("comment-now"));

    await waitFor(() => expect(mockAddInlineComment).toHaveBeenCalled());
    expect(mockAddInlineComment).toHaveBeenCalledWith(
      "/repo",
      172,
      { file: "src/a.ts", body: "Ship it.", side: "RIGHT", line: 11, start_line: null },
      "head-one",
    );
    // Nothing pending: it was published, not drafted.
    expect(screen.queryByTestId("draft-footer")).toBeNull();
  });

  it("offers no line composer at all where line comments are undeclared", async () => {
    // GitLab has line comments; this build cannot post them (it needs a
    // version triple it doesn't build). So the adapter declares the
    // operation false, and the diff stays readable but inert — drafting
    // a note that can only fail at submit is worse than not offering
    // one, because the work is lost at the last step.
    const user = userEvent.setup();
    renderDetail({
      provider: resolveProvider("gitlab"),
      operations: GITLAB_NO_LINE_COMMENTS,
    });
    await openCodeTab(user);
    await user.click(gutter("RIGHT:11"));
    expect(screen.queryByTestId("add-to-review")).toBeNull();
    expect(screen.queryByTestId("comment-now")).toBeNull();
    // The diff itself is still fully readable.
    expect(gutter("RIGHT:11")).toBeInTheDocument();
  });
});

describe("the pending review", () => {
  it("shows the note inline and counts it in the footer", async () => {
    const user = userEvent.setup();
    renderDetail();
    await openCodeTab(user);
    await user.click(gutter("RIGHT:11"));
    await user.type(screen.getByTestId("line-composer-body"), "Name this constant.");
    await user.click(screen.getByTestId("add-to-review"));

    expect(screen.getByTestId("pending-note")).toHaveTextContent("Name this constant.");
    const footer = screen.getByTestId("draft-footer");
    expect(footer).toHaveTextContent("1 pending");
    expect(footer).toHaveTextContent("on 1 file · not visible to anyone yet");
  });

  it("says 3 pending on 2 files, and survives a walk to Summary and back", async () => {
    const user = userEvent.setup();
    renderDetail();
    await openCodeTab(user);

    for (const [id, text] of [
      ["RIGHT:11", "One."],
      ["RIGHT:12", "Two."],
    ] as const) {
      await user.click(gutter(id));
      await user.type(screen.getByTestId("line-composer-body"), text);
      await user.click(screen.getByTestId("add-to-review"));
    }
    const big = screen
      .getAllByTestId("code-file")
      .find((el) => el.getAttribute("data-file-path") === "src/big.ts")!;
    await user.click(within(big).getByTestId("load-anyway"));
    await user.click(gutter("RIGHT:1"));
    await user.type(screen.getByTestId("line-composer-body"), "Three.");
    await user.click(screen.getByTestId("add-to-review"));

    expect(screen.getByTestId("draft-footer")).toHaveTextContent("3 pending");
    expect(screen.getByTestId("draft-footer")).toHaveTextContent("on 2 files");

    // Binding rule 4: a tab switch discards nothing.
    await user.click(screen.getByTestId("review-tab-summary"));
    expect(screen.getByTestId("draft-footer")).toHaveTextContent("3 pending");
    await user.click(screen.getByTestId("review-tab-code"));
    expect(screen.getByTestId("draft-footer")).toHaveTextContent("3 pending");

    // The two on the small file are back in place. The third is on the
    // big file, which comes back held behind Load anyway — so its header
    // carries the count instead of hiding the note entirely.
    expect(await screen.findAllByTestId("pending-note")).toHaveLength(2);
    const bigAgain = screen
      .getAllByTestId("code-file")
      .find((el) => el.getAttribute("data-file-path") === "src/big.ts")!;
    expect(within(bigAgain).getByTestId("file-pending-count")).toHaveTextContent("1 pending");
    await user.click(within(bigAgain).getByTestId("load-anyway"));
    expect(screen.getAllByTestId("pending-note")).toHaveLength(3);
    // Renders the 2,400-row file twice and types into it, which is the
    // point — this is the case where a draft would be lost. It runs in
    // well under two seconds alone but has gone over the 5s default on a
    // loaded CI worker, so it gets a timeout that reflects the work
    // rather than a flake that reflects the machine.
  }, 20_000);

  it("asks before discarding, then clears", async () => {
    const user = userEvent.setup();
    renderDetail();
    await openCodeTab(user);
    await user.click(gutter("RIGHT:11"));
    await user.type(screen.getByTestId("line-composer-body"), "A note.");
    await user.click(screen.getByTestId("add-to-review"));

    await user.click(screen.getByTestId("discard-drafts"));
    expect(screen.getByTestId("draft-footer")).toHaveTextContent("Discard it?");
    await user.click(screen.getByTestId("discard-confirm"));
    await waitFor(() => expect(screen.queryByTestId("draft-footer")).toBeNull());
    expect(screen.queryByTestId("pending-note")).toBeNull();
  });

  it("keeps each PR's notes to itself", async () => {
    const user = userEvent.setup();
    const { rerenderWith } = renderDetail();
    await openCodeTab(user);
    await user.click(gutter("RIGHT:11"));
    await user.type(screen.getByTestId("line-composer-body"), "On 172.");
    await user.click(screen.getByTestId("add-to-review"));

    rerenderWith({ pr: makePr({ number: 285, head_ref_oid: "other-head" }) });
    await waitFor(() => expect(screen.queryByTestId("draft-footer")).toBeNull());

    rerenderWith({ pr: makePr() });
    await waitFor(() =>
      expect(screen.getByTestId("draft-footer")).toHaveTextContent("1 pending"),
    );
  });

  it("does not carry an unfinished composer over to another PR", async () => {
    // The draft that survives relocation must not survive the pull
    // request. The tab holds it in a ref and the detail is not keyed by
    // PR, so without a fresh mount per draft key the paragraph typed
    // about #172 would seed the composer on #285 the moment a row at the
    // same coordinates existed — which, on the same fixture diff, it does.
    const user = userEvent.setup();
    const { rerenderWith } = renderDetail();
    await openCodeTab(user);
    await user.click(gutter("RIGHT:11"));
    await user.type(screen.getByTestId("line-composer-body"), "Meant for 172.");

    rerenderWith({ pr: makePr({ number: 285, head_ref_oid: "other-head" }) });
    await openCodeTab(user);
    // The selection is #172's too, so #285 opens with no composer at all.
    expect(screen.queryByTestId("line-composer")).toBeNull();

    await user.click(gutter("RIGHT:11"));
    expect(screen.getByTestId("line-composer-body")).toHaveValue("");
  });
});

describe("submitting", () => {
  async function withOneNote(user: ReturnType<typeof userEvent.setup>) {
    await openCodeTab(user);
    await user.click(gutter("RIGHT:11"));
    await user.type(screen.getByTestId("line-composer-body"), "Name this constant.");
    await user.click(screen.getByTestId("add-to-review"));
  }

  it("sends verdict, body and notes as one request", async () => {
    const user = userEvent.setup();
    renderDetail();
    await withOneNote(user);

    await user.click(screen.getByTestId("open-submit-sheet"));
    expect(screen.getByTestId("submit-sheet-notes")).toHaveTextContent("Name this constant.");
    await user.click(screen.getByTestId("verdict-option-request-changes"));
    await user.type(screen.getByTestId("submit-sheet-body"), "One thing to fix.");
    await user.click(screen.getByTestId("submit-review-confirm"));

    await waitFor(() => expect(mockSubmitWithComments).toHaveBeenCalledTimes(1));
    expect(mockSubmitWithComments).toHaveBeenCalledWith(
      "/repo",
      172,
      "request-changes",
      "One thing to fix.",
      [
        {
          file: "src/a.ts",
          body: "Name this constant.",
          side: "RIGHT",
          line: 11,
          start_line: null,
        },
      ],
      "head-one",
    );
    await waitFor(() => expect(screen.queryByTestId("draft-footer")).toBeNull());
  });

  it("sends the pending notes when the verdict comes from the action bar", async () => {
    // The bar and the sheet are two doors onto the same act. The bar
    // used to take the plain review route, so approving from it
    // published the verdict and stranded every line note in the draft
    // store — invisible to the author, still pending here.
    const user = userEvent.setup();
    renderDetail();
    await withOneNote(user);

    await user.click(screen.getByTestId("verdict-approve"));

    await waitFor(() => expect(mockSubmitWithComments).toHaveBeenCalledTimes(1));
    const [, , verdict, , comments] = mockSubmitWithComments.mock.calls[0];
    expect(verdict).toBe("approve");
    expect(comments).toHaveLength(1);
    expect(comments[0]).toMatchObject({ file: "src/a.ts", line: 11 });
    await waitFor(() => expect(screen.queryByTestId("draft-footer")).toBeNull());
  });

  it("keeps the sheet's body in the draft pool across a reopen", async () => {
    // The sheet seeded itself from the draft store and never wrote back,
    // so its text lived only in component state and a Cancel — or a
    // Re-anchor, or a failure — took it with it.
    const user = userEvent.setup();
    renderDetail();
    await withOneNote(user);

    await user.click(screen.getByTestId("open-submit-sheet"));
    await user.type(screen.getByTestId("submit-sheet-body"), "Nearly done.");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByTestId("submit-sheet")).toBeNull());

    await user.click(screen.getByTestId("open-submit-sheet"));
    expect(screen.getByTestId("submit-sheet-body")).toHaveValue("Nearly done.");
  });

  it("won't send a wordless comment, and says why", async () => {
    // The host rejects a COMMENT or REQUEST_CHANGES review with no body,
    // and the command layer refuses it before it gets there. Both doors
    // say so before the click rather than after it. Line notes do not
    // count as the body — they hang off the review, they are not it.
    const user = userEvent.setup();
    renderDetail();
    await withOneNote(user);

    expect(screen.getByTestId("review-primary-action")).toBeDisabled();
    expect(screen.getByTestId("verdict-request-changes")).toBeDisabled();
    // Approving is allowed to be wordless: the approval is the statement.
    expect(screen.getByTestId("verdict-approve")).toBeEnabled();

    await user.click(screen.getByTestId("open-submit-sheet"));
    expect(screen.getByTestId("submit-body-required")).toHaveTextContent(
      "A comment review needs a message.",
    );
    expect(screen.getByTestId("submit-review-confirm")).toBeDisabled();

    await user.type(screen.getByTestId("submit-sheet-body"), "Two small things.");
    expect(screen.queryByTestId("submit-body-required")).toBeNull();
    await user.click(screen.getByTestId("submit-review-confirm"));
    await waitFor(() => expect(mockSubmitWithComments).toHaveBeenCalledTimes(1));
  });

  it("does not offer Request changes on GitLab", async () => {
    const user = userEvent.setup();
    renderDetail({
      provider: resolveProvider("gitlab"),
      operations: GITLAB_OPERATIONS,
    });
    await withOneNote(user);
    await user.click(screen.getByTestId("open-submit-sheet"));
    expect(screen.getByTestId("verdict-option-comment")).toBeInTheDocument();
    expect(screen.getByTestId("verdict-option-approve")).toBeInTheDocument();
    expect(screen.queryByTestId("verdict-option-request-changes")).toBeNull();
  });

  it("keeps the notes and offers Retry when the send fails", async () => {
    const user = userEvent.setup();
    mockSubmitWithComments.mockRejectedValue("host unreachable");
    renderDetail();
    await withOneNote(user);

    await user.click(screen.getByTestId("open-submit-sheet"));
    await user.type(screen.getByTestId("submit-sheet-body"), "Summary.");
    await user.click(screen.getByTestId("submit-review-confirm"));

    const notice = await screen.findByTestId("drift-notice");
    expect(notice).toHaveAttribute("data-drift-kind", "submit-failed");
    expect(notice).toHaveTextContent("host unreachable");
    expect(notice).toHaveTextContent("your 1 note are still here");
    expect(screen.getByTestId("draft-footer")).toHaveTextContent("1 pending");

    mockSubmitWithComments.mockResolvedValue(undefined);
    await user.click(within(notice).getByText("Retry"));
    await waitFor(() => expect(mockSubmitWithComments).toHaveBeenCalledTimes(2));
  });

  it("blocks the send while a note no longer matches a line", async () => {
    const user = userEvent.setup();
    const { rerenderWith } = renderDetail();
    await openCodeTab(user);
    await user.click(gutter("RIGHT:12"));
    await user.type(screen.getByTestId("line-composer-body"), "About to be rewritten.");
    await user.click(screen.getByTestId("add-to-review"));

    mockGetPrReviewDiff.mockResolvedValue(DIFF_AFTER);
    rerenderWith({ pr: makePr({ head_ref_oid: "head-two" }) });
    await screen.findByTestId("unanchored-notes");

    await user.click(screen.getByTestId("open-submit-sheet"));
    expect(screen.getByTestId("submit-blocked-reason")).toHaveTextContent(
      "no longer match a line",
    );
    expect(screen.queryByTestId("submit-review-confirm")).toBeNull();
    expect(screen.getByTestId("submit-reanchor")).toBeInTheDocument();
    expect(mockSubmitWithComments).not.toHaveBeenCalled();
  });
});

describe("after a force-push", () => {
  async function forcePushWithNotes() {
    const user = userEvent.setup();
    const { rerenderWith } = renderDetail();
    await openCodeTab(user);
    // 11 survives and slides to 13; 12 is rewritten and is lost.
    for (const [id, text] of [
      ["RIGHT:11", "This one survives."],
      ["RIGHT:12", "This one does not."],
    ] as const) {
      await user.click(gutter(id));
      await user.type(screen.getByTestId("line-composer-body"), text);
      await user.click(screen.getByTestId("add-to-review"));
    }
    mockGetPrReviewDiff.mockResolvedValue(DIFF_AFTER);
    rerenderWith({ pr: makePr({ head_ref_oid: "head-two" }) });
    return user;
  }

  it("moves what it can, labels what it can't, and says so in the notice", async () => {
    await forcePushWithNotes();

    const notice = await screen.findByTestId("drift-notice");
    expect(notice).toHaveAttribute("data-drift-kind", "force-pushed");
    expect(notice).toHaveTextContent("Force-pushed");
    expect(notice).toHaveTextContent("1 of your 2 notes no longer matches a line");
    expect(within(notice).getByText("Show on old diff")).toBeInTheDocument();
    expect(within(notice).getByText("Re-anchor")).toBeInTheDocument();

    await waitFor(() =>
      expect(screen.getByTestId("unanchored-notes")).toHaveTextContent(
        "1 note no longer matches a line",
      ),
    );
    expect(screen.getByTestId("unanchored-notes")).toHaveTextContent("This one does not.");

    const moved = screen.getByTestId("note-moved-badge");
    expect(moved).toHaveTextContent("moved 11 → 13");
    expect(screen.getByTestId("draft-footer")).toHaveTextContent("2 pending");
    expect(screen.getByTestId("draft-footer")).toHaveTextContent("1 unanchored");
  });

  it("re-pins a lost note onto a line you pick", async () => {
    const user = await forcePushWithNotes();
    await screen.findByTestId("unanchored-notes");

    await user.click(screen.getByTestId("repin-note"));
    expect(screen.getByTestId("repin-banner")).toHaveTextContent("Click the line this note");

    await user.click(gutter("RIGHT:14"));
    await waitFor(() => expect(screen.queryByTestId("unanchored-notes")).toBeNull());

    await user.click(screen.getByTestId("open-submit-sheet"));
    expect(screen.queryByTestId("submit-blocked-reason")).toBeNull();
    await user.type(screen.getByTestId("submit-sheet-body"), "Summary.");
    await user.click(screen.getByTestId("submit-review-confirm"));
    await waitFor(() => expect(mockSubmitWithComments).toHaveBeenCalled());
    const [, , , , comments, commitId] = mockSubmitWithComments.mock.calls[0];
    expect(commitId).toBe("head-two");
    expect(comments.map((c: { line: number }) => c.line).sort()).toEqual([13, 14]);
  });

  it("stands down once every note has found a line again", async () => {
    // The notice exists to get lost notes re-anchored. Its `changedAt`
    // used to be cleared only by a merge, so it sat on the panel — and
    // on the one notice slot — for the rest of the session.
    const user = await forcePushWithNotes();
    await screen.findByTestId("unanchored-notes");
    expect(await screen.findByTestId("drift-notice")).toHaveAttribute(
      "data-drift-kind",
      "force-pushed",
    );

    await user.click(screen.getByTestId("repin-note"));
    await user.click(gutter("RIGHT:14"));

    await waitFor(() => expect(screen.queryByTestId("drift-notice")).toBeNull());
  });

  it("does not dismiss another PR's force-push notice on the way past", async () => {
    // The auto-acknowledge asks "were there unanchored notes a moment
    // ago, and are there none now?". `ReviewDetail` is not keyed by pull
    // request, so arriving at a PR with no notes — from one that had
    // unanchored ones — read as "they were all just re-anchored", and
    // dismissed a force-push notice the user had never seen. The flag
    // now remembers which pull request raised it.
    const user = userEvent.setup();

    // #285 is seen first, so a later head change on it reads as a
    // force-push rather than a first sighting.
    const { rerenderWith } = renderDetail({
      pr: makePr({ number: 285, head_ref_oid: "285-head-one" }),
    });

    // Over to #172: two notes, then a force-push that loses one.
    rerenderWith({ pr: makePr() });
    await openCodeTab(user);
    for (const [id, text] of [
      ["RIGHT:11", "This one survives."],
      ["RIGHT:12", "This one does not."],
    ] as const) {
      await user.click(gutter(id));
      await user.type(screen.getByTestId("line-composer-body"), text);
      await user.click(screen.getByTestId("add-to-review"));
    }
    mockGetPrReviewDiff.mockResolvedValue(DIFF_AFTER);
    rerenderWith({ pr: makePr({ head_ref_oid: "head-two" }) });
    await screen.findByTestId("unanchored-notes");

    // Back to #285, which has force-pushed too and has no notes at all.
    rerenderWith({ pr: makePr({ number: 285, head_ref_oid: "285-head-two" }) });

    const notice = await screen.findByTestId("drift-notice");
    expect(notice).toHaveAttribute("data-drift-kind", "force-pushed");
  });

  it("lets a failed submit through instead of hiding it behind the force-push", async () => {
    // Ranked below `force-pushed`, the submit failure was unreachable
    // after any force-push — and Retry with it, while the review sat
    // written and unsent.
    const user = await forcePushWithNotes();
    await screen.findByTestId("unanchored-notes");

    await user.click(screen.getByTestId("repin-note"));
    await user.click(gutter("RIGHT:14"));
    await waitFor(() => expect(screen.queryByTestId("unanchored-notes")).toBeNull());

    mockSubmitWithComments.mockRejectedValue("host unreachable");
    await user.click(screen.getByTestId("open-submit-sheet"));
    await user.type(screen.getByTestId("submit-sheet-body"), "Summary.");
    await user.click(screen.getByTestId("submit-review-confirm"));

    const notice = await screen.findByTestId("drift-notice");
    expect(notice).toHaveAttribute("data-drift-kind", "submit-failed");
    expect(within(notice).getByText("Retry")).toBeInTheDocument();
  });

  it("shows the diff the notes were written against, read-only", async () => {
    const user = await forcePushWithNotes();
    const notice = await screen.findByTestId("drift-notice");
    await user.click(within(notice).getByText("Show on old diff"));

    const banner = await screen.findByTestId("old-diff-banner");
    expect(banner).toHaveTextContent("as it was when you wrote these notes");
    // Selecting on a superseded diff would anchor to lines that are gone.
    expect(document.querySelector("[data-diff-line]")).toBeNull();
  });
});
