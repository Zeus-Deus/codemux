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
import {
  GitPullRequest,
  AlertCircle,
  ChevronLeft,
} from "lucide-react";
import {
  checkGhStatus,
  checkGithubRepo,
  getBranchPullRequest,
  createPullRequest,
  getPullRequestChecks,
  getPrReviewComments,
  getPrInlineComments,
  listBranches,
  getDefaultBranch,
} from "@/tauri/commands";
import type {
  WorkspaceSnapshot,
  GhStatus,
  PullRequestInfo,
  CheckInfo,
  ReviewComment,
  InlineReviewComment,
} from "@/tauri/types";
import { isPrOnCurrentBranch } from "@/components/github/pr-status-icon";
import { ReviewHeader } from "./review/review-header";
import { ReviewChecks } from "./review/review-checks";
import { ReviewThreads } from "./review/review-threads";
import { IncomingPrsView } from "./review/incoming-prs-view";

interface Props {
  workspace: WorkspaceSnapshot;
}

// ── Module-level caches with TTL ──
//
// Only successful results are cached. NotAuthenticated/NotInstalled and
// "not a GitHub repo" responses bypass the cache so the user sees the
// recovery (e.g. after `gh auth login` or after a `git remote add origin`)
// on the very next render, not 60 seconds later.
export const CACHE_TTL_MS = 60_000;

interface CacheEntry<T> { value: T; ts: number; }

let ghStatusCache: CacheEntry<GhStatus> | null = null;
const repoCheckCache = new Map<string, CacheEntry<boolean>>();

export function getCachedGhStatus(): GhStatus | null {
  if (!ghStatusCache || Date.now() - ghStatusCache.ts > CACHE_TTL_MS) {
    ghStatusCache = null;
    return null;
  }
  return ghStatusCache.value;
}

/** Stores only successful auth statuses; failures are dropped on the floor. */
export function setCachedGhStatus(value: GhStatus): void {
  if (value.status !== "Authenticated") return;
  ghStatusCache = { value, ts: Date.now() };
}

export function getCachedRepoCheck(key: string): boolean | undefined {
  const entry = repoCheckCache.get(key);
  if (!entry || Date.now() - entry.ts > CACHE_TTL_MS) {
    repoCheckCache.delete(key);
    return undefined;
  }
  return entry.value;
}

/** Stores only positive repo-check results; non-GitHub paths re-check every call. */
export function setCachedRepoCheck(key: string, value: boolean): void {
  if (!value) return;
  repoCheckCache.set(key, { value, ts: Date.now() });
}

/** Reset all caches — for tests only. */
export function _resetCaches(): void {
  ghStatusCache = null;
  repoCheckCache.clear();
}

// ── Helpers ──

function branchToTitle(branch: string | null): string {
  if (!branch) return "";
  return branch
    .replace(/^(feature|fix|chore|docs|refactor|test)[/-]/, "")
    .replace(/[-_]/g, " ")
    .replace(/^\w/, (c) => c.toUpperCase());
}

// ── Small sub-components ──

function StatusMessage({
  icon,
  message,
}: {
  icon: React.ReactNode;
  message: string;
}) {
  return (
    <div className="flex flex-col items-center justify-center py-8 text-muted-foreground gap-2">
      {icon}
      <p className="text-xs text-center px-4">{message}</p>
    </div>
  );
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

function CreatePrForm({
  cwd,
  branchName,
  onCreated,
  onCancel,
}: {
  cwd: string;
  branchName: string | null;
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
        <Button
          size="icon-xs"
          variant="ghost"
          onClick={onCancel}
          title="Back"
        >
          <ChevronLeft className="h-3 w-3" />
        </Button>
        <p className="text-xs font-medium text-muted-foreground">
          Create Pull Request
        </p>
      </div>
      <Input
        placeholder="PR title"
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
        {creating ? "Creating..." : "Create PR"}
      </Button>
      {error && <p className="text-xs text-danger break-words">{error}</p>}
    </div>
  );
}

// ── NoReviewView ──

function NoReviewView({
  cwd,
  branchName,
  onCreated,
}: {
  cwd: string;
  branchName: string | null;
  onCreated: (pr: PullRequestInfo) => void;
}) {
  const [showForm, setShowForm] = useState(false);

  if (showForm) {
    return (
      <CreatePrForm
        cwd={cwd}
        branchName={branchName}
        onCreated={onCreated}
        onCancel={() => setShowForm(false)}
      />
    );
  }

  return (
    <div className="flex flex-col items-center justify-center py-8 gap-3">
      <GitPullRequest className="h-8 w-8 text-muted-foreground/30" />
      <p className="text-xs text-muted-foreground">
        No pull request for this branch
      </p>
      <Button variant="secondary" size="xs" className="text-xs" onClick={() => setShowForm(true)}>
        Create Pull Request
      </Button>
    </div>
  );
}

// ── ReviewView ──

function ReviewView({
  pr,
  checks,
  reviews,
  inlineComments,
  checksLoading,
  commentsLoading,
}: {
  pr: PullRequestInfo;
  checks: CheckInfo[];
  reviews: ReviewComment[];
  inlineComments: InlineReviewComment[];
  checksLoading: boolean;
  commentsLoading: boolean;
}) {
  // Stripped to match Superset's resting layout: title + status badge
  // + checks + comments only. Composer (Approve / Request changes /
  // Comment), merge controls, and deployments were intentionally
  // removed — composer because Superset has no equivalent (verified
  // against the reference clone), merge controls because the Changes
  // panel toolbar already has a merge dropdown, deployments because
  // it's not part of Superset's Review surface. Backends for the
  // composer and merge are retained for potential future re-wire.
  return (
    <div className="space-y-2 p-3">
      <ReviewHeader pr={pr} />

      <div className="border-t border-border/30" />

      <ReviewChecks checks={checks} isLoading={checksLoading} />
      <ReviewThreads
        reviews={reviews}
        inlineComments={inlineComments}
        isLoading={commentsLoading}
      />
    </div>
  );
}

// ── Main ReviewPanel ──

export function ReviewPanel({ workspace }: Props) {
  const cwd = workspace.worktree_path ?? workspace.cwd;
  // Strictly the checked-out branch's PR. The workspace snapshot may also
  // carry a *badge-only* side-branch association (a PR the workspace opened
  // from a branch it has since left — see `isPrOnCurrentBranch`), and this
  // panel is a working surface, not a badge: honouring one here would review
  // a branch the user is not on while hiding the "Create PR" affordance for
  // the branch they actually are on.
  const hasPr =
    workspace.pr_number != null &&
    isPrOnCurrentBranch(workspace.pr_head_branch, workspace.git_branch);

  const [ghStatus, setGhStatus] = useState<GhStatus | null>(getCachedGhStatus());
  const [isGithubRepo, setIsGithubRepo] = useState<boolean | null>(
    getCachedRepoCheck(cwd) ?? null,
  );
  const [initialLoading, setInitialLoading] = useState(
    getCachedGhStatus() === null || getCachedRepoCheck(cwd) === undefined,
  );

  const queryClient = useQueryClient();
  const [defaultBranch, setDefaultBranch] = useState<string | null>(null);
  // The IncomingPrsView used to be re-fetched via the now-removed
  // refresh button. Auto-poll covers regular PR data; the incoming
  // list relies on its own internal mount cycle (and on the 60s Rust
  // background poll updating workspace.pr_state for each row).
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
        let status = getCachedGhStatus();
        if (!status) {
          status = await checkGhStatus();
          setCachedGhStatus(status);
        }
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
        setIsGithubRepo(isRepo);
      } catch (err) {
        console.error("PR auth init failed:", err);
      } finally {
        if (!cancelled) setInitialLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [cwd]);

  // ── React Query: PR data ──
  //
  // Each query is keyed by (workspaceId, pr_number) so switching
  // workspaces auto-cancels in-flight calls for the previous workspace
  // — the stale-data-flashing bug the analysis (§4) called out.
  // staleTime + refetchInterval keep the surface honest without
  // hammering `gh`. Reviews/inline comments are slower (30s) than
  // PR/checks/deploys (10s) since they change less often.
  //
  // Caveat: AbortSignal is plumbed through but the underlying Rust
  // commands are sync `pub fn` using `std::process::Command`, so a
  // canceled query stops the JS from caring about the result; the
  // `gh` subprocess keeps running until natural completion (already
  // bounded by the existing 10s timeout in `run_gh_timed`). Wasted
  // CPU on a stale call is minimal; the fix that matters here is that
  // the *result* never lands in the new workspace's UI.
  const detailsEnabled =
    !initialLoading &&
    ghStatus?.status === "Authenticated" &&
    isGithubRepo === true &&
    hasPr;

  // Cadence per Superset (`ChangesView.tsx:105` polls at 2500ms when
  // active). PR detail + checks update fast so the user sees their
  // PR move from pending → success without a manual refresh.
  // Reviews + inline comments stay at 30s — they change less often
  // and the JSON payloads are larger.
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

  const pr: PullRequestInfo | null = prDetailQuery.data ?? null;
  const checks: CheckInfo[] = checksQuery.data ?? [];
  const reviews: ReviewComment[] = reviewsQuery.data ?? [];
  const inlineComments: InlineReviewComment[] = inlineQuery.data ?? [];
  // Only the primary detail query's error feeds the banner — the
  // others swallow their errors via empty-array fallbacks below since
  // their failures are usually transient and the empty sections
  // render fine.
  const fetchError = prDetailQuery.error
    ? String((prDetailQuery.error as Error).message ?? prDetailQuery.error)
    : null;

  // Helper to invalidate every PR query for this workspace at once —
  // used by manual refresh and (in §3.3) by commit/push/merge events.
  const invalidatePrQueries = useCallback(() => {
    queryClient.invalidateQueries({
      predicate: (q) =>
        q.queryKey[0] === "pr" &&
        q.queryKey[2] === workspace.workspace_id,
    });
  }, [queryClient, workspace.workspace_id]);

  const handlePrCreated = (newPr: PullRequestInfo) => {
    // Seed the detail cache directly so the user sees the new PR
    // immediately without waiting for the workspace state push to
    // flip pr_number. Then invalidate the rest so checks/reviews/
    // deploys fetch fresh.
    queryClient.setQueryData(
      ["pr", "detail", workspace.workspace_id, newPr.number],
      newPr,
    );
    invalidatePrQueries();
  };

  // ── Render ──

  if (initialLoading) {
    return <ReviewSkeleton />;
  }

  if (ghStatus?.status === "NotInstalled") {
    return (
      <StatusMessage
        icon={<AlertCircle className="h-6 w-6 opacity-40" />}
        message="GitHub CLI (gh) is not installed. Install it from cli.github.com"
      />
    );
  }

  if (ghStatus?.status === "NotAuthenticated") {
    return (
      <StatusMessage
        icon={<AlertCircle className="h-6 w-6 opacity-40" />}
        message="Not authenticated. Run: gh auth login"
      />
    );
  }

  if (isGithubRepo === false) {
    return (
      <StatusMessage
        icon={<GitPullRequest className="h-6 w-6 opacity-40" />}
        message="Not a GitHub repository"
      />
    );
  }

  return (
    <ScrollArea className="h-full [&_[data-slot=scroll-area-viewport]>div]:!block">
      {/* No refresh button — auto-poll handles freshness (2.5s for PR
          detail and checks, 30s for reviews + inline comments while the
          tab is active). Mirrors Superset's pattern. */}
      {fetchError && (
        <div className="mx-3 mb-1 flex items-start gap-1.5 rounded bg-danger/10 px-2 py-1.5 text-xs text-danger">
          <AlertCircle className="h-3 w-3 mt-0.5 shrink-0" />
          <span className="break-words">{fetchError}</span>
        </div>
      )}
      {hasPr && pr ? (
        <ReviewView
          pr={pr}
          checks={checks}
          reviews={reviews}
          inlineComments={inlineComments}
          checksLoading={checksQuery.isFetching}
          commentsLoading={reviewsQuery.isFetching || inlineQuery.isFetching}
        />
      ) : hasPr && !pr ? (
        <ReviewSkeleton />
      ) : isBaseBranch ? (
        <IncomingPrsView
          cwd={cwd}
          baseBranch={defaultBranch!}
          projectRoot={workspace.project_root ?? workspace.cwd}
          refreshKey={incomingRefreshKey}
        />
      ) : (
        <NoReviewView
          cwd={cwd}
          branchName={workspace.git_branch}
          onCreated={handlePrCreated}
        />
      )}
    </ScrollArea>
  );
}
