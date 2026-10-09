/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { PrDetailColumn } from "./pr-detail-column";
import { ReviewPanel, _resetCaches } from "../workspace/review-panel";
import { ALL_OPERATIONS } from "@/lib/provider-auth";
import { _resetRateLimitGate } from "@/lib/pr-rate-limit";
import {
  _resetPrDrafts, addLineDraft, draftKey, getDiffSnapshot, getLineDrafts,
} from "../workspace/review/pr-drafts";
import type { PrRow } from "@/lib/pr-overview";
import type { PullRequestInfo, WorkspaceSnapshot } from "@/tauri/types";

// Keep the actual column, panel, nested ReviewDetail and query observers.
// Only the native/network boundary is replaced; no source-string checks.
const rpc = vi.hoisted(() => ({
  refresh: vi.fn(), detail: vi.fn(), branchDetail: vi.fn(), checks: vi.fn(),
  reviews: vi.fn(), inline: vi.fn(), threads: vi.fn(), diff: vi.fn(),
  timeline: vi.fn(), auth: vi.fn(), repo: vi.fn(), defaultBranch: vi.fn(),
  submit: vi.fn(), quota: vi.fn(),
}));
vi.mock("@/tauri/commands", () => ({
  refreshGithubReadCache: rpc.refresh,
  getGithubPrByPath: rpc.detail,
  getBranchPullRequest: rpc.branchDetail,
  getPullRequestChecks: rpc.checks,
  getPrReviewComments: rpc.reviews,
  getPrInlineComments: rpc.inline,
  getPrReviewThreads: rpc.threads,
  getPrReviewDiff: rpc.diff,
  getPrTimeline: rpc.timeline,
  checkProviderAuth: rpc.auth,
  checkGithubRepo: rpc.repo,
  getDefaultBranch: rpc.defaultBranch,
  submitPrReview: rpc.submit,
  githubRateLimit: rpc.quota,
}));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));
vi.mock("@/lib/toast", () => ({
  toast: { error: vi.fn(), success: vi.fn(), info: vi.fn(), warning: vi.fn() },
}));

const ROOT = "/fixture/repository";
function pr(number = 42): PullRequestInfo {
  return {
    number, title: `Pull request ${number}`, url: `https://github.com/fixture/repo/pull/${number}`,
    state: "OPEN", head_branch: `branch/${number}`, base_branch: "main", is_draft: false,
    mergeable: "MERGEABLE", additions: 1, deletions: 0, review_decision: null,
    checks_passing: true, updated_at: "2026-10-01T00:00:00Z", created_at: null,
    body: "Original body", comments: [], totalComments: 0, author: "author",
    head_ref_oid: `head-${number}`, head_repository_owner: "fixture", merge_state_status: "CLEAN",
    changed_files: 1, merged_by: null, merged_at: null, review_requests: [], latest_reviews: [],
  };
}
function row(number = 42, providerKind = "github"): PrRow {
  return {
    ...pr(number), author: "author", additions: 1, deletions: 0,
    checks: "passing", review_requested_from: [], projectRoot: ROOT, repo: "fixture/repo",
    providerKind,
  };
}
function workspace(number = 42, provider_kind = "github"): WorkspaceSnapshot {
  return {
    workspace_id: "fixture-workspace", title: "Fixture", workspace_type: "standard", cwd: ROOT,
    git_branch: `branch/${number}`, git_ahead: 0, git_behind: 0, git_additions: 0,
    git_deletions: 0, git_changed_files: 0, notification_count: 0, notifications_muted: false,
    latest_agent_state: null, worktree_path: null, project_root: ROOT,
    pr_number: number, pr_state: "OPEN", pr_url: pr(number).url, linked_issue: null,
    tabs: [], active_tab_id: "tab", active_surface_id: "surface", surfaces: [], provider_kind,
  };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}
const clients: QueryClient[] = [];
function mount(surface: "page" | "panel", number = 42, providerKind = "github") {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, retryDelay: 0 } } });
  clients.push(client);
  const node = (selected: number) => (
    <QueryClientProvider client={client}>
      {surface === "page"
        ? <PrDetailColumn row={row(selected, providerKind)} existingWorkspaceId={null} viewerLogin="reviewer" />
        : <ReviewPanel workspace={workspace(selected, providerKind)} />}
    </QueryClientProvider>
  );
  const view = render(node(number));
  return { client, select: (selected: number) => view.rerender(node(selected)) };
}
async function clickRefresh(user = userEvent.setup()) {
  await user.click(screen.getByRole("button", { name: "Pull request actions" }));
  await user.click(await screen.findByRole("menuitem", { name: "Refresh" }));
}

beforeEach(() => {
  vi.resetAllMocks();
  _resetCaches();
  _resetRateLimitGate();
  _resetPrDrafts();
  rpc.refresh.mockResolvedValue(undefined);
  rpc.detail.mockImplementation((_path: string, number: number) => Promise.resolve(pr(number)));
  rpc.branchDetail.mockResolvedValue(pr());
  rpc.checks.mockResolvedValue([]);
  rpc.reviews.mockResolvedValue([]);
  rpc.inline.mockResolvedValue([]);
  rpc.threads.mockResolvedValue([]);
  rpc.diff.mockResolvedValue("diff --git a/file.ts b/file.ts\n--- a/file.ts\n+++ b/file.ts\n@@ -1 +1 @@\n-old\n+new\n");
  rpc.timeline.mockResolvedValue([]);
  rpc.auth.mockResolvedValue({ kind: "github", supported: true, installed: true,
    authenticated: true, username: "reviewer", operations: ALL_OPERATIONS });
  rpc.repo.mockResolvedValue(true);
  rpc.defaultBranch.mockResolvedValue("main");
  rpc.submit.mockResolvedValue(undefined);
});
afterEach(() => {
  cleanup();
  for (const client of clients.splice(0)) client.clear();
});

describe.each(["page", "panel"] as const)("%s manual refresh", (surface) => {
  it.each(["deferred success", "detail failure"] as const)(
    "never caches a pushed patch under the old head while detail has %s",
    async (outcome) => {
      const oldPatch = "diff --git a/file.ts b/file.ts\n--- a/file.ts\n+++ b/file.ts\n@@ -1 +1 @@\n-old\n+new\n";
      const newPatch = oldPatch.replace("+new", "+rewritten");
      let remoteHead = pr().head_ref_oid;
      // Model the native optional contract: legacy unbound reads return the
      // current patch; bound reads fail closed if the head has moved.
      rpc.diff.mockImplementation((_path: string, _number: number, expected?: string) => {
        if (expected != null && expected !== remoteHead) {
          return Promise.reject("Pull request head changed while loading the review diff");
        }
        return Promise.resolve(remoteHead === pr().head_ref_oid ? oldPatch : newPatch);
      });
      const pendingRefresh = deferred<void>();
      const pendingDetail = deferred<PullRequestInfo>();
      const detail = surface === "page" ? rpc.detail : rpc.branchDetail;
      const view = mount(surface);
      await screen.findByText("Pull request 42");
      await userEvent.setup().click(screen.getByTestId("review-tab-code"));
      const oldKey = ["pr", "review-diff", ROOT, 42, pr().head_ref_oid];
      const notesKey = draftKey(surface === "page" ? `page:${ROOT}` : "fixture-workspace", 42);
      await waitFor(() => expect(view.client.getQueryData(oldKey)).toBe(oldPatch));
      await waitFor(() => expect(getDiffSnapshot(notesKey, pr().head_ref_oid!)).toBe(oldPatch));
      act(() => {
        addLineDraft(notesKey, {
          path: "file.ts", side: "RIGHT", line: 1, startLine: null,
          lineText: "new", startLineText: null, contextBefore: null, contextAfter: null,
          hunkHeader: "@@ -1 +1 @@", headOidAtDraft: pr().head_ref_oid!, body: "Keep this note",
        });
      });
      rpc.refresh.mockReturnValue(pendingRefresh.promise);
      detail.mockReturnValue(pendingDetail.promise);
      remoteHead = "head-pushed";
      await clickRefresh();
      expect(detail).toHaveBeenCalledTimes(1);
      expect(rpc.diff).toHaveBeenCalledTimes(1);
      await act(async () => { pendingRefresh.resolve(); });
      await waitFor(() => expect(detail).toHaveBeenCalledTimes(2));
      await waitFor(() => expect(view.client.getQueryState(oldKey)?.fetchStatus).toBe("idle"));
      expect(view.client.getQueryData(oldKey)).toBe(oldPatch);
      expect(view.client.getQueryState(oldKey)?.status).toBe("error");
      expect(getDiffSnapshot(notesKey, pr().head_ref_oid!)).toBe(oldPatch);
      expect(getLineDrafts(notesKey)).toEqual([expect.objectContaining({
        body: "Keep this note", headOidAtDraft: pr().head_ref_oid, status: "pinned",
      })]);

      if (outcome === "deferred success") {
        await act(async () => { pendingDetail.resolve({ ...pr(), head_ref_oid: remoteHead }); });
        const newKey = ["pr", "review-diff", ROOT, 42, remoteHead];
        await waitFor(() => expect(view.client.getQueryData(newKey)).toBe(newPatch));
        await waitFor(() => expect(getDiffSnapshot(notesKey, remoteHead!)).toBe(newPatch));
        expect(rpc.diff).toHaveBeenLastCalledWith(ROOT, 42, remoteHead);
      } else {
        await act(async () => { pendingDetail.reject("detail unavailable"); });
        const detailKey = surface === "page"
          ? ["pr", "page-detail", ROOT, 42]
          : ["pr", "detail", "fixture-workspace", 42];
        await waitFor(() => expect(view.client.getQueryState(detailKey)?.status).toBe("error"));
        expect(screen.getByText("Pull request 42")).toBeInTheDocument();
        expect(view.client.getQueryData(["pr", "review-diff", ROOT, 42, remoteHead])).toBeUndefined();
        expect(getLineDrafts(notesKey)[0].body).toBe("Keep this note");
      }
      expect(view.client.getQueryData(oldKey)).toBe(oldPatch);
      expect(getDiffSnapshot(notesKey, pr().head_ref_oid!)).toBe(oldPatch);
    },
  );

  it("awaits native successful-read invalidation before refetching detail and nested threads", async () => {
    const pending = deferred<void>();
    rpc.refresh.mockReturnValue(pending.promise);
    mount(surface);
    await screen.findByText("Pull request 42");
    await waitFor(() => expect(rpc.threads).toHaveBeenCalledTimes(1));
    const detail = surface === "page" ? rpc.detail : rpc.branchDetail;
    await clickRefresh();
    expect(rpc.refresh).toHaveBeenCalledWith(ROOT);
    expect(detail).toHaveBeenCalledTimes(1);
    expect(rpc.checks).toHaveBeenCalledTimes(1);
    expect(rpc.threads).toHaveBeenCalledTimes(1);
    expect(screen.getByText("Pull request 42")).toBeInTheDocument();
    await act(async () => { pending.resolve(); });
    await waitFor(() => expect(detail).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(rpc.threads).toHaveBeenCalledTimes(2));
  });

  it("coalesces duplicate manual refresh clicks until native and React Query settle", async () => {
    const pending = deferred<void>();
    rpc.refresh.mockReturnValue(pending.promise);
    mount(surface);
    await screen.findByText("Pull request 42");
    await clickRefresh();
    await clickRefresh();
    expect(rpc.refresh).toHaveBeenCalledTimes(1);
    await act(async () => { pending.resolve(); });
    const detail = surface === "page" ? rpc.detail : rpc.branchDetail;
    await waitFor(() => expect(detail).toHaveBeenCalledTimes(2));
  });

  it("does not apply an old refresh to a later visit of the same selected PR", async () => {
    const pending = deferred<void>();
    rpc.refresh.mockReturnValue(pending.promise);
    const view = mount(surface);
    await screen.findByText("Pull request 42");
    await clickRefresh();
    rpc.branchDetail.mockResolvedValue(pr(43));
    view.select(43);
    await screen.findByText("Pull request 43");
    rpc.branchDetail.mockResolvedValue(pr(42));
    view.select(42);
    await screen.findByText("Pull request 42");
    const detail = surface === "page" ? rpc.detail : rpc.branchDetail;
    expect(detail).toHaveBeenCalledTimes(2);
    await act(async () => { pending.resolve(); });
    // Drain scheduled observer updates, not a timeout-based race guess.
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
    expect(detail).toHaveBeenCalledTimes(2);
    expect(screen.getByText("Pull request 42")).toBeInTheDocument();
  });
});