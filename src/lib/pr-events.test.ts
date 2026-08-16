import { beforeEach, describe, expect, it, vi } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";

import { detectPrEvents, snapshotRows, type PrSnapshot } from "./pr-events";
import type { PrRow } from "./pr-overview";

const overviewRows = vi.hoisted(() => ({ current: [] as PrRow[] }));
const viewers = vi.hoisted(() => ({ current: new Map<string, string | null>() }));
const getPullRequestChecks = vi.hoisted(() => vi.fn());
const handOffToAgent = vi.hoisted(() => vi.fn(() => Promise.resolve({} as never)));
const toastInfo = vi.hoisted(() => vi.fn());
const toastError = vi.hoisted(() => vi.fn());

vi.mock("@/lib/pr-overview-query", () => ({
  usePrOverview: () => ({ rows: overviewRows.current, viewerByRoot: viewers.current }),
}));
vi.mock("@/tauri/commands", () => ({ getPullRequestChecks }));
vi.mock("@/lib/pr-agent-handoff", () => ({ handOffToAgent }));
vi.mock("@/lib/toast", () => ({
  toast: { info: toastInfo, error: toastError, success: vi.fn(), warning: vi.fn() },
}));

import { usePrEventToasts } from "./pr-event-toasts";
import { useUIStore } from "@/stores/ui-store";

const ROOT = "/home/dev/projects/codemux";

const row = (over: Partial<PrRow> = {}): PrRow => ({
  number: 285,
  title: "Fix the installer",
  author: "mock-dev",
  head_branch: "fix/installer",
  is_draft: false,
  additions: 4,
  deletions: 1,
  review_decision: null,
  checks: "passing",
  review_requested_from: [],
  updated_at: null,
  url: "https://github.com/example/codemux/pull/285",
  projectRoot: ROOT,
  repo: "example/codemux",
  providerKind: "github",
  ...over,
});

const VIEWERS = new Map<string, string | null>([[ROOT, "mock-dev"]]);

function snap(rows: PrRow[]): PrSnapshot {
  return snapshotRows(rows, VIEWERS);
}

describe("detectPrEvents", () => {
  it("says nothing on the first poll of a session", () => {
    const result = detectPrEvents(
      null,
      snap([row({ review_requested_from: ["mock-dev"] }), row({ number: 9, checks: "failing" })]),
      new Set(),
    );
    expect(result.events).toEqual([]);
  });

  it("fires once when a pull request starts needing your review", () => {
    const before = snap([row()]);
    const after = snap([row({ review_requested_from: ["mock-dev"] })]);

    const first = detectPrEvents(before, after, new Set());
    expect(first.events).toEqual([
      { kind: "review-requested", row: expect.objectContaining({ number: 285 }) },
    ]);

    // The next poll reports the same thing. It is not news twice.
    const second = detectPrEvents(after, after, first.fired);
    expect(second.events).toEqual([]);
  });

  it("says nothing about a repository it is seeing for the first time", () => {
    const before = snapshotRows([], new Map());
    const after = snap([row({ review_requested_from: ["mock-dev"] })]);
    // A project opened mid-session arrives with a backlog, and none of
    // that backlog just happened.
    expect(detectPrEvents(before, after, new Set()).events).toEqual([]);
  });

  it("fires when your own pull request goes red, and not again while it stays red", () => {
    const before = snap([row({ checks: "pending" })]);
    const after = snap([row({ checks: "failing" })]);

    const first = detectPrEvents(before, after, new Set());
    expect(first.events).toEqual([
      { kind: "checks-failed", row: expect.objectContaining({ number: 285 }) },
    ]);
    expect(detectPrEvents(after, after, first.fired).events).toEqual([]);
  });

  it("ignores a red pull request that isn't yours", () => {
    const before = snap([row({ author: "juliusm", checks: "passing" })]);
    const after = snap([row({ author: "juliusm", checks: "failing" })]);
    expect(detectPrEvents(before, after, new Set()).events).toEqual([]);
  });

  it("lets a pull request that went green announce its next failure", () => {
    const red = snap([row({ checks: "failing" })]);
    const green = snap([row({ checks: "passing" })]);

    const first = detectPrEvents(snap([row({ checks: "passing" })]), red, new Set());
    expect(first.events).toHaveLength(1);
    const recovered = detectPrEvents(red, green, first.fired);
    expect(recovered.events).toEqual([]);
    const again = detectPrEvents(green, red, recovered.fired);
    expect(again.events).toHaveLength(1);
  });
});

describe("usePrEventToasts", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    overviewRows.current = [];
    viewers.current = VIEWERS;
    getPullRequestChecks.mockResolvedValue([]);
    useUIStore.setState({ showPullRequests: false, pendingPrSelection: null });
  });

  it("raises a review-request toast with an Open action", async () => {
    overviewRows.current = [row()];
    const { rerender } = renderHook(() => usePrEventToasts());

    overviewRows.current = [row({ review_requested_from: ["mock-dev"] })];
    rerender();

    await waitFor(() => expect(toastInfo).toHaveBeenCalledTimes(1));
    const [message, options] = toastInfo.mock.calls[0];
    expect(message).toBe("Review requested · example/codemux#285");
    expect(options.description).toBe("Fix the installer");

    options.action.onClick();
    expect(useUIStore.getState().pendingPrSelection).toEqual({
      projectRoot: ROOT,
      number: 285,
    });
  });

  it("names the failing check and hands it to an agent from [Fix]", async () => {
    getPullRequestChecks.mockResolvedValue([
      { name: "lint", status: "COMPLETED", conclusion: "pass" },
      { name: "rust (ubuntu-latest)", status: "COMPLETED", conclusion: "fail" },
    ]);
    overviewRows.current = [row({ checks: "passing" })];
    const { rerender } = renderHook(() => usePrEventToasts());

    overviewRows.current = [row({ checks: "failing" })];
    rerender();

    await waitFor(() => expect(toastError).toHaveBeenCalledTimes(1));
    const [message, options] = toastError.mock.calls[0];
    expect(message).toBe("CI failed on example/codemux#285");
    expect(options.description).toBe("rust (ubuntu-latest)");

    options.action.onClick();
    expect(handOffToAgent).toHaveBeenCalledWith(
      expect.objectContaining({
        task: { kind: "failing-check", checkName: "rust (ubuntu-latest)" },
        projectRoot: ROOT,
        prRef: "example/codemux#285",
      }),
    );
  });

  it("offers Open instead of Fix when no check can be named", async () => {
    getPullRequestChecks.mockRejectedValue("host unreachable");
    overviewRows.current = [row({ checks: "passing" })];
    const { rerender } = renderHook(() => usePrEventToasts());

    overviewRows.current = [row({ checks: "failing" })];
    rerender();

    await waitFor(() => expect(toastError).toHaveBeenCalledTimes(1));
    expect(toastError.mock.calls[0][1].action.label).toBe("Open");
  });

  it("stays quiet on the session's first poll", async () => {
    overviewRows.current = [
      row({ review_requested_from: ["mock-dev"] }),
      row({ number: 9, checks: "failing" }),
    ];
    renderHook(() => usePrEventToasts());

    await waitFor(() => expect(getPullRequestChecks).not.toHaveBeenCalled());
    expect(toastInfo).not.toHaveBeenCalled();
    expect(toastError).not.toHaveBeenCalled();
  });
});
