/**
 * One fetch of the pull-request overview, shared by three surfaces.
 *
 * The page, the sidebar badge and the palette's `pr ` mode all want the
 * same rows. They share a query key rather than a fetch each, so opening
 * the page costs nothing the badge hasn't already paid for, and the
 * badge exists before the page has ever been opened.
 *
 * Two things make the first paint fast rather than merely correct:
 *
 * - **The list is fetched in two halves.** The cheap half (who, what,
 *   which branch, who it is waiting on) paints; the expensive half (CI
 *   rollup, line counts) arrives behind it and is *merged* into the rows
 *   already on screen. Merging only ever fills blanks — a slow or failed
 *   stats call can never take a value off the page.
 * - **The last list survives the process.** On mount, before any query
 *   has resolved, rows come from `localStorage`. They are marked
 *   `carried`, labelled with their age, and replaced wholesale per root
 *   the moment that root's real answer lands.
 */

import { useEffect, useMemo } from "react";
import { useQueries, useQueryClient } from "@tanstack/react-query";

import { listPrsOverview, listPrsOverviewStats, listPullRequests } from "@/tauri/commands";
import { useAppStore } from "@/stores/app-store";
import { repoSlugFromUrl } from "@/lib/source-control";
import type { PrRow } from "@/lib/pr-overview";
import { rowKey } from "@/lib/pr-overview";
import {
  prSnapshotKey,
  readPrOverviewSnapshot,
  writePrOverviewSnapshot,
} from "@/lib/pr-overview-snapshot";
import type { PrOverviewStats, PullRequestInfo } from "@/tauri/types";

/** 30s: PRs across every open project change on a human cadence, and
 *  each root is a CLI shell-out. The detail column polls far faster. */
export const PR_OVERVIEW_REFETCH_MS = 30_000;

export const prOverviewKey = (projectRoot: string) =>
  ["prs", "overview", projectRoot] as const;

/** The expensive half. A separate key so it can fail, retry and land on
 *  its own schedule without the listing waiting on it. */
export const prOverviewStatsKey = (projectRoot: string) =>
  ["prs", "overview-stats", projectRoot] as const;

export interface ProjectRoot {
  path: string;
  /** Wire `provider_kind` off any workspace in this root. `null` means
   *  no workspace here has named a host yet — which is not the same as
   *  "an unrecognised host", and must not be flattened into one. */
  providerKind: string | null;
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
        if (existing.providerKind == null && ws.provider_kind) {
          existing.providerKind = ws.provider_kind;
        }
        continue;
      }
      roots.set(path, {
        path,
        providerKind: ws.provider_kind ?? null,
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
    // `null` stays `null`. "none" is the host saying this pull request has
    // no checks; the historical list simply never asked, and flattening
    // the two would draw a confident "no checks" dot — and make a
    // `ci:none` search match rows nobody measured.
    checks:
      pr.checks_passing === true
        ? "passing"
        : pr.checks_passing === false
          ? "failing"
          : null,
    review_requested_from: pr.review_requests ?? [],
    updated_at: pr.updated_at,
    url: pr.url,
    state: pr.state,
    projectRoot: root.path,
    repo: repoSlugFromUrl(pr.url) ?? root.name,
    providerKind: root.providerKind ?? "github",
  };
}

/**
 * Fold the expensive half into a row.
 *
 * Every field uses `??` against the value the row already has, in that
 * order, and that is the whole contract: stats can turn a blank into a
 * value and can update a value it previously supplied, but a `null`
 * coming back from a partial or failed stats call can never blank
 * something the user is looking at. A row with no matching stats entry
 * is returned untouched — identity preserved, so React doesn't re-render
 * a list that didn't change.
 */
export function mergeStats(
  row: PrRow,
  stats: ReadonlyMap<number, PrOverviewStats> | null,
): PrRow {
  const stat = stats?.get(row.number);
  if (!stat) return row;
  const checks = stat.checks ?? row.checks;
  const additions = stat.additions ?? row.additions;
  const deletions = stat.deletions ?? row.deletions;
  if (checks === row.checks && additions === row.additions && deletions === row.deletions) {
    return row;
  }
  return { ...row, checks, additions, deletions };
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
  /** True while any root is *still awaiting its first answer* and is
   *  painting snapshot rows in the meantime. The header labels the age
   *  differently, and the toasts stay quiet — see `pr-event-toasts`.
   *
   *  A root that has already failed does not count: it is a footer line
   *  in `failures`, not a root we are waiting on, so it cannot pin the
   *  page in carried mode and silence every toast for the session. */
  carried: boolean;
  /** When the carried rows were fetched, for "as of 2h ago". */
  carriedAt: number | null;
  /** Every root failed. Distinct from "some root failed", which is a
   *  footer line; this is the case the stale strip exists for. */
  allRootsFailed: boolean;
  /** The refresh cycle settled and produced nothing new: something
   *  failed, nothing succeeded, and nothing is still in flight.
   *
   *  The "nothing in flight" half is what keeps the strip from flashing
   *  on every launch — with one permanently unreachable repository among
   *  five, the failure lands first and the successes arrive a moment
   *  later, and announcing a failed refresh in that gap would be both
   *  alarming and wrong. */
  refreshFailed: boolean;
  isLoading: boolean;
  refresh: () => void;
}

/**
 * The last write, so the three surfaces sharing this hook write once
 * between them rather than three times per poll.
 *
 * Module scope on purpose: they are three components with one query
 * cache, and the thing being deduplicated is a side effect on storage
 * that is global too.
 */
let lastSnapshotWrite = "";

/** Test seam — a fresh module state without reloading the module. */
export function _resetSnapshotWriteGuard(): void {
  lastSnapshotWrite = "";
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

  // The expensive half, fired per root only once that root's listing has
  // landed *and* has said it doesn't know the answer. A host that fills
  // `checks` in its first call (GitLab reads the pipeline straight off
  // the merge request list) never makes this request at all — the gate
  // is the data, not a hardcoded list of product names.
  const needsStats = results.map(
    (result) => result.data?.items.some((item) => item.checks == null) ?? false,
  );

  const statsResults = useQueries({
    queries: roots.map((root, i) => ({
      queryKey: prOverviewStatsKey(root.path),
      queryFn: () => listPrsOverviewStats(root.path),
      enabled: enabled && needsStats[i],
      staleTime: PR_OVERVIEW_REFETCH_MS,
      refetchInterval: PR_OVERVIEW_REFETCH_MS,
      // No retry. The row keeps whatever it has and the next poll tries
      // again in thirty seconds; a second attempt at a slow call is the
      // one thing that would make the slow half slower.
      retry: false,
    })),
  });

  // Closed and merged rows are a second, cheaper list, fetched only
  // when the dropdown asks for them. The open rows keep coming from the
  // overview so they keep the review requests that the historical list
  // has no reason to carry.
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

  // ── The carried paint ──
  //
  // Read once per distinct set of roots. Re-reading on every render
  // would be both wasteful and wrong: the write below happens on the
  // same roots, and a re-read would resurrect rows a fresh fetch had
  // just replaced.
  const rootPaths = useMemo(() => roots.map((root) => root.path), [roots]);
  const storageKey = rootPaths.length > 0 ? prSnapshotKey(rootPaths) : "";
  const snapshot = useMemo(
    () => (storageKey ? readPrOverviewSnapshot(rootPaths) : null),
    // The key *is* the identity of the root set, which is exactly the
    // condition under which a re-read is correct.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [storageKey],
  );

  const rows: PrRow[] = [];
  const viewerByRoot = new Map<string, string | null>();
  const failures: RootFailure[] = [];
  let updatedAt: number | null = null;
  let statsUpdatedAt = 0;
  let isLoading = false;
  let carried = false;
  let statsPending = false;
  let anyRootFresh = false;

  results.forEach((result, i) => {
    const root = roots[i];
    if (!root) return;
    if (result.isLoading) isLoading = true;

    if (result.data) {
      anyRootFresh = true;
      viewerByRoot.set(root.path, result.data.viewer);

      const stats = statsResults[i]?.data ?? null;
      const byNumber = stats ? new Map(stats.map((stat) => [stat.number, stat])) : null;
      if (needsStats[i] && statsResults[i]?.isLoading) statsPending = true;
      const statsAt = statsResults[i]?.dataUpdatedAt ?? 0;
      if (statsAt > statsUpdatedAt) statsUpdatedAt = statsAt;

      for (const item of result.data.items) {
        rows.push(
          mergeStats(
            {
              ...item,
              projectRoot: root.path,
              repo: repoSlugFromUrl(item.url) ?? root.name,
              // An unnamed host is GitHub, per the wire's back-compat rule
              // (`resolveProvider(null)`). Flattening it to "unknown" instead
              // made every GitHub pull request on this page render as an
              // unsupported host — no verdicts, no diff, no merge.
              providerKind: root.providerKind ?? "github",
            },
            byNumber,
          ),
        );
      }
    } else if (snapshot && !result.isError) {
      // Nothing from the host for this root *yet*. Rather than a hole in
      // the list, last session's rows for *this* root — dropped whole
      // the instant the real answer arrives above.
      //
      // "Yet" is load-bearing, and is why an errored root is excluded.
      // A root that has answered — with a failure — is not waiting on
      // anything, and one permanently unreachable repository must not
      // keep the whole page in carried mode for the rest of the session:
      // `carried` blocks every snapshot write and silences every toast,
      // so a single dead root would otherwise mean no review-requested
      // and no CI-failed toast for *any* repository, forever. It becomes
      // a footer line via `failures` below instead, which is also what
      // the write rule further down already assumes ("the failure itself
      // is never written: it isn't in the rows").
      const carriedRows = snapshot.rows.filter((row) => row.projectRoot === root.path);
      if (carriedRows.length > 0 || root.path in snapshot.viewerByRoot) {
        carried = true;
        rows.push(...carriedRows);
        viewerByRoot.set(root.path, snapshot.viewerByRoot[root.path] ?? null);
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

  // ── Recording the refresh ──
  //
  // Only the default view, never while anything on screen is itself
  // carried, and only once the stats that were asked for have settled —
  // a snapshot written mid-flight would persist blanks that were about
  // to be filled, and hand the next cold start a list with no colour in
  // it.
  //
  // A failing root deliberately does *not* block the write. One
  // permanently unreachable repository would otherwise mean the user
  // never gets a carried paint for the repositories that do work, which
  // is the same page-is-blank-on-launch problem with extra steps. The
  // failure itself is never written: it isn't in the rows, and the roots
  // are all refetched next launch regardless.
  //
  // What keeps that honest is `savedAt` — the listing's own fetch time,
  // not the write time. A refresh that fails outright doesn't move it,
  // so it cannot re-date rows nobody re-fetched, and the age the header
  // prints is always the age of the data.
  const canWrite =
    stateFilter === "open" &&
    anyRootFresh &&
    !carried &&
    !statsPending &&
    updatedAt != null &&
    storageKey !== "";
  const writeToken = canWrite ? `${storageKey}|${updatedAt}|${statsUpdatedAt}` : "";

  useEffect(() => {
    if (!writeToken || writeToken === lastSnapshotWrite) return;
    lastSnapshotWrite = writeToken;
    writePrOverviewSnapshot(rootPaths, rows, viewerByRoot, updatedAt ?? Date.now());
    // `rows` is rebuilt every render by design; the token is what says
    // whether its contents actually changed.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [writeToken]);

  const refresh = () => {
    queryClient.invalidateQueries({ predicate: (q) => q.queryKey[0] === "prs" });
  };

  return {
    rows: visible,
    viewerByRoot,
    failures,
    roots,
    updatedAt,
    carried,
    carriedAt: carried ? (snapshot?.savedAt ?? null) : null,
    allRootsFailed: roots.length > 0 && failures.length === roots.length,
    refreshFailed: failures.length > 0 && !anyRootFresh && !isLoading,
    isLoading,
    refresh,
  };
}
