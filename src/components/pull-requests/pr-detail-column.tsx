import { useCallback } from "react";
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
import { ReviewDetail } from "@/components/workspace/review/review-detail";
import { RepoUnreachableState } from "@/components/workspace/review/review-empty-states";

/** Same cadence as the panel: a check going green is the one thing you
 *  wait for, and 2.5s is fast enough that you never reach for refresh. */
const DETAIL_POLL_MS = 2_500;
const CONVERSATION_POLL_MS = 30_000;

export interface PrDetailColumnProps {
  row: PrRow;
  /** Workspace standing on this PR's branch, when there is one. */
  existingWorkspaceId: string | null;
  viewerLogin: string | null;
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

  const detailQuery = useQuery({
    queryKey: ["pr", "page-detail", path, number] as const,
    queryFn: () => getGithubPrByPath(path, number),
    staleTime: DETAIL_POLL_MS,
    refetchInterval: DETAIL_POLL_MS,
  });

  const checksQuery = useQuery({
    queryKey: ["pr", "page-checks", path, number] as const,
    queryFn: () => getPullRequestChecks(path, number),
    staleTime: DETAIL_POLL_MS,
    refetchInterval: DETAIL_POLL_MS,
  });

  const reviewsQuery = useQuery({
    queryKey: ["pr", "page-reviews", path, number] as const,
    queryFn: () => getPrReviewComments(path, number),
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
    staleTime: CONVERSATION_POLL_MS,
    refetchInterval: CONVERSATION_POLL_MS,
  });

  const refresh = useCallback(() => {
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
