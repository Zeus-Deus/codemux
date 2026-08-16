/**
 * One fetch of the pull-request overview, shared by three surfaces.
 *
 * The page, the sidebar badge and the palette's `pr ` mode all want the
 * same rows. They share a query key rather than a fetch each, so opening
 * the page costs nothing the badge hasn't already paid for, and the
 * badge exists before the page has ever been opened.
 */

import { useMemo } from "react";
import { useQueries, useQueryClient } from "@tanstack/react-query";

import { listPrsOverview, listPullRequests } from "@/tauri/commands";
import { useAppStore } from "@/stores/app-store";
import { repoSlugFromUrl } from "@/lib/source-control";
import type { PrRow } from "@/lib/pr-overview";
import { rowKey } from "@/lib/pr-overview";
import type { PullRequestInfo } from "@/tauri/types";

/** 30s: PRs across every open project change on a human cadence, and
 *  each root is a CLI shell-out. The detail column polls far faster. */
export const PR_OVERVIEW_REFETCH_MS = 30_000;

export const prOverviewKey = (projectRoot: string) =>
  ["prs", "overview", projectRoot] as const;

export interface ProjectRoot {
  path: string;
  /** Wire `provider_kind` off any workspace in this root. */
  providerKind: string;
  /** Last path segment — what the row prints when the host can't name
   *  the repository. */
  name: string;
}

/** Every distinct repository root the user has a workspace open in. */
export function useProjectRoots(): ProjectRoot[] {
  const workspaces = useAppStore((s) => s.appState?.workspaces);
  return useMemo(() => {
    const roots = new Map<string, ProjectRoot>();
    for (const ws of workspaces ?? []) {
      const path = ws.project_root ?? ws.cwd;
      if (!path) continue;
      const existing = roots.get(path);
      if (existing) {
        // A root only needs one workspace to name its product; prefer
        // whichever one actually has an answer.
        if (existing.providerKind === "unknown" && ws.provider_kind) {
          existing.providerKind = ws.provider_kind;
        }
        continue;
      }
      roots.set(path, {
        path,
        providerKind: ws.provider_kind ?? "unknown",
        name: path.split(/[/\\]/).filter(Boolean).pop() ?? path,
      });
    }
    return [...roots.values()].sort((a, b) => a.name.localeCompare(b.name));
  }, [workspaces]);
}

export interface RootFailure {
  root: ProjectRoot;
  message: string;
}

/** What the list's state dropdown can ask for. */
export type PrStateFilter = "open" | "closed" | "all";

export const prHistoryKey = (projectRoot: string, state: PrStateFilter) =>
  ["prs", "history", projectRoot, state] as const;

/**
 * A closed or merged pull request, in row shape.
 *
 * `list_pull_requests` is the cheap historical list and carries no CI
 * rollup, so the row draws no dot rather than a green one it can't
 * stand behind.
 */
function historyRow(pr: PullRequestInfo, root: ProjectRoot): PrRow {
  return {
    number: pr.number,
    title: pr.title,
    author: pr.author ?? "",
    head_branch: pr.head_branch,
    is_draft: pr.is_draft,
    additions: pr.additions,
    deletions: pr.deletions,
    review_decision: pr.review_decision,
    checks:
      pr.checks_passing === true
        ? "passing"
        : pr.checks_passing === false
          ? "failing"
          : "none",
    review_requested_from: pr.review_requests ?? [],
    updated_at: pr.updated_at,
    url: pr.url,
    state: pr.state,
    projectRoot: root.path,
    repo: repoSlugFromUrl(pr.url) ?? root.name,
    providerKind: root.providerKind,
  };
}

export interface PrOverviewResult {
  rows: PrRow[];
  viewerByRoot: Map<string, string | null>;
  /** Roots whose fetch failed. The rest of the list still renders —
   *  a repository you can't reach is a footer line, not a blank page. */
  failures: RootFailure[];
  roots: ProjectRoot[];
  /** Newest successful fetch across all roots, for "20s ago". */
  updatedAt: number | null;
  isLoading: boolean;
  refresh: () => void;
}

/**
 * The overview across every open project.
 *
 * One query per root rather than one for all of them: a root that is
 * unreachable then fails alone, and the other repositories keep both
 * their rows and their refresh cadence (binding rule 2).
 */
export function usePrOverview(
  enabled = true,
  stateFilter: PrStateFilter = "open",
): PrOverviewResult {
  const roots = useProjectRoots();
  const queryClient = useQueryClient();

  const results = useQueries({
    queries: roots.map((root) => ({
      queryKey: prOverviewKey(root.path),
      queryFn: () => listPrsOverview(root.path),
      enabled,
      staleTime: PR_OVERVIEW_REFETCH_MS,
      refetchInterval: PR_OVERVIEW_REFETCH_MS,
      // One retry, then the footer says so. Retrying a repository that
      // isn't there just delays the sentence that explains it.
      retry: 1,
    })),
  });

  // Closed and merged rows are a second, cheaper list, fetched only
  // when the dropdown asks for them. The open rows keep coming from the
  // overview so they keep the CI rollup and the review requests that
  // the historical list has no reason to carry.
  const wantsHistory = stateFilter !== "open";
  const historyResults = useQueries({
    queries: roots.map((root) => ({
      queryKey: prHistoryKey(root.path, stateFilter),
      queryFn: () => listPullRequests(root.path, stateFilter === "closed" ? "closed" : "all"),
      enabled: enabled && wantsHistory,
      staleTime: PR_OVERVIEW_REFETCH_MS,
      retry: 1,
    })),
  });

  const rows: PrRow[] = [];
  const viewerByRoot = new Map<string, string | null>();
  const failures: RootFailure[] = [];
  let updatedAt: number | null = null;
  let isLoading = false;

  results.forEach((result, i) => {
    const root = roots[i];
    if (!root) return;
    if (result.isLoading) isLoading = true;
    if (result.data) {
      viewerByRoot.set(root.path, result.data.viewer);
      for (const item of result.data.items) {
        rows.push({
          ...item,
          projectRoot: root.path,
          repo: repoSlugFromUrl(item.url) ?? root.name,
          providerKind: root.providerKind,
        });
      }
    }
    if (result.dataUpdatedAt > 0) {
      updatedAt = Math.max(updatedAt ?? 0, result.dataUpdatedAt);
    }
    // Stale and labelled beats blank: a root that has data *and* a
    // failing refresh keeps its rows and still reports the failure.
    if (result.isError) {
      failures.push({ root, message: String(result.error) });
    }
  });

  if (wantsHistory) {
    const seen = new Set(rows.map(rowKey));
    historyResults.forEach((result, i) => {
      const root = roots[i];
      if (!root || !result.data) return;
      for (const pr of result.data as PullRequestInfo[]) {
        const row = historyRow(pr, root);
        if (seen.has(rowKey(row))) continue;
        seen.add(rowKey(row));
        rows.push(row);
      }
    });
  }

  // "Closed" means closed: the open rows the overview always fetches
  // are filtered out here rather than left in a list whose label says
  // they aren't there.
  const visible =
    stateFilter === "closed"
      ? rows.filter((row) => row.state && row.state.toUpperCase() !== "OPEN")
      : rows;

  const refresh = () => {
    queryClient.invalidateQueries({ predicate: (q) => q.queryKey[0] === "prs" });
  };

  return {
    rows: visible,
    viewerByRoot,
    failures,
    roots,
    updatedAt,
    isLoading,
    refresh,
  };
}
