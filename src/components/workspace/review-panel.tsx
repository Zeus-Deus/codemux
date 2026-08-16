import { useState, useEffect, useCallback } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Skeleton } from "@/components/ui/skeleton";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { ChevronLeft } from "lucide-react";
import {
  checkGithubRepo,
  getBranchPullRequest,
  createPullRequest,
  getPullRequestChecks,
  getPrReviewComments,
  getPrInlineComments,
  getGitBranchInfo,
  gitPushChanges,
  listBranches,
  getDefaultBranch,
} from "@/tauri/commands";
import type {
  WorkspaceSnapshot,
  GhStatus,
  ProviderAuthStatus,
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
  PROVIDER_AUTH_TTL_MS,
  _resetProviderAuthCache,
} from "@/lib/provider-auth";
import { isPrOnCurrentBranch } from "@/components/github/pr-status-icon";
import { toast } from "@/lib/toast";
import { useUIStore } from "@/stores/ui-store";
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
function cachedStatus(cwd: string, kind: string): ProviderGate | null {
  const cached = getCachedProviderAuth(cwd, kind);
  return cached ? toProviderGate(cached) : null;
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
): Promise<ProviderGate> {
  return toProviderGate(await fetchProviderAuth(cwd, provider.kind));
}

/** The probe's verdict in the vocabulary this panel already renders. */
function toProviderGate(status: ProviderAuthStatus): ProviderGate {
  if (!status.supported) return { status: "Unsupported" };
  if (!status.installed) return { status: "NotInstalled" };
  if (!status.authenticated) return { status: "NotAuthenticated" };
  return { status: "Authenticated", username: status.username ?? "" };
}

// ── Helpers ──

function branchToTitle(branch: string | null): string {
  if (!branch) return "";
  return branch
    .replace(/^(feature|fix|chore|docs|refactor|test)[/-]/, "")
    .replace(/[-_]/g, " ")
    .replace(/^\w/, (c) => c.toUpperCase());
}

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

// ── CreatePrForm ──
//
// Untouched by this ship: the create form gets its own redesign later,
// and half-redesigning it would leave two visual languages on the same
// surface.

function CreatePrForm({
  cwd,
  branchName,
  provider,
  onCreated,
  onCancel,
}: {
  cwd: string;
  branchName: string | null;
  provider: ProviderPresentation;
  onCreated: (pr: PullRequestInfo) => void;
  onCancel: () => void;
}) {
  const [title, setTitle] = useState(() => branchToTitle(branchName));
  const [body, setBody] = useState("");
  const [baseBranch, setBaseBranch] = useState("main");
  const [branches, setBranches] = useState<string[]>([]);
  const [branchesLoaded, setBranchesLoaded] = useState(false);
  const [isDraft, setIsDraft] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);

  const loadBranches = useCallback(() => {
    if (branchesLoaded) return;
    listBranches(cwd, false)
      .then((b) => {
        setBranches(b);
        setBranchesLoaded(true);
        if (b.includes("main")) setBaseBranch("main");
        else if (b.includes("master")) setBaseBranch("master");
        else if (b.length > 0) setBaseBranch(b[0]);
      })
      .catch(console.error);
  }, [cwd, branchesLoaded]);

  const handleCreate = async () => {
    if (!title.trim() || creating) return;
    setCreating(true);
    setError(null);
    try {
      const pr = await createPullRequest(
        cwd,
        title.trim(),
        body.trim(),
        baseBranch,
        isDraft,
      );
      onCreated(pr);
    } catch (err) {
      setError(String(err));
    } finally {
      setCreating(false);
    }
  };

  return (
    <div className="space-y-2 p-3">
      <div className="flex items-center gap-1">
        <Button size="icon-xs" variant="ghost" onClick={onCancel} title="Back">
          <ChevronLeft className="h-3 w-3" />
        </Button>
        <p className="text-xs font-medium text-muted-foreground">
          Create {provider.nounTitleCase}
        </p>
      </div>
      <Input
        placeholder={`${provider.shortNoun} title`}
        value={title}
        onChange={(e) => setTitle(e.target.value)}
        onKeyDown={(e) => e.key === "Enter" && handleCreate()}
        className="h-7 text-xs"
      />
      <Textarea
        placeholder="Description (optional)"
        value={body}
        onChange={(e) => setBody(e.target.value)}
        className="text-xs resize-none h-16 min-h-16"
      />
      <div className="flex items-center gap-2">
        <div className="flex-1">
          <Select
            value={baseBranch}
            onValueChange={setBaseBranch}
            onOpenChange={(open) => {
              if (open) loadBranches();
            }}
          >
            <SelectTrigger className="h-7 text-xs">
              <SelectValue placeholder="Base branch" />
            </SelectTrigger>
            <SelectContent>
              {branches.length > 0 ? (
                branches.map((b) => (
                  <SelectItem key={b} value={b} className="text-xs">
                    {b}
                  </SelectItem>
                ))
              ) : (
                <SelectItem value={baseBranch} className="text-xs">
                  {baseBranch}
                </SelectItem>
              )}
            </SelectContent>
          </Select>
        </div>
        <label className="flex items-center gap-1 text-xs text-muted-foreground cursor-pointer">
          <input
            type="checkbox"
            checked={isDraft}
            onChange={(e) => setIsDraft(e.target.checked)}
            className="rounded"
          />
          Draft
        </label>
      </div>
      <Button
        size="xs"
        className="w-full text-xs h-7"
        disabled={!title.trim() || creating}
        onClick={handleCreate}
      >
        {creating ? "Creating..." : `Create ${provider.shortNoun}`}
      </Button>
      {error && <p className="text-xs text-danger break-words">{error}</p>}
    </div>
  );
}

// ── Main ReviewPanel ──

export function ReviewPanel({ workspace }: Props) {
  const cwd = workspace.worktree_path ?? workspace.cwd;
  const provider = providerForWorkspace(workspace);
  // Strictly the checked-out branch's PR. The workspace snapshot may also
  // carry a *badge-only* side-branch association (a PR the workspace opened
  // from a branch it has since left — see `isPrOnCurrentBranch`), and this
  // panel is a working surface, not a badge: honouring one here would review
  // a branch the user is not on while hiding the "Create PR" affordance for
  // the branch they actually are on.
  const hasPr =
    workspace.pr_number != null &&
    isPrOnCurrentBranch(workspace.pr_head_branch, workspace.git_branch);

  const [ghStatus, setGhStatus] = useState<ProviderGate | null>(
    cachedStatus(cwd, provider.kind),
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
        setGhStatus(status);

        if (status.status !== "Authenticated") {
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
    queryKey: ["pr", "detail", workspace.workspace_id, workspace.pr_number] as const,
    queryFn: () => getBranchPullRequest(cwd),
    enabled: detailsEnabled,
    staleTime: 2_500,
    refetchInterval: 2_500,
  });

  const checksQuery = useQuery({
    queryKey: ["pr", "checks", workspace.workspace_id, workspace.pr_number] as const,
    queryFn: () => getPullRequestChecks(cwd),
    enabled: detailsEnabled,
    staleTime: 2_500,
    refetchInterval: 2_500,
  });

  const reviewsQuery = useQuery({
    queryKey: ["pr", "reviews", workspace.workspace_id, workspace.pr_number] as const,
    queryFn: () => getPrReviewComments(cwd),
    enabled: detailsEnabled,
    staleTime: 30_000,
    refetchInterval: 30_000,
  });

  const inlineQuery = useQuery({
    queryKey: ["pr", "inline", workspace.workspace_id, workspace.pr_number] as const,
    queryFn: () => {
      const num = workspace.pr_number;
      if (num == null) return Promise.resolve([] as InlineReviewComment[]);
      return getPrInlineComments(cwd, num);
    },
    enabled: detailsEnabled && workspace.pr_number != null,
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

  const pr: PullRequestInfo | null = prDetailQuery.data ?? null;
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

  const handlePrCreated = (newPr: PullRequestInfo) => {
    // Seed the detail cache directly so the new PR shows up immediately
    // rather than waiting for the workspace state push to flip
    // pr_number, then refresh the rest.
    queryClient.setQueryData(
      ["pr", "detail", workspace.workspace_id, newPr.number],
      newPr,
    );
    setShowCreateForm(false);
    invalidatePrQueries();
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
      <ScrollArea className="h-full [&_[data-slot=scroll-area-viewport]>div]:!block">
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
      </ScrollArea>
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
          branchName={workspace.git_branch}
          provider={provider}
          onCreated={handlePrCreated}
          onCancel={() => setShowCreateForm(false)}
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
