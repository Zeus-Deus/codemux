import { useState, useEffect, useCallback } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Skeleton } from "@/components/ui/skeleton";
import {
  checkGithubRepo,
  getBranchPullRequest,
  getPullRequestChecks,
  getPrReviewComments,
  getPrInlineComments,
  getGitBranchInfo,
  gitPushChanges,
  getDefaultBranch,
  refreshWorkspacePr,
} from "@/tauri/commands";
import type {
  WorkspaceSnapshot,
  GhStatus,
  ProviderAuthStatus,
  ProviderOperations,
  PullRequestInfo,
  CheckInfo,
  ReviewComment,
  InlineReviewComment,
} from "@/tauri/types";
import {
  providerForWorkspace,
  repoSlugFromUrl,
  type ProviderPresentation,
} from "@/lib/source-control";
import {
  fetchProviderAuth,
  getCachedProviderAuth,
  NO_OPERATIONS,
  PROVIDER_AUTH_TTL_MS,
  _resetProviderAuthCache,
} from "@/lib/provider-auth";
import { isPrOnCurrentBranch } from "@/components/github/pr-status-icon";
import { toast } from "@/lib/toast";
import { useUIStore } from "@/stores/ui-store";
import { CreatePrForm } from "./review/create-pr-form";
import { ReviewDetail } from "./review/review-detail";
import { ReviewThreads } from "./review/review-threads";
import { IncomingPrsView } from "./review/incoming-prs-view";
import {
  BranchLocalOnlyState,
  CliMissingState,
  NoPullRequestState,
  RepoUnreachableState,
  SignedOutState,
  UnsupportedHostState,
} from "./review/review-empty-states";

interface Props {
  workspace: WorkspaceSnapshot;
}

// ── Module-level caches with TTL ──
//
// Only successful results are cached. NotAuthenticated/NotInstalled and
// "no integration for this repo" responses bypass the cache so the user
// sees the recovery (e.g. after a CLI login or after a `git remote add
// origin`) on the very next render, not 60 seconds later.
//
// The auth half of this lives in `@/lib/provider-auth`, shared with the
// composer and the new-workspace preflight and keyed by *path* as well
// as product — a slot keyed by product alone would serve one self-hosted
// instance's login for another's workspace, which is the very thing the
// host-scoped probe exists to prevent.
export const CACHE_TTL_MS = PROVIDER_AUTH_TTL_MS;

interface CacheEntry<T> { value: T; ts: number; }

/**
 * The probe's verdict in the vocabulary this panel renders.
 *
 * `GhStatus` covers the three states a CLI can be in. "Unsupported" is
 * the fourth and it is not one of them: a checkout on a host Codemux has
 * no adapter for has no CLI to install or sign in to, so it must render
 * the not-a-supported-repo state rather than an install prompt for a
 * tool that is very possibly already installed.
 */
type ProviderGate = GhStatus | { status: "Unsupported" };

const repoCheckCache = new Map<string, CacheEntry<boolean>>();

/** Auth status for one checkout, if a usable verdict is still warm. */
function cachedStatus(cwd: string, kind: string): ProviderAuthStatus | null {
  return getCachedProviderAuth(cwd, kind);
}

export function getCachedRepoCheck(key: string): boolean | undefined {
  const entry = repoCheckCache.get(key);
  if (!entry || Date.now() - entry.ts > CACHE_TTL_MS) {
    repoCheckCache.delete(key);
    return undefined;
  }
  return entry.value;
}

/** Stores only positive repo-check results; unsupported paths re-check
 *  every call. */
export function setCachedRepoCheck(key: string, value: boolean): void {
  if (!value) return;
  repoCheckCache.set(key, { value, ts: Date.now() });
}

/** Reset all caches — for tests only. */
export function _resetCaches(): void {
  repoCheckCache.clear();
  _resetProviderAuthCache();
}

/**
 * Auth status for whichever product serves this workspace.
 *
 * One host-scoped probe for every product, including GitHub: the backend
 * resolves the checkout's provider and asks *that* adapter, which for
 * GitHub is the same `gh auth status` call this used to make directly.
 */
async function fetchProviderAuthStatus(
  cwd: string,
  provider: ProviderPresentation,
): Promise<ProviderAuthStatus> {
  return fetchProviderAuth(cwd, provider.kind);
}

/** The probe's verdict in the vocabulary this panel already renders. */
function toProviderGate(status: ProviderAuthStatus): ProviderGate {
  if (!status.supported) return { status: "Unsupported" };
  if (!status.installed) return { status: "NotInstalled" };
  if (!status.authenticated) return { status: "NotAuthenticated" };
  return { status: "Authenticated", username: status.username ?? "" };
}

// ── Helpers ──

function ReviewSkeleton() {
  return (
    <div className="space-y-3 p-3">
      <div className="space-y-1.5">
        <div className="flex items-center gap-1.5">
          <Skeleton className="h-4 w-14" />
          <Skeleton className="h-3 w-8" />
        </div>
        <Skeleton className="h-4 w-3/4" />
        <Skeleton className="h-3 w-1/3" />
      </div>
      <Skeleton className="h-3 w-24" />
      <div className="space-y-1">
        <Skeleton className="h-3 w-full" />
        <Skeleton className="h-3 w-full" />
        <Skeleton className="h-3 w-2/3" />
      </div>
      <Skeleton className="h-7 w-full" />
    </div>
  );
}

// ── Main ReviewPanel ──

export function ReviewPanel({ workspace }: Props) {
  const cwd = workspace.worktree_path ?? workspace.cwd;
  const provider = providerForWorkspace(workspace);
  /**
   * A pull request this pane just opened, held until the backend's
   * workspace snapshot catches up.
   *
   * `pr_number` arrives on the snapshot from a poll that runs on its own
   * schedule, so without this the panel would drop back to "No pull
   * request yet" for a few seconds immediately after creating one — the
   * one moment the user is certain there is one.
   */
  const [justCreated, setJustCreated] = useState<PullRequestInfo | null>(null);

  // Strictly the checked-out branch's PR. The workspace snapshot may also
  // carry a *badge-only* side-branch association (a PR the workspace opened
  // from a branch it has since left — see `isPrOnCurrentBranch`), and this
  // panel is a working surface, not a badge: honouring one here would review
  // a branch the user is not on while hiding the "Create PR" affordance for
  // the branch they actually are on.
  const hasPr =
    justCreated != null ||
    (workspace.pr_number != null &&
      isPrOnCurrentBranch(workspace.pr_head_branch, workspace.git_branch));
  const prNumber = workspace.pr_number ?? justCreated?.number ?? null;

  // The snapshot has caught up; the local hold is no longer needed.
  useEffect(() => {
    if (workspace.pr_number != null) setJustCreated(null);
  }, [workspace.pr_number]);

  const [ghStatus, setGhStatus] = useState<ProviderGate | null>(() => {
    const cached = cachedStatus(cwd, provider.kind);
    return cached ? toProviderGate(cached) : null;
  });
  // The per-operation declarations for this checkout, from the same
  // probe that answers the auth gate — so no surface needs a second
  // command, and no control renders before its right to exist is known.
  const [operations, setOperations] = useState<ProviderOperations>(
    () => cachedStatus(cwd, provider.kind)?.operations ?? NO_OPERATIONS,
  );
  const [repoSupported, setRepoSupported] = useState<boolean | null>(
    getCachedRepoCheck(cwd) ?? null,
  );
  const [initialLoading, setInitialLoading] = useState(
    cachedStatus(cwd, provider.kind) === null ||
      getCachedRepoCheck(cwd) === undefined,
  );
  const [showCreateForm, setShowCreateForm] = useState(false);

  const queryClient = useQueryClient();
  const [defaultBranch, setDefaultBranch] = useState<string | null>(null);
  const incomingRefreshKey = 0;

  const isBaseBranch = defaultBranch != null && workspace.git_branch === defaultBranch;

  // Detect default branch
  useEffect(() => {
    getDefaultBranch(cwd).then(setDefaultBranch).catch(() => setDefaultBranch(null));
  }, [cwd]);

  // Auth init — uses module-level cache with TTL
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const status = await fetchProviderAuthStatus(cwd, provider);
        if (cancelled) return;
        const gate = toProviderGate(status);
        setGhStatus(gate);
        setOperations(status.operations ?? NO_OPERATIONS);

        if (gate.status !== "Authenticated") {
          setInitialLoading(false);
          return;
        }

        let isRepo = getCachedRepoCheck(cwd);
        if (isRepo === undefined) {
          isRepo = await checkGithubRepo(cwd);
          setCachedRepoCheck(cwd, isRepo);
        }
        if (cancelled) return;
        setRepoSupported(isRepo);
      } catch (err) {
        console.error("Source control auth init failed:", err);
      } finally {
        if (!cancelled) setInitialLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [cwd, provider]);

  // ── React Query: PR data ──
  //
  // Each query is keyed by (workspaceId, pr_number) so switching
  // workspaces auto-cancels in-flight calls for the previous workspace.
  // Cadence: PR detail + checks at 2.5s so a check going green shows up
  // without a manual refresh; reviews + inline comments at 30s, since
  // they change far less often and carry larger payloads.
  const detailsEnabled =
    !initialLoading &&
    ghStatus?.status === "Authenticated" &&
    repoSupported === true &&
    hasPr;

  const prDetailQuery = useQuery({
    queryKey: ["pr", "detail", workspace.workspace_id, prNumber] as const,
    queryFn: () => getBranchPullRequest(cwd),
    enabled: detailsEnabled,
    staleTime: 2_500,
    refetchInterval: 2_500,
  });

  const checksQuery = useQuery({
    queryKey: ["pr", "checks", workspace.workspace_id, prNumber] as const,
    queryFn: () => getPullRequestChecks(cwd),
    enabled: detailsEnabled,
    staleTime: 2_500,
    refetchInterval: 2_500,
  });

  const reviewsQuery = useQuery({
    queryKey: ["pr", "reviews", workspace.workspace_id, prNumber] as const,
    queryFn: () => getPrReviewComments(cwd),
    enabled: detailsEnabled,
    staleTime: 30_000,
    refetchInterval: 30_000,
  });

  const inlineQuery = useQuery({
    queryKey: ["pr", "inline", workspace.workspace_id, prNumber] as const,
    queryFn: () => {
      if (prNumber == null) return Promise.resolve([] as InlineReviewComment[]);
      return getPrInlineComments(cwd, prNumber);
    },
    enabled: detailsEnabled && prNumber != null,
    staleTime: 30_000,
    refetchInterval: 30_000,
  });

  // Is this branch on the remote at all? Decides which of the two
  // no-PR empty states applies.
  const branchInfoQuery = useQuery({
    queryKey: ["pr", "branch-info", workspace.workspace_id, workspace.git_branch] as const,
    queryFn: () => getGitBranchInfo(cwd),
    enabled: !initialLoading && !hasPr,
    staleTime: 10_000,
  });

  // The pull request this pane just opened stands in until the first
  // poll answers for it, so the panel never flashes a skeleton over a
  // pull request the user has just watched itself create.
  const pr: PullRequestInfo | null = prDetailQuery.data ?? justCreated ?? null;
  const checks: CheckInfo[] = checksQuery.data ?? [];
  const reviews: ReviewComment[] = reviewsQuery.data ?? [];
  const inlineComments: InlineReviewComment[] = inlineQuery.data ?? [];

  /**
   * How old the newest good data is, when the polls are currently
   * failing — null while they're healthy.
   *
   * Binding rule 2: a failed refresh never blanks what is being read.
   * The panel keeps rendering the last good PR and says how old it is;
   * only a PR that has *never* loaded gets a skeleton.
   */
  const staleAgeMs =
    prDetailQuery.isError && prDetailQuery.dataUpdatedAt > 0
      ? Date.now() - prDetailQuery.dataUpdatedAt
      : null;

  // Helper to invalidate every PR query for this workspace at once.
  const invalidatePrQueries = useCallback(() => {
    queryClient.invalidateQueries({
      predicate: (q) =>
        q.queryKey[0] === "pr" &&
        q.queryKey[2] === workspace.workspace_id,
    });
  }, [queryClient, workspace.workspace_id]);

  const openChangesPane = useCallback(() => {
    useUIStore.getState().setRightPanelTab(workspace.workspace_id, "changes");
  }, [workspace.workspace_id]);

  /**
   * The form's success path: the pane becomes the review view for the
   * pull request that was just opened, without an intervening blank.
   *
   * Three things in order — hold the new pull request locally, seed the
   * detail cache under its number so the first render has real data, and
   * ask the backend to re-read the workspace's pull request so
   * `pr_number` lands on the snapshot and the local hold can retire.
   */
  const handlePrCreated = (newPr: PullRequestInfo) => {
    setJustCreated(newPr);
    queryClient.setQueryData(
      ["pr", "detail", workspace.workspace_id, newPr.number],
      newPr,
    );
    setShowCreateForm(false);
    invalidatePrQueries();
    void refreshWorkspacePr(workspace.workspace_id).catch(() => {});
  };

  // ── Render ──

  if (initialLoading) {
    return <ReviewSkeleton />;
  }

  // A host with no adapter is not a missing-CLI problem, and telling a
  // user with `gh` on their PATH to install it is simply wrong — so
  // both no-adapter cases are answered before the CLI states below.
  if (ghStatus?.status === "Unsupported" || repoSupported === false) {
    return (
      <UnsupportedHostState provider={provider} url={workspace.pr_url ?? null} />
    );
  }

  if (ghStatus?.status === "NotInstalled") {
    return <CliMissingState provider={provider} />;
  }

  if (ghStatus?.status === "NotAuthenticated") {
    return <SignedOutState provider={provider} />;
  }

  // Authenticated and the repo is supported, but the PR itself can't be
  // fetched and never has been: that is a reachability problem, not an
  // empty state.
  if (hasPr && !pr && prDetailQuery.isError) {
    return (
      <RepoUnreachableState
        repoSlug={repoSlugFromUrl(workspace.pr_url)}
        provider={provider}
        onRetry={invalidatePrQueries}
      />
    );
  }

  if (hasPr && pr) {
    return (
      // No scroll wrapper: the detail lays itself out to the pane's full
      // height and scrolls only its tab body, so the action bar sits on
      // the bottom edge of the pane rather than wherever the description
      // happened to end.
      <ReviewDetail
        pr={pr}
        checks={checks}
        reviews={reviews}
        inlineComments={inlineComments}
        checksLoading={checksQuery.isFetching}
        commentsLoading={reviewsQuery.isFetching || inlineQuery.isFetching}
        cwd={cwd}
        workspaceId={workspace.workspace_id}
        projectRoot={workspace.project_root ?? workspace.cwd}
        provider={provider}
        operations={operations}
        repoSlug={repoSlugFromUrl(pr.url) ?? repoSlugFromUrl(workspace.pr_url)}
        viewerLogin={
          ghStatus?.status === "Authenticated" ? ghStatus.username || null : null
        }
        checkedOutHere={isPrOnCurrentBranch(pr.head_branch, workspace.git_branch)}
        gitBehind={workspace.git_behind ?? 0}
        gitDirtyFiles={workspace.git_changed_files ?? 0}
        staleAgeMs={staleAgeMs}
        onRefresh={invalidatePrQueries}
        onOpenChanges={openChangesPane}
      />
    );
  }

  if (hasPr && !pr) {
    return <ReviewSkeleton />;
  }

  if (showCreateForm) {
    return (
      <ScrollArea className="h-full [&_[data-slot=scroll-area-viewport]>div]:!block">
        <CreatePrForm
          cwd={cwd}
          projectRoot={workspace.project_root ?? workspace.cwd}
          branchName={workspace.git_branch}
          defaultBranch={defaultBranch}
          provider={provider}
          onCreated={handlePrCreated}
          onCancel={() => setShowCreateForm(false)}
          onOpenChanges={openChangesPane}
        />
      </ScrollArea>
    );
  }

  if (isBaseBranch) {
    return (
      <ScrollArea className="h-full [&_[data-slot=scroll-area-viewport]>div]:!block">
        <IncomingPrsView
          cwd={cwd}
          baseBranch={defaultBranch!}
          projectRoot={workspace.project_root ?? workspace.cwd}
          providerKind={workspace.provider_kind}
          refreshKey={incomingRefreshKey}
        />
      </ScrollArea>
    );
  }

  const pushed = branchInfoQuery.data?.has_upstream ?? false;
  const dirtyFiles = workspace.git_changed_files ?? 0;

  return (
    <ScrollArea className="h-full [&_[data-slot=scroll-area-viewport]>div]:!block">
      {pushed ? (
        <NoPullRequestState
          branch={workspace.git_branch}
          baseBranch={defaultBranch ?? "main"}
          commitsAhead={workspace.git_ahead ?? 0}
          provider={provider}
          onCreate={() => setShowCreateForm(true)}
          onViewCommits={openChangesPane}
        />
      ) : (
        <BranchLocalOnlyState
          changedFiles={dirtyFiles}
          pushOnly={dirtyFiles === 0}
          onOpenChanges={openChangesPane}
          onCommitAndPush={() => {
            if (dirtyFiles > 0) {
              // Committing needs a message, and the Changes pane is
              // where that conversation already happens.
              openChangesPane();
              return;
            }
            gitPushChanges(cwd, true)
              .then(() => {
                toast.success("Branch pushed");
                invalidatePrQueries();
                void branchInfoQuery.refetch();
              })
              .catch((err) => toast.error(String(err)));
          }}
        />
      )}
    </ScrollArea>
  );
}

// Re-exported so the pane keeps a single import surface for the review
// subtree even though the threads list now renders inside ReviewDetail.
// `repoSlugFromUrl` moved to `@/lib/source-control` when the Pull
// Requests page needed it without pulling the panel in.
export { ReviewThreads, repoSlugFromUrl };
