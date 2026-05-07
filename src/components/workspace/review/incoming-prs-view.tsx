import { useState, useEffect, useCallback, useMemo, memo } from "react";
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
import type { IncomingPrItem, WorkspaceSnapshot } from "@/tauri/types";

// ── Module-level cache ──
//
// On a repo with thousands of PRs the `gh pr list` shell-out is the
// expensive part — even with the new 15s backend timeout we don't want
// to re-run it every time the user toggles into the Review tab. A
// 30-second TTL keyed by (cwd, baseBranch) makes re-mounts feel
// instant while still picking up new PRs on a normal browse cadence.
// `refreshKey` (bumped by the parent on commit/push events) bypasses
// the cache by forcing a fresh fetch.
const INCOMING_CACHE_TTL_MS = 30_000;
type CacheKey = string;
interface CacheEntry { value: IncomingPrItem[]; ts: number; }
const incomingCache = new Map<CacheKey, CacheEntry>();
const cacheKey = (cwd: string, base: string): CacheKey => `${cwd}\0${base}`;

/** Reset the module-level cache. For tests only — mirrors the
 * `_resetCaches` hook used by `review-panel.tsx`. */
export function _resetIncomingPrsCache(): void {
  incomingCache.clear();
}

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
  // Resolved at the parent so the row doesn't subscribe to the entire
  // workspaces array. Without this, every store update (background
  // git/PR poll, terminal output, etc.) re-runs `workspaces.find` for
  // each of the 50 rows — a real cost when the user has many open
  // workspaces. `existingWs` is null when no checked-out worktree
  // matches this PR's head branch.
  existingWs: WorkspaceSnapshot | null;
}

function IncomingPrRowImpl({ pr, projectRoot, existingWs }: RowProps) {
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

// Memoize so a re-render of the parent (e.g. after a polling fetch
// resolves with the same data) doesn't walk all 50 rows. With the
// `existingWs` lookup hoisted to the parent, prop equality is now
// stable across store updates that don't actually change this row.
const IncomingPrRow = memo(IncomingPrRowImpl);

interface Props {
  cwd: string;
  baseBranch: string;
  projectRoot: string;
  refreshKey: number;
}

export function IncomingPrsView({ cwd, baseBranch, projectRoot, refreshKey }: Props) {
  // Seed from the cache so a re-mount within the TTL paints instantly
  // instead of flashing the skeleton while the fetch is in flight.
  const cached = incomingCache.get(cacheKey(cwd, baseBranch));
  const cacheFresh = cached != null && Date.now() - cached.ts < INCOMING_CACHE_TTL_MS;
  const [prs, setPrs] = useState<IncomingPrItem[]>(cacheFresh ? cached!.value : []);
  const [loading, setLoading] = useState(!cacheFresh);
  const [error, setError] = useState<string | null>(null);

  // Resolve PR → existing-workspace once at the parent. Subscribing
  // here (instead of inside each row) collapses the per-row store
  // dependency to a single subscription and replaces 50 linear scans
  // with one Map lookup per row.
  const workspaces = useAppStore((s) => s.appState?.workspaces);
  const wsByBranch = useMemo(() => {
    const m = new Map<string, WorkspaceSnapshot>();
    if (!workspaces) return m;
    for (const w of workspaces) {
      if (w.project_root === projectRoot && w.git_branch) {
        m.set(w.git_branch, w);
      }
    }
    return m;
  }, [workspaces, projectRoot]);

  const fetchPrs = useCallback(async (force: boolean) => {
    if (!force) {
      const hit = incomingCache.get(cacheKey(cwd, baseBranch));
      if (hit && Date.now() - hit.ts < INCOMING_CACHE_TTL_MS) {
        setPrs(hit.value);
        setLoading(false);
        setError(null);
        return;
      }
    }
    setLoading(true);
    setError(null);
    try {
      const result = await listIncomingPrs(cwd, baseBranch);
      setPrs(result);
      incomingCache.set(cacheKey(cwd, baseBranch), { value: result, ts: Date.now() });
    } catch (err) {
      console.warn("[incoming-prs] fetch failed:", err);
      setError(String(err));
      setPrs([]);
    } finally {
      setLoading(false);
    }
  }, [cwd, baseBranch]);

  useEffect(() => {
    // refreshKey > 0 is an explicit invalidation from the parent
    // (commit/push/etc.) — bypass the cache in that case.
    fetchPrs(refreshKey > 0);
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
            <IncomingPrRow
              key={pr.number}
              pr={pr}
              projectRoot={projectRoot}
              existingWs={(pr.head_branch && wsByBranch.get(pr.head_branch)) || null}
            />
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
