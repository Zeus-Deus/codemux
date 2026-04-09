import { useState, useEffect, useCallback } from "react";
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
  RefreshCw,
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
  getPrDeployments,
  listBranches,
  refreshWorkspacePr,
  getDefaultBranch,
} from "@/tauri/commands";
import type {
  WorkspaceSnapshot,
  GhStatus,
  PullRequestInfo,
  CheckInfo,
  ReviewComment,
  InlineReviewComment,
  DeploymentInfo,
} from "@/tauri/types";
import { PrHeader } from "./pr/pr-header";
import { PrChecks } from "./pr/pr-checks";
import { PrReviews } from "./pr/pr-reviews";
import { PrReviewActions } from "./pr/pr-review-actions";
import { PrDeployments } from "./pr/pr-deployments";
import { PrMergeControls } from "./pr/pr-merge-controls";
import { IncomingPrsView } from "./pr/incoming-prs-view";

interface Props {
  workspace: WorkspaceSnapshot;
}

// ── Module-level caches with TTL ──
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

export function setCachedGhStatus(value: GhStatus): void {
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

export function setCachedRepoCheck(key: string, value: boolean): void {
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

function PrSkeleton() {
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

// ── NoPrView ──

function NoPrView({
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

// ── PrView ──

function PrView({
  pr,
  checks,
  reviews,
  inlineComments,
  deployments,
  cwd,
  onRefresh,
}: {
  pr: PullRequestInfo;
  checks: CheckInfo[];
  reviews: ReviewComment[];
  inlineComments: InlineReviewComment[];
  deployments: DeploymentInfo[];
  cwd: string;
  onRefresh: () => void;
}) {
  return (
    <div className="space-y-2 p-3">
      <PrHeader pr={pr} />

      <div className="border-t border-border/30" />

      <PrChecks checks={checks} />
      <PrReviews reviews={reviews} inlineComments={inlineComments} />

      {pr.state === "OPEN" && (
        <>
          <div className="border-t border-border/30" />
          <PrReviewActions
            cwd={cwd}
            prNumber={pr.number}
            onSubmitted={onRefresh}
          />
        </>
      )}

      <PrDeployments deployments={deployments} />

      {pr.state === "OPEN" && (
        <>
          <div className="border-t border-border/30" />
          <PrMergeControls pr={pr} cwd={cwd} onRefresh={onRefresh} />
        </>
      )}
    </div>
  );
}

// ── Main PrPanel ──

export function PrPanel({ workspace }: Props) {
  const cwd = workspace.worktree_path ?? workspace.cwd;
  const hasPr = workspace.pr_number != null;

  const [ghStatus, setGhStatus] = useState<GhStatus | null>(getCachedGhStatus());
  const [isGithubRepo, setIsGithubRepo] = useState<boolean | null>(
    getCachedRepoCheck(cwd) ?? null,
  );
  const [initialLoading, setInitialLoading] = useState(
    getCachedGhStatus() === null || getCachedRepoCheck(cwd) === undefined,
  );

  const [pr, setPr] = useState<PullRequestInfo | null>(null);
  const [checks, setChecks] = useState<CheckInfo[]>([]);
  const [reviews, setReviews] = useState<ReviewComment[]>([]);
  const [inlineComments, setInlineComments] = useState<InlineReviewComment[]>([]);
  const [deployments, setDeployments] = useState<DeploymentInfo[]>([]);
  const [detailLoading, setDetailLoading] = useState(false);
  const [fetchError, setFetchError] = useState<string | null>(null);
  const [defaultBranch, setDefaultBranch] = useState<string | null>(null);
  const [incomingRefreshKey, setIncomingRefreshKey] = useState(0);

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

  // Fetch full PR details when pr_number changes
  const fetchDetails = useCallback(async () => {
    if (!hasPr) {
      setPr(null);
      setChecks([]);
      setReviews([]);
      setInlineComments([]);
      setDeployments([]);
      return;
    }
    setDetailLoading(true);
    try {
      setFetchError(null);
      const prNum = workspace.pr_number!;
      const [prInfo, prChecks, prReviews, prInline, prDeploys] = await Promise.all([
        getBranchPullRequest(cwd),
        getPullRequestChecks(cwd).catch((e) => { console.warn("[pr-panel] checks:", e); return [] as CheckInfo[]; }),
        getPrReviewComments(cwd).catch((e) => { console.warn("[pr-panel] reviews:", e); return [] as ReviewComment[]; }),
        getPrInlineComments(cwd, prNum).catch((e) => { console.warn("[pr-panel] inline:", e); return [] as InlineReviewComment[]; }),
        getPrDeployments(cwd, prNum).catch((e) => { console.warn("[pr-panel] deploys:", e); return [] as DeploymentInfo[]; }),
      ]);
      setPr(prInfo);
      setChecks(prChecks);
      setReviews(prReviews);
      setInlineComments(prInline);
      setDeployments(prDeploys);
    } catch (err) {
      console.warn("[pr-panel] fetchDetails failed:", err);
      setFetchError(String(err));
      setPr(null);
      setChecks([]);
      setReviews([]);
      setInlineComments([]);
      setDeployments([]);
    } finally {
      setDetailLoading(false);
    }
  }, [cwd, hasPr, workspace.pr_number]);

  useEffect(() => {
    if (initialLoading) return;
    if (ghStatus?.status !== "Authenticated") return;
    if (!isGithubRepo) return;
    fetchDetails();
  }, [fetchDetails, initialLoading, ghStatus, isGithubRepo]);

  const handleRefresh = useCallback(async () => {
    setFetchError(null);
    if (!hasPr && isBaseBranch) {
      // On base branch — refresh incoming PR list
      setIncomingRefreshKey((k) => k + 1);
      return;
    }
    if (!hasPr) {
      // No PR known — re-discover via backend (gh pr view)
      setDetailLoading(true);
      try {
        // Bust caches so auth/repo re-check on next render cycle
        ghStatusCache = null;
        repoCheckCache.delete(cwd);
        await refreshWorkspacePr(workspace.workspace_id);
        // State update flows via app-state-changed → re-render.
        // If PR found, hasPr becomes true → useEffect triggers fetchDetails.
      } catch (err) {
        console.warn("[pr-panel] refresh_workspace_pr failed:", err);
        setFetchError(String(err));
      } finally {
        setDetailLoading(false);
      }
      return;
    }
    // PR exists — re-fetch full details
    fetchDetails();
  }, [hasPr, isBaseBranch, cwd, workspace.workspace_id, fetchDetails]);

  const handlePrCreated = (newPr: PullRequestInfo) => {
    setPr(newPr);
    const prNum = newPr.number;
    Promise.all([
      getPullRequestChecks(cwd).catch((e) => { console.warn("[pr-panel] checks:", e); return [] as CheckInfo[]; }),
      getPrReviewComments(cwd).catch((e) => { console.warn("[pr-panel] reviews:", e); return [] as ReviewComment[]; }),
      getPrInlineComments(cwd, prNum).catch((e) => { console.warn("[pr-panel] inline:", e); return [] as InlineReviewComment[]; }),
      getPrDeployments(cwd, prNum).catch((e) => { console.warn("[pr-panel] deploys:", e); return [] as DeploymentInfo[]; }),
    ]).then(([c, r, ic, d]) => {
      setChecks(c);
      setReviews(r);
      setInlineComments(ic);
      setDeployments(d);
    });
  };

  // ── Render ──

  if (initialLoading) {
    return <PrSkeleton />;
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
      <div className="flex items-center justify-end px-3 pt-2">
        <Button
          size="xs"
          variant="ghost"
          className="h-6 w-6 p-0"
          onClick={handleRefresh}
          title="Refresh"
          disabled={detailLoading}
        >
          <RefreshCw
            className={`h-3 w-3 ${detailLoading ? "animate-spin" : ""}`}
          />
        </Button>
      </div>
      {fetchError && (
        <div className="mx-3 mb-1 flex items-start gap-1.5 rounded bg-danger/10 px-2 py-1.5 text-xs text-danger">
          <AlertCircle className="h-3 w-3 mt-0.5 shrink-0" />
          <span className="break-words">{fetchError}</span>
        </div>
      )}
      {hasPr && pr ? (
        <PrView
          pr={pr}
          checks={checks}
          reviews={reviews}
          inlineComments={inlineComments}
          deployments={deployments}
          cwd={cwd}
          onRefresh={handleRefresh}
        />
      ) : hasPr && !pr ? (
        <PrSkeleton />
      ) : isBaseBranch ? (
        <IncomingPrsView
          cwd={cwd}
          baseBranch={defaultBranch!}
          projectRoot={workspace.project_root ?? workspace.cwd}
          refreshKey={incomingRefreshKey}
        />
      ) : (
        <NoPrView
          cwd={cwd}
          branchName={workspace.git_branch}
          onCreated={handlePrCreated}
        />
      )}
    </ScrollArea>
  );
}
