import { useCallback, useRef } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";

import { Skeleton } from "@/components/ui/skeleton";
import {
  getGithubPrByPath,
  getPrInlineComments,
  getPrReviewComments,
  getPullRequestChecks,
} from "@/tauri/commands";
import { resolveProvider } from "@/lib/source-control";
import { fetchProviderAuth } from "@/lib/provider-auth";
import type { PrRow } from "@/lib/pr-overview";
import type { CheckInfo } from "@/tauri/types";
import { ReviewDetail } from "@/components/workspace/review/review-detail";
import { RepoUnreachableState } from "@/components/workspace/review/review-empty-states";
import { checkState } from "@/components/workspace/review/review-ui";
import { budgetApplies, clearRateLimitPause, useRateLimitPause } from "@/lib/pr-rate-limit";

/**
 * How often the open pull request re-reads itself while its checks are
 * still running.
 *
 * Fast on purpose: a build going green under the cursor is most of what
 * the column is for, and two and a half seconds is what makes that feel
 * like watching rather than refreshing.
 */
const DETAIL_POLL_MS = 2_500;

/**
 * And how often once every check has reported.
 *
 * A settled pull request has nothing left to watch at the fast cadence,
 * and the cost of pretending otherwise is not theoretical: two calls
 * every 2.5s is roughly three thousand requests an hour, spent re-asking
 * a question whose answer has stopped changing, against an hourly budget
 * of five thousand for the whole account. A pull request left open on a
 * second monitor was quietly the most expensive thing in the app.
 *
 * Twenty seconds is still well inside "I looked away and it updated",
 * and it is already the cadence the conversation below it polls at — so
 * a settled column now refreshes as one surface rather than in two
 * tiers.
 */
const SETTLED_POLL_MS = 20_000;
const CONVERSATION_POLL_MS = 30_000;

export interface PrDetailColumnProps {
  row: PrRow;
  /** Workspace standing on this PR's branch, when there is one. */
  existingWorkspaceId: string | null;
  viewerLogin: string | null;
}

/**
 * Nothing is still running, so nothing is still changing — and the fast
 * cadence has nothing left to buy.
 *
 * `checks` undefined is "not asked yet", which is not settled: the first
 * answer should arrive on the fast clock.
 *
 * A failing checks call *is* settled. Re-asking a host that just said no
 * every two and a half seconds is the behaviour the budget gate exists
 * to prevent, and doing it here would be doing it in the one place that
 * asks most often.
 *
 * The empty list is the case worth naming, because it is two different
 * situations wearing the same shape: a repository with no CI at all, and
 * a pull request whose first check has not registered yet — the seconds
 * right after a push, which is exactly when someone opens the column to
 * watch. Only the row can tell them apart. `"none"` is the host having
 * looked and found nothing; `null` is nobody having asked. So an empty
 * list settles on the host's own word and on nothing else, and a
 * just-pushed pull request keeps the fast cadence until its checks show
 * up rather than going quiet for the twenty seconds that matter most.
 */
export function checksAreSettled(
  checks: CheckInfo[] | undefined,
  checksFailed: boolean,
  rowChecks: string | null,
): boolean {
  if (checksFailed) return true;
  if (!checks) return false;
  if (checks.length === 0) return rowChecks === "none";
  // `checkState` returns "running" for anything it does not recognise —
  // queued, in_progress, waiting, action_required — so an unfamiliar
  // status keeps the fast cadence rather than silencing the column.
  return !checks.some((check) => checkState(check.conclusion, check.status) === "running");
}

/**
 * The detail column: the same component the workspace panel renders,
 * given a project path and a number instead of a workspace.
 *
 * Everything below the header — checks, reviewers, description, threads,
 * the action bar, the drift slot, the agent handoffs — comes across
 * unchanged. That is the point of it having been built without
 * workspace-shaped props.
 */
export function PrDetailColumn({
  row,
  existingWorkspaceId,
  viewerLogin,
}: PrDetailColumnProps) {
  const queryClient = useQueryClient();
  const path = row.projectRoot;
  const number = row.number;

  // The cadence both live queries read, through a ref so that changing
  // it never tears down and re-creates their timers: React Query calls
  // the callback form each time a timer fires, which is exactly when a
  // new value should take effect.
  const pollMsRef = useRef(DETAIL_POLL_MS);

  // The same budget gate the list obeys. An open pull request polling
  // twice every couple of seconds is the most expensive surface in the
  // app, so leaving it outside the gate would mean the page stopped
  // asking and the column carried on spending on its behalf — and every
  // one of those calls is refused anyway while the budget is gone.
  const paused = useRateLimitPause() > 0 && budgetApplies(row.providerKind);

  const checksQuery = useQuery({
    queryKey: ["pr", "page-checks", path, number] as const,
    queryFn: () => getPullRequestChecks(path, number),
    enabled: !paused,
    staleTime: DETAIL_POLL_MS,
    refetchInterval: () => pollMsRef.current,
  });

  pollMsRef.current = checksAreSettled(checksQuery.data, checksQuery.isError, row.checks)
    ? SETTLED_POLL_MS
    : DETAIL_POLL_MS;

  const detailQuery = useQuery({
    queryKey: ["pr", "page-detail", path, number] as const,
    queryFn: () => getGithubPrByPath(path, number),
    enabled: !paused,
    staleTime: DETAIL_POLL_MS,
    refetchInterval: () => pollMsRef.current,
  });

  const reviewsQuery = useQuery({
    queryKey: ["pr", "page-reviews", path, number] as const,
    queryFn: () => getPrReviewComments(path, number),
    enabled: !paused,
    staleTime: CONVERSATION_POLL_MS,
    refetchInterval: CONVERSATION_POLL_MS,
  });

  /**
   * Per-operation declarations for this repository.
   *
   * The same `check_provider_auth` the panel asks, through the same
   * one-minute cache — so this is not a second command, it is the second
   * *caller* of one. Long stale time: what a host can do does not change
   * between two polls of a pull request.
   */
  const opsQuery = useQuery({
    queryKey: ["pr", "page-ops", path] as const,
    queryFn: () => fetchProviderAuth(path, row.providerKind),
    staleTime: 60_000,
  });

  const inlineQuery = useQuery({
    queryKey: ["pr", "page-inline", path, number] as const,
    queryFn: () => getPrInlineComments(path, number),
    enabled: !paused,
    staleTime: CONVERSATION_POLL_MS,
    refetchInterval: CONVERSATION_POLL_MS,
  });

  const refresh = useCallback(() => {
    // Same meaning as the list's Retry: asking by hand lifts the gate,
    // because otherwise this invalidates a set of queries that are
    // disabled — marking them stale and refreshing nothing, which is a
    // control that quietly does not do what it says.
    clearRateLimitPause();
    queryClient.invalidateQueries({
      predicate: (q) =>
        q.queryKey[0] === "pr" &&
        typeof q.queryKey[1] === "string" &&
        q.queryKey[1].startsWith("page-") &&
        q.queryKey[2] === path &&
        q.queryKey[3] === number,
    });
    // The row's own summary (checks colour, review decision) came from
    // the overview, so the list has to hear about it too.
    queryClient.invalidateQueries({ predicate: (q) => q.queryKey[0] === "prs" });
  }, [queryClient, path, number]);

  const pr = detailQuery.data ?? null;
  const operations = opsQuery.data?.operations ?? null;

  // Binding rule 2: only a PR that has *never* loaded gets a skeleton.
  // The declarations are waited for alongside the PR rather than
  // defaulted to: a control drawn on a guess and then withdrawn is worse
  // than one that arrives a moment later with the data it gates on.
  if (!pr || !operations) {
    if (detailQuery.isError || opsQuery.isError) {
      return (
        <RepoUnreachableState
          repoSlug={row.repo}
          provider={resolveProvider(row.providerKind)}
          onRetry={refresh}
        />
      );
    }
    return (
      <div className="flex flex-col gap-3 p-4" data-testid="pr-detail-skeleton">
        <Skeleton className="h-4 w-40" />
        <Skeleton className="h-5 w-3/4" />
        <Skeleton className="h-3 w-1/3" />
        <Skeleton className="h-16 w-full" />
      </div>
    );
  }

  const staleAgeMs =
    detailQuery.isError && detailQuery.dataUpdatedAt > 0
      ? Date.now() - detailQuery.dataUpdatedAt
      : null;

  return (
    <ReviewDetail
      pr={pr}
      checks={checksQuery.data ?? []}
      reviews={reviewsQuery.data ?? []}
      inlineComments={inlineQuery.data ?? []}
      checksLoading={checksQuery.isFetching}
      commentsLoading={reviewsQuery.isFetching || inlineQuery.isFetching}
      cwd={path}
      workspaceId={null}
      projectRoot={path}
      provider={resolveProvider(row.providerKind)}
      operations={operations}
      repoSlug={row.repo}
      viewerLogin={viewerLogin}
      checkedOutHere={false}
      existingWorkspaceId={existingWorkspaceId}
      showCheckout
      // The page is not standing in a worktree, so there is no local
      // drift to report: behind/dirty are properties of a checkout.
      gitBehind={0}
      gitDirtyFiles={0}
      staleAgeMs={staleAgeMs}
      onRefresh={refresh}
    />
  );
}
