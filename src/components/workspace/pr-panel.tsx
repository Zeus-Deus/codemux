import { useState, useEffect, useCallback, useRef } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Badge } from "@/components/ui/badge";
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
} from "lucide-react";
import {
  checkGhStatus,
  checkGithubRepo,
  getBranchPullRequest,
  createPullRequest,
  mergePullRequest,
  getPullRequestChecks,
  listBranches,
} from "@/tauri/commands";
import type {
  WorkspaceSnapshot,
  GhStatus,
  PullRequestInfo,
  CheckInfo,
} from "@/tauri/types";

interface Props {
  workspace: WorkspaceSnapshot;
}

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

function CheckIcon({ status }: { status: string }) {
  if (status === "pass" || status === "success") return <CheckCircle2 className="h-3 w-3 text-success" />;
  if (status === "fail" || status === "failure") return <XCircle className="h-3 w-3 text-danger" />;
  return <Clock className="h-3 w-3 text-warning" />;
}

function StatusMessage({ icon, message }: { icon: React.ReactNode; message: string }) {
  return (
    <div className="flex flex-col items-center justify-center py-8 text-muted-foreground gap-2">
      {icon}
      <p className="text-xs text-center px-4">{message}</p>
    </div>
  );
}

function CreatePrForm({ cwd, onCreated }: { cwd: string; onCreated: () => void }) {
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  const [baseBranch, setBaseBranch] = useState("main");
  const [branches, setBranches] = useState<string[]>([]);
  const [isDraft, setIsDraft] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);

  useEffect(() => {
    listBranches(cwd, false)
      .then((b) => {
        setBranches(b);
        if (b.includes("main")) setBaseBranch("main");
        else if (b.includes("master")) setBaseBranch("master");
        else if (b.length > 0) setBaseBranch(b[0]);
      })
      .catch(console.error);
  }, [cwd]);

  const handleCreate = async () => {
    if (!title.trim() || creating) return;
    setCreating(true);
    setError(null);
    try {
      await createPullRequest(cwd, title.trim(), body.trim(), baseBranch, isDraft);
      onCreated();
    } catch (err) {
      setError(String(err));
    } finally {
      setCreating(false);
    }
  };

  return (
    <div className="space-y-2 p-3">
      <p className="text-xs font-medium text-muted-foreground">Create Pull Request</p>
      <Input
        placeholder="PR title"
        value={title}
        onChange={(e) => setTitle(e.target.value)}
        onKeyDown={(e) => e.key === "Enter" && handleCreate()}
        className="h-7 text-xs"
      />
      <textarea
        placeholder="Description (optional)"
        value={body}
        onChange={(e) => setBody(e.target.value)}
        className="w-full rounded-md border border-input bg-transparent px-2 py-1 text-xs resize-none h-16 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
      />
      <div className="flex items-center gap-2">
        <div className="flex-1">
          <Select value={baseBranch} onValueChange={setBaseBranch}>
            <SelectTrigger className="h-7 text-xs">
              <SelectValue placeholder="Base branch" />
            </SelectTrigger>
            <SelectContent>
              {branches.map((b) => (
                <SelectItem key={b} value={b} className="text-xs">
                  {b}
                </SelectItem>
              ))}
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
      {error && (
        <p className="text-xs text-danger break-words">{error}</p>
      )}
    </div>
  );
}

function PrView({
  pr,
  checks,
  cwd,
  onRefresh,
}: {
  pr: PullRequestInfo;
  checks: CheckInfo[];
  cwd: string;
  onRefresh: () => void;
}) {
  const [mergeMethod, setMergeMethod] = useState("squash");
  const [confirmMerge, setConfirmMerge] = useState(false);
  const [merging, setMerging] = useState(false);
  const [error, setError] = useState<string | null>(null);

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

  const stateLabel = pr.is_draft ? "Draft" : (pr.state ?? "OPEN");
  const stateColorCls = STATE_COLORS[stateLabel.toUpperCase()] ?? STATE_COLORS.OPEN;
  const review = pr.review_decision ? REVIEW_LABELS[pr.review_decision] : null;

  return (
    <div className="space-y-3 p-3">
      {/* Header */}
      <div className="space-y-1">
        <div className="flex items-center gap-1.5">
          <Badge className={`text-[10px] px-1.5 py-0 ${stateColorCls}`}>
            {stateLabel}
          </Badge>
          <span className="text-[10px] text-muted-foreground">#{pr.number}</span>
        </div>
        <p className="text-xs font-medium text-foreground">{pr.title}</p>
        <div className="flex items-center gap-1 text-[10px] text-muted-foreground">
          {pr.base_branch && pr.head_branch && (
            <span>{pr.base_branch} ← {pr.head_branch}</span>
          )}
        </div>
        <div className="flex items-center gap-2 text-[10px]">
          {pr.additions != null && (
            <span className="text-success">+{pr.additions}</span>
          )}
          {pr.deletions != null && (
            <span className="text-danger">-{pr.deletions}</span>
          )}
          {review && (
            <span className={review.cls}>{review.label}</span>
          )}
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
              <CheckIcon status={check.status} />
              <span className="text-xs text-foreground truncate">{check.name}</span>
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
                <SelectItem value="squash" className="text-xs">Squash and merge</SelectItem>
                <SelectItem value="merge" className="text-xs">Create merge commit</SelectItem>
                <SelectItem value="rebase" className="text-xs">Rebase and merge</SelectItem>
              </SelectContent>
            </Select>
            <Button
              size="xs"
              className="text-xs h-7"
              variant={confirmMerge ? "destructive" : "default"}
              disabled={merging || pr.mergeable === "CONFLICTING"}
              onClick={handleMerge}
            >
              {merging ? "Merging..." : confirmMerge ? "Confirm" : "Merge"}
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

      {/* External link */}
      {pr.url && (
        <a
          href={pr.url}
          target="_blank"
          rel="noopener noreferrer"
          className="inline-flex items-center gap-1 text-xs text-primary hover:underline"
          onClick={(e) => {
            e.preventDefault();
            // Use Tauri shell open if available, otherwise fallback
            window.open(pr.url, "_blank");
          }}
        >
          <ExternalLink className="h-3 w-3" />
          View on GitHub
        </a>
      )}
    </div>
  );
}

export function PrPanel({ workspace }: Props) {
  const cwd = workspace.worktree_path ?? workspace.cwd;
  const [ghStatus, setGhStatus] = useState<GhStatus | null>(null);
  const [isGithubRepo, setIsGithubRepo] = useState<boolean | null>(null);
  const [pr, setPr] = useState<PullRequestInfo | null>(null);
  const [checks, setChecks] = useState<CheckInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [noPr, setNoPr] = useState(false);
  const refreshRef = useRef<ReturnType<typeof setInterval> | null>(null);

  const loadPr = useCallback(async () => {
    try {
      const prInfo = await getBranchPullRequest(cwd);
      setPr(prInfo);
      setNoPr(prInfo === null);
      if (prInfo) {
        const prChecks = await getPullRequestChecks(cwd).catch(() => []);
        setChecks(prChecks);
      }
    } catch {
      setPr(null);
      setNoPr(true);
    }
  }, [cwd]);

  const init = useCallback(async () => {
    setLoading(true);
    try {
      const status = await checkGhStatus();
      setGhStatus(status);
      if (status.status !== "Authenticated") {
        setLoading(false);
        return;
      }
      const isRepo = await checkGithubRepo(cwd);
      setIsGithubRepo(isRepo);
      if (!isRepo) {
        setLoading(false);
        return;
      }
      await loadPr();
    } catch (err) {
      console.error("PR init failed:", err);
    } finally {
      setLoading(false);
    }
  }, [cwd, loadPr]);

  useEffect(() => {
    init();
    // Poll PR every 10s if PR exists
    refreshRef.current = setInterval(() => {
      if (!noPr) loadPr();
    }, 10000);
    return () => {
      if (refreshRef.current) clearInterval(refreshRef.current);
    };
  }, [init, loadPr, noPr]);

  const handleRefresh = () => {
    init();
  };

  if (loading) {
    return (
      <div className="flex items-center justify-center py-8">
        <RefreshCw className="h-4 w-4 animate-spin text-muted-foreground" />
      </div>
    );
  }

  // Status checks
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
        >
          <RefreshCw className="h-3 w-3" />
        </Button>
      </div>
      {pr ? (
        <PrView pr={pr} checks={checks} cwd={cwd} onRefresh={handleRefresh} />
      ) : (
        <CreatePrForm cwd={cwd} onCreated={handleRefresh} />
      )}
    </ScrollArea>
  );
}
