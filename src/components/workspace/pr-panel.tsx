import { useState, useEffect, useCallback } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Badge } from "@/components/ui/badge";
import { Skeleton } from "@/components/ui/skeleton";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  ExternalLink,
  GitPullRequest,
  CheckCircle2,
  XCircle,
  Clock,
  AlertCircle,
  RefreshCw,
  Copy,
  MessageSquare,
  ShieldCheck,
  ShieldAlert,
  ChevronLeft,
} from "lucide-react";
import {
  checkGhStatus,
  checkGithubRepo,
  getBranchPullRequest,
  createPullRequest,
  mergePullRequest,
  getPullRequestChecks,
  getPrReviewComments,
  listBranches,
} from "@/tauri/commands";
import type {
  WorkspaceSnapshot,
  GhStatus,
  PullRequestInfo,
  CheckInfo,
  ReviewComment,
} from "@/tauri/types";

interface Props {
  workspace: WorkspaceSnapshot;
}

// ── Module-level caches ──
let ghStatusCache: GhStatus | null = null;
const repoCheckCache = new Map<string, boolean>();

// ── Constants ──

const STATE_COLORS: Record<string, string> = {
  OPEN: "bg-success/20 text-success",
  DRAFT: "bg-muted text-muted-foreground",
  MERGED: "bg-purple-500/20 text-purple-400",
  CLOSED: "bg-danger/20 text-danger",
};

const REVIEW_LABELS: Record<string, { label: string; cls: string }> = {
  APPROVED: { label: "Approved", cls: "text-success" },
  CHANGES_REQUESTED: { label: "Changes requested", cls: "text-warning" },
  REVIEW_REQUIRED: { label: "Review pending", cls: "text-muted-foreground" },
};

// ── Helpers ──

function branchToTitle(branch: string | null): string {
  if (!branch) return "";
  return branch
    .replace(/^(feature|fix|chore|docs|refactor|test)[/-]/, "")
    .replace(/[-_]/g, " ")
    .replace(/^\w/, (c) => c.toUpperCase());
}

// ── Small sub-components ──

function CheckIcon({ status }: { status: string }) {
  if (status === "pass" || status === "success")
    return <CheckCircle2 className="h-3 w-3 text-success" />;
  if (status === "fail" || status === "failure")
    return <XCircle className="h-3 w-3 text-danger" />;
  return <Clock className="h-3 w-3 text-warning" />;
}

function ReviewStateIcon({ state }: { state: string }) {
  if (state === "APPROVED")
    return <ShieldCheck className="h-3 w-3 text-success" />;
  if (state === "CHANGES_REQUESTED")
    return <ShieldAlert className="h-3 w-3 text-warning" />;
  return <MessageSquare className="h-3 w-3 text-muted-foreground" />;
}

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
      <Button size="xs" className="text-xs" onClick={() => setShowForm(true)}>
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
  cwd,
  onRefresh,
}: {
  pr: PullRequestInfo;
  checks: CheckInfo[];
  reviews: ReviewComment[];
  cwd: string;
  onRefresh: () => void;
}) {
  const [mergeMethod, setMergeMethod] = useState("squash");
  const [confirmMerge, setConfirmMerge] = useState(false);
  const [merging, setMerging] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  const handleMerge = async () => {
    if (!confirmMerge) {
      setConfirmMerge(true);
      setTimeout(() => setConfirmMerge(false), 5000);
      return;
    }
    setMerging(true);
    setError(null);
    try {
      await mergePullRequest(cwd, pr.number, mergeMethod);
      onRefresh();
    } catch (err) {
      setError(String(err));
    } finally {
      setMerging(false);
      setConfirmMerge(false);
    }
  };

  const handleCopyUrl = () => {
    navigator.clipboard.writeText(pr.url);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  const stateLabel = pr.is_draft ? "Draft" : (pr.state ?? "OPEN");
  const stateColorCls =
    STATE_COLORS[stateLabel.toUpperCase()] ?? STATE_COLORS.OPEN;
  const review = pr.review_decision ? REVIEW_LABELS[pr.review_decision] : null;

  return (
    <div className="space-y-3 p-3">
      {/* Header */}
      <div className="space-y-1">
        <div className="flex items-center gap-1.5">
          <Badge className={`text-[10px] px-1.5 py-0 ${stateColorCls}`}>
            {stateLabel}
          </Badge>
          <span className="text-[10px] text-muted-foreground">
            #{pr.number}
          </span>
        </div>
        <p className="text-xs font-medium text-foreground">{pr.title}</p>
        <div className="flex items-center gap-1 text-[10px] text-muted-foreground">
          {pr.base_branch && pr.head_branch && (
            <span>
              {pr.base_branch} ← {pr.head_branch}
            </span>
          )}
        </div>
        <div className="flex items-center gap-2 text-[10px]">
          {pr.additions != null && (
            <span className="text-success">+{pr.additions}</span>
          )}
          {pr.deletions != null && (
            <span className="text-danger">-{pr.deletions}</span>
          )}
          {review && <span className={review.cls}>{review.label}</span>}
        </div>
      </div>

      {/* Checks */}
      {checks.length > 0 && (
        <div className="space-y-1">
          <span className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
            Checks
          </span>
          {checks.map((check) => (
            <div key={check.name} className="flex items-center gap-1.5">
              <CheckIcon status={check.conclusion ?? check.status} />
              <span className="text-xs text-foreground truncate">
                {check.name}
              </span>
            </div>
          ))}
        </div>
      )}

      {/* Reviews */}
      {reviews.length > 0 && (
        <div className="space-y-1.5">
          <span className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
            Reviews
          </span>
          {reviews.map((r) => (
            <div key={r.id || `${r.author}-${r.created_at}`} className="space-y-0.5">
              <div className="flex items-center gap-1.5">
                <ReviewStateIcon state={r.state} />
                <span className="text-xs font-medium text-foreground">
                  {r.author}
                </span>
                {r.created_at && (
                  <span className="text-[10px] text-muted-foreground">
                    {new Date(r.created_at).toLocaleDateString()}
                  </span>
                )}
              </div>
              <p className="text-xs text-muted-foreground pl-[18px]">
                {r.body}
              </p>
            </div>
          ))}
        </div>
      )}

      {/* Merge controls */}
      {pr.state === "OPEN" && (
        <div className="space-y-1.5">
          <div className="flex gap-1">
            <Select value={mergeMethod} onValueChange={setMergeMethod}>
              <SelectTrigger className="h-7 text-xs flex-1">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="squash" className="text-xs">
                  Squash and merge
                </SelectItem>
                <SelectItem value="merge" className="text-xs">
                  Create merge commit
                </SelectItem>
                <SelectItem value="rebase" className="text-xs">
                  Rebase and merge
                </SelectItem>
              </SelectContent>
            </Select>
            <Button
              size="xs"
              className="text-xs h-7"
              variant={confirmMerge ? "destructive" : "default"}
              disabled={merging || pr.mergeable === "CONFLICTING"}
              onClick={handleMerge}
            >
              {merging
                ? "Merging..."
                : confirmMerge
                  ? "Confirm"
                  : "Merge"}
            </Button>
          </div>
          {pr.mergeable === "CONFLICTING" && (
            <p className="text-[10px] text-danger">Has merge conflicts</p>
          )}
          {error && (
            <p className="text-xs text-danger break-words">{error}</p>
          )}
        </div>
      )}

      {/* Actions */}
      <div className="flex items-center gap-1.5">
        <a
          href={pr.url}
          target="_blank"
          rel="noopener noreferrer"
          className="inline-flex items-center gap-1 text-xs text-primary hover:underline"
          onClick={(e) => {
            e.preventDefault();
            window.open(pr.url, "_blank");
          }}
        >
          <ExternalLink className="h-3 w-3" />
          View on GitHub
        </a>
        <Button
          size="xs"
          variant="ghost"
          className="text-xs h-6 px-1.5"
          onClick={handleCopyUrl}
        >
          <Copy className="h-3 w-3 mr-1" />
          {copied ? "Copied" : "Copy URL"}
        </Button>
      </div>
    </div>
  );
}

// ── Main PrPanel ──

export function PrPanel({ workspace }: Props) {
  const cwd = workspace.worktree_path ?? workspace.cwd;
  const hasPr = workspace.pr_number != null;

  const [ghStatus, setGhStatus] = useState<GhStatus | null>(ghStatusCache);
  const [isGithubRepo, setIsGithubRepo] = useState<boolean | null>(
    repoCheckCache.get(cwd) ?? null,
  );
  const [initialLoading, setInitialLoading] = useState(
    ghStatusCache === null || !repoCheckCache.has(cwd),
  );

  const [pr, setPr] = useState<PullRequestInfo | null>(null);
  const [checks, setChecks] = useState<CheckInfo[]>([]);
  const [reviews, setReviews] = useState<ReviewComment[]>([]);
  const [detailLoading, setDetailLoading] = useState(false);

  // Auth init — uses module-level cache
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        let status = ghStatusCache;
        if (!status) {
          status = await checkGhStatus();
          ghStatusCache = status;
        }
        if (cancelled) return;
        setGhStatus(status);

        if (status.status !== "Authenticated") {
          setInitialLoading(false);
          return;
        }

        let isRepo = repoCheckCache.get(cwd);
        if (isRepo === undefined) {
          isRepo = await checkGithubRepo(cwd);
          repoCheckCache.set(cwd, isRepo);
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
      return;
    }
    setDetailLoading(true);
    try {
      const [prInfo, prChecks, prReviews] = await Promise.all([
        getBranchPullRequest(cwd),
        getPullRequestChecks(cwd).catch(() => [] as CheckInfo[]),
        getPrReviewComments(cwd).catch(() => [] as ReviewComment[]),
      ]);
      setPr(prInfo);
      setChecks(prChecks);
      setReviews(prReviews);
    } catch {
      setPr(null);
      setChecks([]);
      setReviews([]);
    } finally {
      setDetailLoading(false);
    }
  }, [cwd, hasPr]);

  useEffect(() => {
    if (initialLoading) return;
    if (ghStatus?.status !== "Authenticated") return;
    if (!isGithubRepo) return;
    fetchDetails();
  }, [fetchDetails, initialLoading, ghStatus, isGithubRepo]);

  const handleRefresh = () => {
    fetchDetails();
  };

  const handlePrCreated = (newPr: PullRequestInfo) => {
    setPr(newPr);
    // Fetch checks/reviews for the newly created PR
    Promise.all([
      getPullRequestChecks(cwd).catch(() => [] as CheckInfo[]),
      getPrReviewComments(cwd).catch(() => [] as ReviewComment[]),
    ]).then(([c, r]) => {
      setChecks(c);
      setReviews(r);
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
    <ScrollArea className="h-full">
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
      {hasPr && pr ? (
        <PrView
          pr={pr}
          checks={checks}
          reviews={reviews}
          cwd={cwd}
          onRefresh={handleRefresh}
        />
      ) : hasPr && !pr ? (
        <PrSkeleton />
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
