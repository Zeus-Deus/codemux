/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, cleanup, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const mockToastError = vi.fn();
vi.mock("@/lib/toast", () => ({
  toast: {
    info: vi.fn(),
    success: vi.fn(),
    warning: vi.fn(),
    error: (...a: unknown[]) => mockToastError(...a),
  },
}));

import { ReviewThreads } from "./review-threads";
import type {
  InlineReviewComment,
  PrReviewThread,
  ReviewComment,
} from "@/tauri/types";

function thread(over: Partial<PrReviewThread> = {}): PrReviewThread {
  return {
    id: "T1",
    is_resolved: false,
    is_outdated: false,
    is_resolvable: true,
    path: "src/a.ts",
    line: 12,
    comments: [
      {
        id: "C1",
        database_id: 8001,
        author: "juliusm",
        body: "This needs a close on the error path.",
        created_at: new Date(Date.now() - 60_000).toISOString(),
      },
    ],
    ...over,
  };
}

function inline(over: Partial<InlineReviewComment> = {}): InlineReviewComment {
  return {
    id: 8001,
    author: "juliusm",
    body: "This needs a close on the error path.",
    path: "src/a.ts",
    line: 12,
    created_at: new Date(Date.now() - 60_000).toISOString(),
    in_reply_to_id: null,
    pull_request_review_id: 9001,
    ...over,
  };
}

function review(over: Partial<ReviewComment> = {}): ReviewComment {
  return {
    id: 9001,
    author: "juliusm",
    body: "Left two notes; nothing blocking.",
    state: "COMMENTED",
    created_at: new Date(Date.now() - 120_000).toISOString(),
    ...over,
  };
}

/** A promise whose settling this test controls. */
function deferred<T = unknown>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function renderThreads(over: Partial<Parameters<typeof ReviewThreads>[0]> = {}) {
  return render(
    <ReviewThreads
      reviews={[]}
      inlineComments={[]}
      threads={[]}
      onReply={vi.fn().mockResolvedValue(undefined)}
      onSetResolved={vi.fn().mockResolvedValue(undefined)}
      {...over}
    />,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  cleanup();
});

describe("thread rendering", () => {
  it("puts unresolved threads first and expanded, resolved ones folded behind a count", () => {
    renderThreads({
      threads: [
        thread({ id: "T-resolved", is_resolved: true, comments: [
          { id: "C1", database_id: 1, author: "juliusm", body: "Settled question.", created_at: "" },
          { id: "C2", database_id: 2, author: "mock-dev", body: "Fixed.", created_at: "" },
        ] }),
        thread({ id: "T-open", comments: [
          { id: "C3", database_id: 3, author: "juliusm", body: "Still open here.", created_at: "" },
        ] }),
      ],
    });

    const rendered = screen.getAllByTestId("review-thread");
    // Order, not just presence: the open question is why anyone opens
    // this section.
    expect(rendered[0]).toHaveAttribute("data-thread-id", "T-open");
    expect(rendered[1]).toHaveAttribute("data-thread-id", "T-resolved");

    // Open: its words are on screen without a click.
    expect(screen.getByText("Still open here.")).toBeInTheDocument();

    // Resolved: a header with the count, and the conversation folded.
    expect(screen.getByText(/Resolved · 2 comments/)).toBeInTheDocument();
    expect(screen.queryByText("Settled question.")).not.toBeInTheDocument();
  });

  it("expands a resolved thread when its header is clicked", async () => {
    const user = userEvent.setup();
    renderThreads({
      threads: [thread({ id: "T-resolved", is_resolved: true })],
    });

    expect(screen.queryByText(/needs a close/)).not.toBeInTheDocument();
    await user.click(screen.getByTestId("thread-resolved-header-T-resolved"));
    expect(screen.getByText(/needs a close/)).toBeInTheDocument();
  });

  it("labels an outdated thread rather than hiding it", () => {
    renderThreads({ threads: [thread({ is_outdated: true, line: null })] });
    expect(screen.getByTestId("thread-outdated")).toHaveTextContent("Outdated");
    // The objection is still readable — only its lines went away.
    expect(screen.getByText(/needs a close/)).toBeInTheDocument();
  });

  it("counts the unresolved threads in the section header", () => {
    renderThreads({
      threads: [
        thread({ id: "A" }),
        thread({ id: "B" }),
        thread({ id: "C", is_resolved: true }),
      ],
    });
    expect(screen.getByTestId("unresolved-count")).toHaveTextContent("2 unresolved");
  });
});

describe("reply", () => {
  it("sends on ⌘↵ and clears the text only when the host accepts it", async () => {
    const user = userEvent.setup();
    const gate = deferred();
    const onReply = vi.fn().mockReturnValue(gate.promise);
    renderThreads({ threads: [thread({ id: "T1" })], onReply });

    const box = screen.getByTestId("thread-reply-input-T1");
    await user.click(box);
    await user.keyboard("looks right to me");
    await user.keyboard("{Meta>}{Enter}{/Meta}");

    expect(onReply).toHaveBeenCalledTimes(1);
    expect(onReply.mock.calls[0][1]).toBe("looks right to me");

    // In flight: send is disabled, and every word is still there.
    expect(screen.getByTestId("thread-reply-send-T1")).toBeDisabled();
    expect(box).toHaveValue("looks right to me");

    gate.resolve(undefined);
    await waitFor(() => expect(box).toHaveValue(""));
  });

  it("keeps the text and offers Retry when the reply is refused", async () => {
    const user = userEvent.setup();
    const onReply = vi
      .fn()
      .mockRejectedValueOnce("host unreachable")
      .mockResolvedValueOnce(undefined);
    renderThreads({ threads: [thread({ id: "T1" })], onReply });

    const box = screen.getByTestId("thread-reply-input-T1");
    await user.click(box);
    await user.keyboard("a paragraph nobody wants to retype");
    await user.click(screen.getByTestId("thread-reply-send-T1"));

    await waitFor(() =>
      expect(screen.getByTestId("thread-reply-error-T1")).toBeInTheDocument(),
    );
    expect(screen.getByTestId("thread-reply-error-T1")).toHaveTextContent(
      /host unreachable/,
    );
    expect(box).toHaveValue("a paragraph nobody wants to retype");

    // Retry sends exactly what failed.
    await user.click(screen.getByTestId("thread-reply-retry-T1"));
    expect(onReply.mock.calls[1][1]).toBe("a paragraph nobody wants to retype");
    await waitFor(() => expect(box).toHaveValue(""));
  });

  it("keeps the typed text when Escape is pressed", async () => {
    const user = userEvent.setup();
    renderThreads({ threads: [thread({ id: "T1" })] });

    const box = screen.getByTestId("thread-reply-input-T1");
    await user.click(box);
    await user.keyboard("half a thought");
    await user.keyboard("{Escape}");

    // Escape gives the keyboard back; it does not throw work away.
    expect(box).toHaveValue("half a thought");
    expect(box).not.toHaveFocus();
  });
});

describe("resolve", () => {
  it("flips immediately and folds the thread", async () => {
    const user = userEvent.setup();
    const onSetResolved = vi.fn().mockResolvedValue(undefined);
    renderThreads({ threads: [thread({ id: "T1" })], onSetResolved });

    expect(screen.getByTestId("thread-resolve-T1")).toHaveTextContent("Resolve");
    await user.click(screen.getByTestId("thread-resolve-T1"));

    expect(onSetResolved).toHaveBeenCalledWith(expect.objectContaining({ id: "T1" }), true);
    await waitFor(() =>
      expect(screen.getByTestId("review-thread")).toHaveAttribute("data-resolved", "true"),
    );
    expect(screen.getByText(/Resolved · 1 comment/)).toBeInTheDocument();
  });

  it("rolls the flip back and says so when the host refuses", async () => {
    const user = userEvent.setup();
    const onSetResolved = vi.fn().mockRejectedValue("403 Forbidden");
    renderThreads({ threads: [thread({ id: "T1" })], onSetResolved });

    await user.click(screen.getByTestId("thread-resolve-T1"));

    await waitFor(() =>
      expect(screen.getByTestId("review-thread")).toHaveAttribute("data-resolved", "false"),
    );
    expect(screen.getByTestId("thread-resolve-T1")).toHaveTextContent("Resolve");
    expect(mockToastError).toHaveBeenCalledWith("403 Forbidden");
  });

  it("offers Unresolve on a resolved thread", async () => {
    const user = userEvent.setup();
    const onSetResolved = vi.fn().mockResolvedValue(undefined);
    renderThreads({
      threads: [thread({ id: "T1", is_resolved: true })],
      onSetResolved,
    });

    await user.click(screen.getByTestId("thread-resolved-header-T1"));
    expect(screen.getByTestId("thread-resolve-T1")).toHaveTextContent("Unresolve");
    await user.click(screen.getByTestId("thread-resolve-T1"));
    expect(onSetResolved).toHaveBeenCalledWith(expect.objectContaining({ id: "T1" }), false);
  });

  it("draws no Resolve button on a thread the host cannot resolve", () => {
    renderThreads({ threads: [thread({ id: "T1", is_resolvable: false })] });
    expect(screen.queryByTestId("thread-resolve-T1")).not.toBeInTheDocument();
  });
});

describe("dedupe", () => {
  it("renders a comment that lives in a thread exactly once", () => {
    renderThreads({
      threads: [thread()],
      inlineComments: [inline()],
      reviews: [review()],
    });

    expect(screen.getAllByText("This needs a close on the error path.")).toHaveLength(1);
    // The verdict summary is a different resource and stays: it is what
    // the reviewer said *about* the review, not a thread comment.
    expect(screen.getByText("Left two notes; nothing blocking.")).toBeInTheDocument();
  });

  it("still renders an inline comment that no thread carries", () => {
    renderThreads({
      threads: [thread()],
      inlineComments: [inline(), inline({ id: 8999, body: "Orphaned note." })],
    });
    expect(screen.getByText("Orphaned note.")).toBeInTheDocument();
  });

  it("drops a review summary that is the same note as a thread comment", () => {
    // GitLab's shape: a review summary and a thread comment are one
    // note, sharing an id and a body.
    renderThreads({
      threads: [
        thread({
          comments: [
            { id: "5502", database_id: 5502, author: "mira", body: "Reads well.", created_at: "" },
          ],
        }),
      ],
      reviews: [review({ id: 5502, body: "Reads well." })],
    });
    expect(screen.getAllByText("Reads well.")).toHaveLength(1);
  });
});

describe("capability gating", () => {
  it("renders threads read-only when neither write is declared", () => {
    renderThreads({
      threads: [thread({ id: "T1" }), thread({ id: "T2", is_resolved: true })],
      onReply: undefined,
      onSetResolved: undefined,
    });

    // Readable — every thread is still there.
    expect(screen.getAllByTestId("review-thread")).toHaveLength(2);
    expect(screen.getByText(/needs a close/)).toBeInTheDocument();

    // And not a single control that would answer with an apology.
    expect(screen.queryByTestId("thread-reply-input-T1")).not.toBeInTheDocument();
    expect(screen.queryByTestId("thread-resolve-T1")).not.toBeInTheDocument();
  });

  it("draws the composer without the Resolve button when only replies are declared", () => {
    renderThreads({ threads: [thread({ id: "T1" })], onSetResolved: undefined });
    expect(screen.getByTestId("thread-reply-input-T1")).toBeInTheDocument();
    expect(screen.queryByTestId("thread-resolve-T1")).not.toBeInTheDocument();
  });

  it("keeps the agent handoff on unresolved threads, with the thread's anchor", async () => {
    const user = userEvent.setup();
    const onSendToAgent = vi.fn().mockResolvedValue(undefined);
    renderThreads({ threads: [thread({ id: "T1" })], onSendToAgent });

    const open = screen.getByTestId("review-thread");
    await user.click(within(open).getByTestId("send-thread-to-agent"));

    expect(onSendToAgent).toHaveBeenCalledWith(
      expect.objectContaining({
        kind: "review-thread",
        path: "src/a.ts",
        line: 12,
        body: "This needs a close on the error path.",
      }),
    );
  });
});
