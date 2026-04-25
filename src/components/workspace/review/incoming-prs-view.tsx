import { useState, useEffect, useCallback } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  AlertCircle,
  Check,
  Circle,
  ExternalLink,
  GitBranch,
  GitPullRequest,
  X,
} from "lucide-react";
import { listIncomingPrs, activateWorkspace, createWorktreeWorkspace } from "@/tauri/commands";
import { useAppStore } from "@/stores/app-store";
import { openUrl } from "@tauri-apps/plugin-opener";
import { toast } from "@/lib/toast";
import type { IncomingPrItem } from "@/tauri/types";

function formatRelativeTime(dateStr: string): string {
  const date = new Date(dateStr);
  const now = new Date();
  const diffMs = now.getTime() - date.getTime();
  const diffMins = Math.floor(diffMs / 60000);
  if (diffMins < 1) return "just now";
  if (diffMins < 60) return `${diffMins}m ago`;
  const diffHours = Math.floor(diffMins / 60);
  if (diffHours < 24) return `${diffHours}h ago`;
  const diffDays = Math.floor(diffHours / 24);
  if (diffDays < 30) return `${diffDays}d ago`;
  return date.toLocaleDateString();
}

function ChecksIndicator({ status }: { status: string | null }) {
  if (!status) return null;
  switch (status) {
    case "success":
      return <Check className="h-3 w-3 text-success" />;
    case "failure":
      return <X className="h-3 w-3 text-danger" />;
    case "pending":
      return <Circle className="h-2.5 w-2.5 fill-warning text-warning" />;
    default:
      return null;
  }
}

const REVIEW_LABELS: Record<string, { label: string; cls: string }> = {
  APPROVED: { label: "Approved", cls: "text-success" },
  CHANGES_REQUESTED: { label: "Changes", cls: "text-warning" },
};

interface RowProps {
  pr: IncomingPrItem;
  projectRoot: string;
}

function IncomingPrRow({ pr, projectRoot }: RowProps) {
  const workspaces = useAppStore((s) => s.appState?.workspaces ?? []);
  const existingWs = workspaces.find(
    (w) => w.git_branch === pr.head_branch && w.project_root === projectRoot,
  );

  const handleView = (e: React.MouseEvent) => {
    e.stopPropagation();
    if (pr.url) openUrl(pr.url);
  };

  const handleCheckout = async (e: React.MouseEvent) => {
    e.stopPropagation();
    try {
      if (existingWs) {
        await activateWorkspace(existingWs.workspace_id);
      } else if (pr.head_branch) {
        await createWorktreeWorkspace(projectRoot, pr.head_branch, false, "single", null, null, null, pr.number);
      }
    } catch (err) {
      console.warn("[incoming-prs] checkout failed:", err);
      toast.error(String(err));
    }
  };

  const review = pr.review_decision ? REVIEW_LABELS[pr.review_decision] : null;

  return (
    <div
      className="group px-3 py-1.5 hover:bg-accent/30 rounded-sm transition-colors cursor-default min-w-0"
      onClick={() => { if (existingWs) activateWorkspace(existingWs.workspace_id); }}
    >
      <div className="flex items-center gap-1 min-w-0">
        <ChecksIndicator status={pr.checks_status} />
        <span className="text-muted-foreground font-mono text-xs shrink-0">#{pr.number}</span>
        <span className={`truncate flex-1 min-w-0 text-sm ${pr.is_draft ? "italic text-muted-foreground" : ""}`}>
          {pr.title}
        </span>
        {pr.is_draft && (
          <Badge variant="outline" className="h-3.5 px-1 text-[9px] leading-none shrink-0">
            Draft
          </Badge>
        )}
        {review && (
          <span className={`text-[10px] shrink-0 ${review.cls}`}>{review.label}</span>
        )}
      </div>

      <div className="flex items-center gap-1 mt-0.5 min-w-0">
        <span className="text-[11px] text-muted-foreground/60 shrink-0">{pr.author}</span>
        {pr.head_branch && (
          <span className="text-[11px] text-muted-foreground/60 font-mono truncate min-w-0">{pr.head_branch}</span>
        )}

        <span className="flex items-center gap-1 shrink-0 group-hover:hidden ml-auto">
          {pr.additions != null && pr.additions > 0 && (
            <span className="text-[10px] font-mono text-success">+{pr.additions}</span>
          )}
          {pr.deletions != null && pr.deletions > 0 && (
            <span className="text-[10px] font-mono text-danger">&minus;{pr.deletions}</span>
          )}
          {pr.updated_at && (
            <span className="text-[10px] text-muted-foreground">{formatRelativeTime(pr.updated_at)}</span>
          )}
        </span>

        <span className="hidden group-hover:flex items-center gap-1 shrink-0 ml-auto">
          <Button
            size="xs"
            variant="ghost"
            className="h-5 px-1.5 text-[10px]"
            onClick={handleView}
            title="View on GitHub"
          >
            <ExternalLink className="h-2.5 w-2.5 mr-0.5" />
            View
          </Button>
          {pr.head_branch && (
            <Button
              size="xs"
              variant="ghost"
              className="h-5 px-1.5 text-[10px]"
              onClick={handleCheckout}
              title={existingWs ? "Switch to workspace" : "Checkout in new worktree"}
            >
              <GitBranch className="h-2.5 w-2.5 mr-0.5" />
              {existingWs ? "Switch" : "Checkout"}
            </Button>
          )}
        </span>
      </div>
    </div>
  );
}

interface Props {
  cwd: string;
  baseBranch: string;
  projectRoot: string;
  refreshKey: number;
}

export function IncomingPrsView({ cwd, baseBranch, projectRoot, refreshKey }: Props) {
  const [prs, setPrs] = useState<IncomingPrItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const fetchPrs = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const result = await listIncomingPrs(cwd, baseBranch);
      setPrs(result);
    } catch (err) {
      console.warn("[incoming-prs] fetch failed:", err);
      setError(String(err));
      setPrs([]);
    } finally {
      setLoading(false);
    }
  }, [cwd, baseBranch]);

  useEffect(() => {
    fetchPrs();
  }, [fetchPrs, refreshKey]);

  return (
    <div className="px-1.5 pb-3">
      {/* Header */}
      <div className="flex items-center gap-2 px-1.5 pb-2 min-w-0">
        <span className="text-xs font-medium text-foreground truncate">Pull Requests</span>
        {!loading && prs.length > 0 && (
          <Badge variant="secondary" className="h-4 px-1.5 text-[10px] leading-none shrink-0">
            {prs.length}
          </Badge>
        )}
      </div>

      {/* Error */}
      {error && (
        <div className="mx-1.5 mb-2 flex items-start gap-1.5 rounded bg-danger/10 px-2 py-1.5 text-xs text-danger">
          <AlertCircle className="h-3 w-3 mt-0.5 shrink-0" />
          <span className="break-words">{error}</span>
        </div>
      )}

      {/* Loading */}
      {loading && (
        <div className="flex flex-col gap-2 px-3 py-2">
          {[1, 2, 3].map((i) => (
            <div key={i} className="animate-pulse flex gap-2">
              <div className="h-3 w-6 bg-muted rounded" />
              <div className="h-3 flex-1 bg-muted rounded" />
              <div className="h-3 w-12 bg-muted rounded" />
            </div>
          ))}
        </div>
      )}

      {/* Empty state */}
      {!loading && !error && prs.length === 0 && (
        <div className="flex flex-col items-center justify-center py-8 text-muted-foreground">
          <GitPullRequest className="h-8 w-8 opacity-30 mb-2" />
          <p className="text-xs">No open pull requests</p>
        </div>
      )}

      {/* PR list */}
      {!loading && prs.length > 0 && (
        <div className="flex flex-col gap-0.5">
          {prs.map((pr) => (
            <IncomingPrRow key={pr.number} pr={pr} projectRoot={projectRoot} />
          ))}
          {prs.length >= 50 && (
            <button
              className="text-[10px] text-muted-foreground hover:text-foreground px-3 py-1 transition-colors"
              onClick={() => {
                const repoUrl = prs[0]?.url?.replace(/\/pull\/\d+$/, "/pulls");
                if (repoUrl) openUrl(repoUrl);
              }}
            >
              View all on GitHub &rarr;
            </button>
          )}
        </div>
      )}
    </div>
  );
}
