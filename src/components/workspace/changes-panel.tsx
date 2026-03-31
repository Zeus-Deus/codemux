import { useState, useEffect, useCallback, useRef, useMemo } from "react";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
} from "@/components/ui/dropdown-menu";
import {
  Tooltip,
  TooltipTrigger,
  TooltipContent,
  TooltipProvider,
} from "@/components/ui/tooltip";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import {
  Plus,
  Minus,
  GitCommit,
  ArrowUp,
  ArrowDown,
  RefreshCw,
  Loader2,
  Check,
  Trash2,
  ChevronRight,
  Sparkles,
  AlertTriangle,
  GitMerge,
  XCircle,
  CheckCircle2,
  ChevronDown,
  ArrowUpToLine,
  GitBranch,
  Archive,
  ArchiveRestore,
  List,
  FolderTree,
  ArrowDownUp,
  Download,
} from "lucide-react";
import {
  getGitStatus,
  getGitDiff,
  getGitBranchInfo,
  gitStageFiles,
  gitUnstageFiles,
  gitCommitChanges,
  gitPushChanges,
  gitPullChanges,
  gitDiscardFile,
  gitLogEntries,
  getMergeState,
  mergeBranch,
  resolveConflictOurs,
  resolveConflictTheirs,
  markConflictResolved,
  abortMerge,
  continueMerge,
  mergeIntoBase,
  completeMergeIntoBase,
  abortMergeIntoBase,
  createTab,
  activateTab,
  checkClaudeAvailable,
  getBaseBranchDiff,
  getDefaultBranch,
  listBranches,
  gitFetchChanges,
  gitStashPush,
  gitStashPop,
  getCommitFiles,
} from "@/tauri/commands";
import {
  ContextMenu,
  ContextMenuTrigger,
  ContextMenuContent,
  ContextMenuItem,
} from "@/components/ui/context-menu";
import {
  VscDiffAdded,
  VscDiffModified,
  VscDiffRemoved,
  VscDiffRenamed,
  VscCopy,
} from "react-icons/vsc";
import { useDiffStore } from "@/stores/diff-store";
import { useAppStore } from "@/stores/app-store";
import { useAiCommitStore } from "@/stores/ai-commit-store";
import { useAiMergeStore } from "@/stores/ai-merge-store";
import type {
  WorkspaceSnapshot,
  GitFileStatus,
  GitBranchInfo,
  GitLogEntry,
  MergeState,
  CommitFileEntry,
} from "@/tauri/types";

interface Props {
  workspace: WorkspaceSnapshot;
}

const STATUS_ICON_CLASS = "w-3 h-3";

function StatusIcon({ status }: { status: string }) {
  switch (status) {
    case "added":
    case "untracked":
      return <VscDiffAdded className={STATUS_ICON_CLASS} />;
    case "modified":
      return <VscDiffModified className={STATUS_ICON_CLASS} />;
    case "deleted":
      return <VscDiffRemoved className={STATUS_ICON_CLASS} />;
    case "renamed":
      return <VscDiffRenamed className={STATUS_ICON_CLASS} />;
    case "copied":
      return <VscCopy className={STATUS_ICON_CLASS} />;
    case "conflicted":
      return <VscDiffModified className={STATUS_ICON_CLASS} />;
    default:
      return null;
  }
}

const STATUS_COLOR: Record<string, string> = {
  added: "text-green-400",
  modified: "text-yellow-400",
  deleted: "text-red-400",
  renamed: "text-blue-400",
  untracked: "text-green-400",
  copied: "text-purple-400",
  conflicted: "text-red-400",
};

const CONFLICT_TYPE_LABEL: Record<string, string> = {
  both_modified: "Both Modified",
  both_added: "Both Added",
  both_deleted: "Both Deleted",
  deleted_by_them: "Deleted by Them",
  deleted_by_us: "Deleted by Us",
  added_by_them: "Added by Them",
  added_by_us: "Added by Us",
};

// ── Helpers ──

function groupByDirectory(files: GitFileStatus[]): Array<{ dir: string; files: GitFileStatus[] }> {
  const map = new Map<string, GitFileStatus[]>();
  for (const file of files) {
    const lastSlash = file.path.lastIndexOf("/");
    const dir = lastSlash >= 0 ? file.path.substring(0, lastSlash) : "";
    const arr = map.get(dir) || [];
    arr.push(file);
    map.set(dir, arr);
  }
  // Sort: root ("") first, then alphabetical
  return Array.from(map.entries())
    .sort(([a], [b]) => {
      if (a === "") return -1;
      if (b === "") return 1;
      return a.localeCompare(b);
    })
    .map(([dir, files]) => ({ dir, files }));
}

function fileName(path: string): string {
  const idx = path.lastIndexOf("/");
  return idx >= 0 ? path.substring(idx + 1) : path;
}

// ── Section Header ──

function SectionHeader({
  title,
  count,
  expanded,
  onToggle,
  actions,
  variant = "default",
}: {
  title: string;
  count: number;
  expanded: boolean;
  onToggle: () => void;
  actions?: React.ReactNode;
  variant?: "default" | "danger";
}) {
  const textColor = variant === "danger" ? "text-danger" : "text-muted-foreground";
  return (
    <div className="flex items-center">
      <button
        type="button"
        className="flex flex-1 items-center gap-1.5 text-left min-w-0 hover:bg-accent/30 rounded-sm px-2 py-1.5 cursor-pointer transition-colors"
        onClick={onToggle}
      >
        <ChevronRight
          className={`h-3 w-3 shrink-0 ${textColor} transition-transform ${expanded ? "rotate-90" : ""}`}
        />
        <span className={`text-xs font-medium uppercase tracking-wide ${textColor}`}>
          {title}
        </span>
        <span className={`text-xs tabular-nums ${textColor}`}>{count}</span>
      </button>
      {actions && (
        <div className="flex items-center gap-0.5 shrink-0 pr-1.5">{actions}</div>
      )}
    </div>
  );
}

// ── Commit Row ──

const COMMIT_FILE_COLOR: Record<string, string> = {
  added: "text-green-400",
  modified: "text-yellow-400",
  deleted: "text-red-400",
  renamed: "text-blue-400",
  copied: "text-purple-400",
};

function formatRelativeTime(timeAgo: string): string {
  // The backend returns e.g. "3 hours ago", "5 minutes ago", "2 days ago"
  // Convert to compact format: "3h ago", "5m ago", "2d ago"
  const match = timeAgo.match(/^(\d+)\s+(second|minute|hour|day|week|month|year)s?\s+ago$/);
  if (!match) return timeAgo;
  const [, n, unit] = match;
  const abbrev: Record<string, string> = {
    second: "s", minute: "m", hour: "h", day: "d", week: "w", month: "mo", year: "y",
  };
  return `${n}${abbrev[unit] ?? unit} ago`;
}

function CommitRow({
  commit,
  cwd,
  expanded,
  onToggle,
}: {
  commit: GitLogEntry;
  cwd: string;
  expanded: boolean;
  onToggle: () => void;
}) {
  const [files, setFiles] = useState<CommitFileEntry[] | null>(null);

  useEffect(() => {
    if (expanded && files === null) {
      getCommitFiles(cwd, commit.hash)
        .then(setFiles)
        .catch(() => setFiles([]));
    }
  }, [expanded, cwd, commit.hash, files]);

  const handleCopyHash = () => {
    navigator.clipboard.writeText(commit.hash).catch(console.error);
  };

  return (
    <div>
      <ContextMenu>
        <Tooltip>
          <TooltipTrigger asChild>
            <ContextMenuTrigger asChild>
              <button
                type="button"
                className="flex w-full items-center gap-1.5 rounded-sm px-1.5 py-1 text-left hover:bg-accent/50 cursor-pointer transition-colors"
                onClick={onToggle}
              >
                <ChevronRight
                  className={`h-2.5 w-2.5 shrink-0 text-muted-foreground transition-transform ${expanded ? "rotate-90" : ""}`}
                />
                <span className="shrink-0 text-[10px] font-mono text-muted-foreground">
                  {commit.short_hash}
                </span>
                <span className="flex-1 truncate text-xs text-foreground">{commit.message}</span>
                <span className="shrink-0 text-[10px] text-muted-foreground">
                  {formatRelativeTime(commit.time_ago)}
                </span>
              </button>
            </ContextMenuTrigger>
          </TooltipTrigger>
          <TooltipContent side="right" className="text-xs max-w-64">
            {commit.message}
          </TooltipContent>
        </Tooltip>
        <ContextMenuContent className="w-52">
          <ContextMenuItem onClick={handleCopyHash} className="text-xs">
            <Check className="h-3.5 w-3.5 mr-2" />
            Copy Commit Hash
          </ContextMenuItem>
        </ContextMenuContent>
      </ContextMenu>
      {expanded && files && files.length > 0 && (
        <div className="ml-4 pl-1.5 border-l border-border mt-0.5 mb-0.5">
          {files.map((f) => (
            <div
              key={f.path}
              className="flex items-center gap-1 px-1 py-0.5 text-xs"
            >
              <span className={`shrink-0 w-4 flex items-center justify-center ${COMMIT_FILE_COLOR[f.status] ?? "text-muted-foreground"}`}>
                <StatusIcon status={f.status} />
              </span>
              <span className="truncate text-xs text-foreground">{fileName(f.path)}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

// ── Diff View ──

function DiffView({ diff }: { diff: string }) {
  if (!diff) {
    return (
      <pre className="mx-1 mb-1 rounded-sm bg-muted/30 p-1.5 text-[10px] leading-normal font-mono text-muted-foreground">
        (empty diff)
      </pre>
    );
  }
  const lines = diff.split("\n");
  return (
    <div className="mx-1 mb-1 max-h-48 overflow-auto rounded-sm bg-muted/30 text-[10px] leading-normal font-mono">
      {lines.map((line, i) => {
        let cls = "px-1.5 whitespace-pre-wrap break-all";
        if (line.startsWith("+") && !line.startsWith("+++")) {
          cls += " text-success bg-success/10";
        } else if (line.startsWith("-") && !line.startsWith("---")) {
          cls += " text-danger bg-danger/10";
        } else if (line.startsWith("@@")) {
          cls += " text-muted-foreground/60 bg-muted/30 text-[9px]";
        } else {
          cls += " text-muted-foreground";
        }
        return (
          <div key={i} className={cls}>
            {line || " "}
          </div>
        );
      })}
    </div>
  );
}

// ── File Row ──

function FileRow({
  file,
  staged,
  cwd,
  expanded,
  onToggleExpand,
  onRefresh,
  onOpenDiff,
  activeDiffFile,
}: {
  file: GitFileStatus;
  staged: boolean;
  cwd: string;
  expanded: boolean;
  onToggleExpand: () => void;
  onRefresh: () => void;
  onOpenDiff?: (filePath: string, staged: boolean) => void;
  activeDiffFile?: string | null;
}) {
  const [diff, setDiff] = useState<string | null>(null);
  const [confirmDiscard, setConfirmDiscard] = useState(false);

  useEffect(() => {
    if (expanded) {
      getGitDiff(cwd, file.path, staged)
        .then(setDiff)
        .catch(() => setDiff("Failed to load diff"));
    } else {
      setDiff(null);
    }
  }, [expanded, cwd, file.path, staged]);

  const handleStageToggle = async (e: React.MouseEvent) => {
    e.stopPropagation();
    try {
      if (staged) {
        await gitUnstageFiles(cwd, [file.path]);
      } else {
        await gitStageFiles(cwd, [file.path]);
      }
      onRefresh();
    } catch (err) {
      console.error("Stage/unstage failed:", err);
    }
  };

  const handleDiscard = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (!confirmDiscard) {
      setConfirmDiscard(true);
      setTimeout(() => setConfirmDiscard(false), 3000);
      return;
    }
    try {
      await gitDiscardFile(cwd, file.path);
      onRefresh();
    } catch (err) {
      console.error("Discard failed:", err);
    }
    setConfirmDiscard(false);
  };

  const name = fileName(file.path);

  return (
    <div>
      <Tooltip>
        <TooltipTrigger asChild>
          <div
            role="button"
            tabIndex={0}
            className={`group flex w-full items-stretch gap-1 rounded-sm text-left hover:bg-accent/50 transition-colors cursor-default px-1.5 ${activeDiffFile === file.path ? "bg-accent/30" : ""}`}
            onClick={(e) => {
              if (e.altKey || !onOpenDiff) {
                onToggleExpand();
              } else {
                onOpenDiff(file.path, staged);
              }
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                onToggleExpand();
              }
            }}
          >
            <div className="flex items-center gap-1.5 flex-1 min-w-0 py-1">
              <span className={`shrink-0 w-4 flex items-center justify-center ${STATUS_COLOR[file.status] ?? "text-muted-foreground"}`}>
                <StatusIcon status={file.status} />
              </span>
              <span className="flex-1 min-w-0 flex items-center gap-1">
                <span className="truncate text-xs text-foreground">{name}</span>
                {(file.additions > 0 || file.deletions > 0) && (
                  <span className="flex items-center gap-0.5 shrink-0 text-[10px] tabular-nums opacity-60">
                    {file.additions > 0 && (
                      <span className="text-success">+{file.additions}</span>
                    )}
                    {file.deletions > 0 && (
                      <span className="text-danger">-{file.deletions}</span>
                    )}
                  </span>
                )}
              </span>
            </div>
            <div className="flex items-center shrink-0 opacity-0 group-hover:opacity-100 transition-opacity">
              {!staged && (
                <Button
                  variant="ghost"
                  size="icon-xs"
                  className={confirmDiscard ? "text-danger hover:text-danger" : "text-muted-foreground"}
                  onClick={handleDiscard}
                  title={confirmDiscard ? "Click again to discard" : "Discard changes"}
                >
                  <Trash2 className="h-3 w-3" />
                </Button>
              )}
              <Button
                variant="ghost"
                size="icon-xs"
                className="text-muted-foreground"
                onClick={handleStageToggle}
                title={staged ? "Unstage" : "Stage"}
              >
                {staged ? <Minus className="h-3 w-3" /> : <Plus className="h-3 w-3" />}
              </Button>
            </div>
          </div>
        </TooltipTrigger>
        <TooltipContent side="left" className="text-xs">
          {file.path}
        </TooltipContent>
      </Tooltip>
      {expanded && diff !== null && <DiffView diff={diff} />}
    </div>
  );
}

// ── Conflict File Row ──

function ConflictFileRow({
  file,
  cwd,
  onRefresh,
  onOpenDiff,
}: {
  file: GitFileStatus;
  cwd: string;
  onRefresh: () => void;
  onOpenDiff?: (filePath: string, staged: boolean) => void;
}) {
  const [busy, setBusy] = useState<string | null>(null);

  const handleResolve = async (action: "ours" | "theirs" | "resolved") => {
    setBusy(action);
    try {
      if (action === "ours") await resolveConflictOurs(cwd, file.path);
      else if (action === "theirs") await resolveConflictTheirs(cwd, file.path);
      else await markConflictResolved(cwd, file.path);
      onRefresh();
    } catch (err) {
      console.error("Resolve failed:", err);
    } finally {
      setBusy(null);
    }
  };

  const name = fileName(file.path);

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <div
          role="button"
          tabIndex={0}
          className="group flex w-full items-stretch gap-1 rounded-sm px-1.5 text-left hover:bg-danger/10 transition-colors cursor-default"
          onClick={() => onOpenDiff?.(file.path, false)}
        >
          <div className="flex items-center gap-1.5 flex-1 min-w-0 py-1">
            <span className="shrink-0 w-4 flex items-center justify-center text-red-400">
              <StatusIcon status="conflicted" />
            </span>
            <span className="flex-1 min-w-0 flex items-center gap-1">
              <span className="truncate text-xs text-foreground">{name}</span>
              {file.conflict_type && (
                <span className="shrink-0 text-[9px] text-danger/70 bg-danger/10 px-1 rounded">
                  {CONFLICT_TYPE_LABEL[file.conflict_type] ?? file.conflict_type}
                </span>
              )}
            </span>
          </div>
          <div className="flex items-center shrink-0 gap-0.5 opacity-0 group-hover:opacity-100 transition-opacity">
            <Button
              variant="ghost"
              size="icon-xs"
              className="text-foreground"
              onClick={(e) => { e.stopPropagation(); handleResolve("ours"); }}
              title="Accept ours"
              disabled={busy !== null}
            >
              {busy === "ours" ? <Loader2 className="h-3 w-3 animate-spin" /> : <span className="text-[9px] font-bold">O</span>}
            </Button>
            <Button
              variant="ghost"
              size="icon-xs"
              className="text-purple-400"
              onClick={(e) => { e.stopPropagation(); handleResolve("theirs"); }}
              title="Accept theirs"
              disabled={busy !== null}
            >
              {busy === "theirs" ? <Loader2 className="h-3 w-3 animate-spin" /> : <span className="text-[9px] font-bold">T</span>}
            </Button>
            <Button
              variant="ghost"
              size="icon-xs"
              className="text-success"
              onClick={(e) => { e.stopPropagation(); handleResolve("resolved"); }}
              title="Mark as resolved"
              disabled={busy !== null}
            >
              {busy === "resolved" ? <Loader2 className="h-3 w-3 animate-spin" /> : <CheckCircle2 className="h-3 w-3" />}
            </Button>
          </div>
        </div>
      </TooltipTrigger>
      <TooltipContent side="left" className="text-xs">
        {file.path}
      </TooltipContent>
    </Tooltip>
  );
}

// ── Conflicts Section ──

function ConflictsSection({
  files,
  cwd,
  onRefresh,
  onOpenDiff,
  resolverEnabled,
  resolverStatus,
  resolverError,
  onStartResolve,
  onApproveResolve,
  onRejectResolve,
}: {
  files: GitFileStatus[];
  cwd: string;
  onRefresh: () => void;
  onOpenDiff?: (filePath: string, staged: boolean) => void;
  resolverEnabled: boolean;
  resolverStatus: string;
  resolverError: string | null;
  onStartResolve: () => void;
  onApproveResolve: () => void;
  onRejectResolve: () => void;
}) {
  // Show resolver progress UI when active
  if (resolverStatus !== "idle") {
    return (
      <div className="py-1 space-y-1.5">
        <div className="px-1.5">
          <div className="flex items-center gap-1.5 py-0.5 pl-0.5">
            <span className="text-xs font-medium uppercase tracking-wide text-danger">
              Conflicts
            </span>
          </div>
        </div>

        {resolverStatus === "creating_branch" && (
          <div className="flex items-center gap-1.5 px-1 py-1">
            <Loader2 className="h-3 w-3 animate-spin text-primary" />
            <span className="text-xs text-muted-foreground">Creating temp branch...</span>
          </div>
        )}

        {resolverStatus === "resolving" && (
          <div className="space-y-1">
            <div className="flex items-center gap-1.5 px-1 py-1">
              <Loader2 className="h-3 w-3 animate-spin text-primary" />
              <span className="text-xs text-muted-foreground">AI is resolving conflicts...</span>
            </div>
            <Button
              size="xs"
              variant="ghost"
              className="text-[10px] h-5 text-danger"
              onClick={onRejectResolve}
            >
              <XCircle className="h-3 w-3 mr-0.5" />
              Stop & Abort
            </Button>
          </div>
        )}

        {resolverStatus === "review" && (
          <div className="space-y-1.5">
            <div className="flex items-center gap-1.5 px-1 py-1">
              <CheckCircle2 className="h-3.5 w-3.5 text-success" />
              <span className="text-xs text-foreground font-medium">Conflicts resolved — review changes</span>
            </div>
            <div className="flex gap-1">
              <Button
                size="xs"
                className="text-[10px] h-6 flex-1 bg-success/20 text-success hover:bg-success/30"
                onClick={onApproveResolve}
              >
                <CheckCircle2 className="h-3 w-3 mr-0.5" />
                Approve
              </Button>
              <Button
                size="xs"
                variant="ghost"
                className="text-[10px] h-6 text-danger"
                onClick={onRejectResolve}
              >
                <XCircle className="h-3 w-3 mr-0.5" />
                Reject
              </Button>
            </div>
          </div>
        )}

        {resolverStatus === "applying" && (
          <div className="flex items-center gap-1.5 px-1 py-1">
            <Loader2 className="h-3 w-3 animate-spin text-success" />
            <span className="text-xs text-muted-foreground">Applying resolution...</span>
          </div>
        )}

        {resolverStatus === "error" && (
          <div className="space-y-1">
            <p className="text-[10px] text-danger break-words px-1">{resolverError}</p>
            <div className="flex gap-1">
              <Button
                size="xs"
                variant="ghost"
                className="text-[10px] h-5"
                onClick={onStartResolve}
              >
                Try Again
              </Button>
              <Button
                size="xs"
                variant="ghost"
                className="text-[10px] h-5 text-danger"
                onClick={onRejectResolve}
              >
                Abort
              </Button>
            </div>
          </div>
        )}
      </div>
    );
  }

  const [conflictsCollapsed, setConflictsCollapsed] = useState(false);

  return (
    <div className="py-1">
      <SectionHeader
        title="Conflicts"
        count={files.length}
        expanded={!conflictsCollapsed}
        onToggle={() => setConflictsCollapsed(!conflictsCollapsed)}
        variant="danger"
      />
      {!conflictsCollapsed && (
        <>
          {files.map((file) => (
            <ConflictFileRow
              key={file.path}
              file={file}
              cwd={cwd}
              onRefresh={onRefresh}
              onOpenDiff={onOpenDiff}
            />
          ))}
          <Tooltip>
        <TooltipTrigger asChild>
          <span className="block px-1.5 mt-1">
            <Button
              size="xs"
              variant="secondary"
              className="text-[10px] h-6 w-full"
              disabled={!resolverEnabled}
              onClick={onStartResolve}
            >
              <Sparkles className="h-3 w-3 mr-1" />
              Resolve with AI
            </Button>
          </span>
        </TooltipTrigger>
        {!resolverEnabled && (
          <TooltipContent side="bottom" className="text-xs">
            Configure in Settings &rarr; Git &rarr; AI Tools
          </TooltipContent>
        )}
          </Tooltip>
        </>
      )}
    </div>
  );
}

// ── Directory Group ──

function DirectoryGroup({
  dir,
  files,
  staged,
  cwd,
  expandedFile,
  expandedStaged,
  onToggleExpand,
  onRefresh,
  onOpenDiff,
  activeDiffFile,
}: {
  dir: string;
  files: GitFileStatus[];
  staged: boolean;
  cwd: string;
  expandedFile: string | null;
  expandedStaged: boolean;
  onToggleExpand: (path: string, staged: boolean) => void;
  onRefresh: () => void;
  onOpenDiff?: (filePath: string, staged: boolean) => void;
  activeDiffFile?: string | null;
}) {
  // Root-level files (no directory) render without header
  if (dir === "") {
    return (
      <>
        {files.map((f) => (
          <FileRow
            key={f.path}
            file={f}
            staged={staged}
            cwd={cwd}
            expanded={expandedFile === f.path && expandedStaged === staged}
            onToggleExpand={() => onToggleExpand(f.path, staged)}
            onRefresh={onRefresh}
            onOpenDiff={onOpenDiff}
            activeDiffFile={activeDiffFile}
          />
        ))}
      </>
    );
  }

  return (
    <div>
      <div className="flex items-center gap-1 px-1.5 py-0.5">
        <span className="truncate text-[10px] text-muted-foreground">{dir}</span>
        <span className="text-[10px] text-muted-foreground/50 shrink-0 tabular-nums">
          {files.length}
        </span>
      </div>
      <div>
        {files.map((f) => (
          <FileRow
            key={f.path}
            file={f}
            staged={staged}
            cwd={cwd}
            expanded={expandedFile === f.path && expandedStaged === staged}
            onToggleExpand={() => onToggleExpand(f.path, staged)}
            onRefresh={onRefresh}
            onOpenDiff={onOpenDiff}
            activeDiffFile={activeDiffFile}
          />
        ))}
      </div>
    </div>
  );
}

// ── File Section (Staged / Changes) ──

function FileSection({
  label,
  files,
  staged,
  cwd,
  expandedFile,
  expandedStaged,
  onToggleExpand,
  onRefresh,
  onBulkAction,
  bulkLabel,
  onOpenDiff,
  activeDiffFile,
  flat,
}: {
  label: string;
  files: GitFileStatus[];
  staged: boolean;
  cwd: string;
  expandedFile: string | null;
  expandedStaged: boolean;
  onToggleExpand: (path: string, staged: boolean) => void;
  onRefresh: () => void;
  onBulkAction: () => void;
  bulkLabel: string;
  onOpenDiff?: (filePath: string, staged: boolean) => void;
  activeDiffFile?: string | null;
  flat?: boolean;
}) {
  const [collapsed, setCollapsed] = useState(false);
  const groups = useMemo(() => groupByDirectory(files), [files]);

  return (
    <div>
      <SectionHeader
        title={label}
        count={files.length}
        expanded={!collapsed}
        onToggle={() => setCollapsed(!collapsed)}
        actions={
          <Tooltip>
            <TooltipTrigger asChild>
              <Button variant="ghost" size="icon-xs" className="h-6 w-6" onClick={onBulkAction}>
                {staged ? <Minus className="h-3.5 w-3.5" /> : <Plus className="h-3.5 w-3.5" />}
              </Button>
            </TooltipTrigger>
            <TooltipContent side="bottom" className="text-xs">{bulkLabel}</TooltipContent>
          </Tooltip>
        }
      />
      {!collapsed && (
        <div className="px-0.5 pb-1">
          {flat ? files.map((f) => (
            <FileRow
              key={f.path}
              file={f}
              staged={staged}
              cwd={cwd}
              expanded={expandedFile === f.path && expandedStaged === staged}
              onToggleExpand={() => onToggleExpand(f.path, staged)}
              onRefresh={onRefresh}
              onOpenDiff={onOpenDiff}
              activeDiffFile={activeDiffFile}
            />
          )) : groups.map((group) => (
            <DirectoryGroup
              key={group.dir}
              dir={group.dir}
              files={group.files}
              staged={staged}
              cwd={cwd}
              expandedFile={expandedFile}
              expandedStaged={expandedStaged}
              onToggleExpand={onToggleExpand}
              onRefresh={onRefresh}
              onOpenDiff={onOpenDiff}
              activeDiffFile={activeDiffFile}
            />
          ))}
        </div>
      )}
    </div>
  );
}

// ── Main Panel ──

export function ChangesPanel({ workspace }: Props) {
  const cwd = workspace.worktree_path ?? workspace.cwd;
  const [files, setFiles] = useState<GitFileStatus[]>([]);
  const [branchInfo, setBranchInfo] = useState<GitBranchInfo | null>(null);
  const [commits, setCommits] = useState<GitLogEntry[]>([]);
  const [mergeState, setMergeState] = useState<MergeState | null>(null);
  const [expandedFile, setExpandedFile] = useState<string | null>(null);
  const [expandedStaged, setExpandedStaged] = useState(false);
  const [commitMsg, setCommitMsg] = useState("");
  const [busyAction, setBusyAction] = useState<"commit" | "push" | "pull" | "merge" | "fetch" | "stash" | null>(null);
  const [viewMode, setViewMode] = useState<"grouped" | "flat">("grouped");
  const [gitError, setGitError] = useState<string | null>(null);
  const [commitsExpanded, setCommitsExpanded] = useState(false);
  const [expandedCommits, setExpandedCommits] = useState<Set<string>>(new Set());
  const [claudeReady, setClaudeReady] = useState<boolean | null>(null);
  const refreshRef = useRef<ReturnType<typeof setInterval> | null>(null);

  // Against-base state
  const [baseBranch, setBaseBranch] = useState<string>("main");
  const [baseBranchFiles, setBaseBranchFiles] = useState<GitFileStatus[]>([]);
  const [baseBranchExpanded, setBaseBranchExpanded] = useState(false);
  const [remoteBranches, setRemoteBranches] = useState<string[]>([]);

  const config = useAppStore((s) => s.appState?.config);
  const aiEnabled = config?.ai_commit_message_enabled ?? true;

  const generation = useAiCommitStore((s) => s.getGeneration(workspace.workspace_id));
  const requestGeneration = useAiCommitStore((s) => s.requestGeneration);
  const consumeMessage = useAiCommitStore((s) => s.consumeMessage);
  const clearGeneration = useAiCommitStore((s) => s.clearGeneration);
  const isGenerating = generation?.status === "generating";

  const refresh = useCallback(() => {
    if (!cwd) return;
    Promise.all([
      getGitStatus(cwd).catch((e) => { console.error("[ChangesPanel] git status failed:", e); return [] as GitFileStatus[]; }),
      getGitBranchInfo(cwd).catch((e) => { console.error("[ChangesPanel] branch info failed:", e); return null; }),
      gitLogEntries(cwd, 10).catch((e) => { console.error("[ChangesPanel] git log failed:", e); return [] as GitLogEntry[]; }),
      getMergeState(cwd).catch(() => null as MergeState | null),
    ]).then(([status, info, log, merge]) => {
      setFiles(status);
      if (info) setBranchInfo(info);
      setCommits(log);
      setMergeState(merge);
    });
  }, [cwd]);

  useEffect(() => {
    refresh();
    refreshRef.current = setInterval(refresh, 10000);
    return () => {
      if (refreshRef.current) clearInterval(refreshRef.current);
    };
  }, [refresh]);

  useEffect(() => {
    if (gitError) {
      const t = setTimeout(() => setGitError(null), 8000);
      return () => clearTimeout(t);
    }
  }, [gitError]);

  useEffect(() => {
    if (aiEnabled) {
      checkClaudeAvailable().then(setClaudeReady).catch(() => setClaudeReady(false));
    }
  }, [aiEnabled]);

  useEffect(() => {
    if (generation?.status === "done") {
      const msg = consumeMessage(workspace.workspace_id);
      if (msg) setCommitMsg(msg);
    } else if (generation?.status === "error") {
      setGitError(generation.error ?? "Generation failed");
      clearGeneration(workspace.workspace_id);
    }
  }, [generation?.status, workspace.workspace_id, consumeMessage, clearGeneration]);

  // Fetch default branch and remote branches on mount
  useEffect(() => {
    if (!cwd) return;
    getDefaultBranch(cwd).then(setBaseBranch).catch(() => setBaseBranch("main"));
    listBranches(cwd, true).then(setRemoteBranches).catch(() => {});
  }, [cwd]);

  // Fetch base branch diff
  const refreshBaseDiff = useCallback(() => {
    if (!cwd) return;
    getBaseBranchDiff(cwd, baseBranch)
      .then((result) => setBaseBranchFiles(result.files))
      .catch(() => setBaseBranchFiles([]));
  }, [cwd, baseBranch]);

  useEffect(() => { refreshBaseDiff(); }, [refreshBaseDiff]);

  const staged = useMemo(() => files.filter((f) => f.is_staged), [files]);
  const unstaged = useMemo(() => files.filter((f) => f.is_unstaged), [files]);
  const conflicted = useMemo(() => files.filter((f) => f.status === "conflicted"), [files]);
  const isMerging = mergeState?.is_merging || mergeState?.is_rebasing || false;

  const resolver = useAiMergeStore((s) => s.getResolver(workspace.workspace_id));
  const startResolution = useAiMergeStore((s) => s.startResolution);
  const approveResolution = useAiMergeStore((s) => s.approveResolution);
  const rejectResolution = useAiMergeStore((s) => s.rejectResolution);

  const handleStartResolve = () => {
    const cli = config?.ai_resolver_cli ?? "claude";
    const model = config?.ai_resolver_model ?? null;
    const strategy = config?.ai_resolver_strategy ?? "smart_merge";
    const target = branchInfo?.branch === "main" ? "main" : "main"; // TODO: detect target from PR or config
    startResolution(workspace.workspace_id, cwd, target, cli, model, strategy);
  };

  const handleApproveResolve = () => {
    approveResolution(workspace.workspace_id, cwd, "Resolve merge conflicts").then(refresh);
  };

  const handleRejectResolve = () => {
    rejectResolution(workspace.workspace_id, cwd).then(refresh);
  };

  const busy = busyAction !== null;

  const handleGenerateCommitMsg = () => {
    if (isGenerating || staged.length === 0) return;
    setGitError(null);
    const model = config?.ai_commit_message_model ?? null;
    requestGeneration(workspace.workspace_id, cwd, model);
  };

  const handleAbortMerge = async () => {
    if (busy) return;
    setBusyAction("commit");
    setGitError(null);
    try {
      await abortMerge(cwd);
      refresh();
    } catch (err) {
      setGitError(String(err));
    } finally {
      setBusyAction(null);
    }
  };

  const handleCompleteMerge = async () => {
    if (busy) return;
    setBusyAction("commit");
    setGitError(null);
    try {
      const msg = commitMsg.trim() || "Merge commit";
      await continueMerge(cwd, msg);
      setCommitMsg("");
      refresh();
    } catch (err) {
      setGitError(String(err));
    } finally {
      setBusyAction(null);
    }
  };

  const handleCommit = async () => {
    if (!commitMsg.trim() || staged.length === 0 || busy || conflicted.length > 0) return;
    setBusyAction("commit");
    setGitError(null);
    try {
      await gitCommitChanges(cwd, commitMsg.trim());
      setCommitMsg("");
      setExpandedFile(null);
      refresh();
      refreshBaseDiff();
    } catch (err) {
      setGitError(String(err));
    } finally {
      setBusyAction(null);
    }
  };

  const handlePush = async () => {
    if (busy) return;
    setBusyAction("push");
    setGitError(null);
    try {
      const needsPublish = branchInfo && !branchInfo.has_upstream;
      await gitPushChanges(cwd, !!needsPublish);
      refresh();
      refreshBaseDiff();
    } catch (err) {
      setGitError(String(err));
    } finally {
      setBusyAction(null);
    }
  };

  const handlePull = async () => {
    if (busy) return;
    setBusyAction("pull");
    setGitError(null);
    try {
      await gitPullChanges(cwd);
      refresh();
    } catch (err) {
      setGitError(String(err));
    } finally {
      setBusyAction(null);
    }
  };

  const handleFetch = async () => {
    if (busy) return;
    setBusyAction("fetch");
    setGitError(null);
    try {
      await gitFetchChanges(cwd);
      refresh();
    } catch (err) {
      setGitError(String(err));
    } finally {
      setBusyAction(null);
    }
  };

  const handleFetchAndPull = async () => {
    if (busy) return;
    setBusyAction("fetch");
    setGitError(null);
    try {
      await gitFetchChanges(cwd);
      setBusyAction("pull");
      await gitPullChanges(cwd);
      refresh();
    } catch (err) {
      setGitError(String(err));
    } finally {
      setBusyAction(null);
    }
  };

  const handleSync = async () => {
    if (busy) return;
    setBusyAction("pull");
    setGitError(null);
    try {
      await gitPullChanges(cwd);
      setBusyAction("push");
      await gitPushChanges(cwd, false);
      refresh();
      refreshBaseDiff();
    } catch (err) {
      setGitError(String(err));
    } finally {
      setBusyAction(null);
    }
  };

  const handleStash = async (includeUntracked: boolean) => {
    if (busy) return;
    setBusyAction("stash");
    setGitError(null);
    try {
      await gitStashPush(cwd, includeUntracked);
      refresh();
    } catch (err) {
      setGitError(String(err));
    } finally {
      setBusyAction(null);
    }
  };

  const handleStashPop = async () => {
    if (busy) return;
    setBusyAction("stash");
    setGitError(null);
    try {
      await gitStashPop(cwd);
      refresh();
    } catch (err) {
      setGitError(String(err));
    } finally {
      setBusyAction(null);
    }
  };

  const [mergeSuccess, setMergeSuccess] = useState<string | null>(null);

  useEffect(() => {
    if (mergeSuccess) {
      const t = setTimeout(() => setMergeSuccess(null), 4000);
      return () => clearTimeout(t);
    }
  }, [mergeSuccess]);

  const handleMergeBranch = async () => {
    if (busy || isMerging) return;
    setBusyAction("merge");
    setGitError(null);
    setMergeSuccess(null);
    try {
      const result = await mergeBranch(cwd, baseBranch);
      refresh();
      refreshBaseDiff();
      if (result === "up_to_date") {
        setMergeSuccess(`Already up to date with ${baseBranch}`);
      } else if (result === "merged") {
        setMergeSuccess(`Merged ${baseBranch} into current branch`);
      }
      // "conflicts" — merge banner and conflict UI handle this automatically
    } catch (err) {
      setGitError(String(err));
    } finally {
      setBusyAction(null);
    }
  };

  // ── Merge into base state ──
  const [mergeIntoBaseState, setMergeIntoBaseState] = useState<{
    active: boolean;
    sourceBranch: string;
    baseBranch: string;
    tempBranch: string;
    deleteSourceAfter: boolean;
  } | null>(null);
  const [showMergeIntoDialog, setShowMergeIntoDialog] = useState(false);
  const [mergeIntoDeleteBranch, setMergeIntoDeleteBranch] = useState(false);

  const handleMergeIntoBase = async () => {
    if (busy || isMerging) return;
    setBusyAction("merge");
    setGitError(null);
    setMergeSuccess(null);
    setMergeIntoBaseState(null);
    setShowMergeIntoDialog(false);
    try {
      const result = await mergeIntoBase(cwd, baseBranch, mergeIntoDeleteBranch);
      if (result.status === "already_up_to_date") {
        setMergeSuccess(`Already up to date — nothing to merge into ${baseBranch}`);
      } else if (result.status === "merged") {
        setMergeSuccess(`Successfully merged ${result.source_branch} into ${baseBranch}`);
      } else if (result.status === "conflicts" && result.temp_branch) {
        setMergeIntoBaseState({
          active: true,
          sourceBranch: result.source_branch,
          baseBranch,
          tempBranch: result.temp_branch,
          deleteSourceAfter: mergeIntoDeleteBranch,
        });
      }
    } catch (err) {
      setGitError(String(err));
    } finally {
      setBusyAction(null);
      try { refresh(); } catch { /* ignore */ }
      try { refreshBaseDiff(); } catch { /* ignore */ }
    }
  };

  const handleCompleteMergeIntoBase = async () => {
    if (!mergeIntoBaseState || busy) return;
    setBusyAction("merge");
    setGitError(null);
    try {
      await completeMergeIntoBase(
        cwd,
        mergeIntoBaseState.baseBranch,
        mergeIntoBaseState.tempBranch,
        mergeIntoBaseState.sourceBranch,
        mergeIntoBaseState.deleteSourceAfter,
      );
      setMergeSuccess(`Successfully merged ${mergeIntoBaseState.sourceBranch} into ${mergeIntoBaseState.baseBranch}`);
      setMergeIntoBaseState(null);
      refresh();
      refreshBaseDiff();
    } catch (err) {
      setGitError(String(err));
    } finally {
      setBusyAction(null);
    }
  };

  const handleAbortMergeIntoBase = async () => {
    if (!mergeIntoBaseState || busy) return;
    setBusyAction("merge");
    setGitError(null);
    try {
      await abortMergeIntoBase(cwd, mergeIntoBaseState.sourceBranch, mergeIntoBaseState.tempBranch);
      setMergeSuccess(`Merge aborted. No changes were made to ${mergeIntoBaseState.baseBranch}.`);
      setMergeIntoBaseState(null);
      refresh();
      refreshBaseDiff();
    } catch (err) {
      setGitError(String(err));
    } finally {
      setBusyAction(null);
    }
  };

  const handleStageAll = async () => {
    const paths = unstaged.map((f) => f.path);
    if (paths.length === 0) return;
    await gitStageFiles(cwd, paths).catch(console.error);
    refresh();
  };

  const handleUnstageAll = async () => {
    const paths = staged.map((f) => f.path);
    if (paths.length === 0) return;
    await gitUnstageFiles(cwd, paths).catch(console.error);
    refresh();
  };

  // Smart primary action for ButtonGroup
  const canCommit = !!commitMsg.trim() && staged.length > 0 && !busy && conflicted.length === 0;
  const pushCount = branchInfo?.ahead ?? 0;
  const pullCount = branchInfo?.behind ?? 0;
  const hasUpstream = branchInfo?.has_upstream ?? false;

  const primaryAction = (() => {
    if (canCommit) {
      return {
        label: "Commit",
        icon: busyAction === "commit" ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <GitCommit className="h-3.5 w-3.5" />,
        handler: handleCommit,
        disabled: false,
        tooltip: "Commit staged changes (Ctrl+Enter)",
        badge: null as string | null,
      };
    }
    if (pushCount > 0 && pullCount > 0) {
      return {
        label: "Sync",
        icon: (busyAction === "push" || busyAction === "pull") ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <ArrowDownUp className="h-3.5 w-3.5" />,
        handler: handleSync,
        disabled: busy,
        tooltip: `Pull ${pullCount}, push ${pushCount}`,
        badge: `${pullCount}/${pushCount}`,
      };
    }
    if (pushCount > 0) {
      return {
        label: "Push",
        icon: busyAction === "push" ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <ArrowUp className="h-3.5 w-3.5" />,
        handler: handlePush,
        disabled: busy,
        tooltip: `Push ${pushCount} commit${pushCount !== 1 ? "s" : ""} to remote`,
        badge: String(pushCount),
      };
    }
    if (pullCount > 0) {
      return {
        label: "Pull",
        icon: busyAction === "pull" ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <ArrowDown className="h-3.5 w-3.5" />,
        handler: handlePull,
        disabled: busy,
        tooltip: `Pull ${pullCount} commit${pullCount !== 1 ? "s" : ""}`,
        badge: String(pullCount),
      };
    }
    if (!hasUpstream && branchInfo?.branch) {
      return {
        label: "Publish Branch",
        icon: busyAction === "push" ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <ArrowUp className="h-3.5 w-3.5" />,
        handler: handlePush,
        disabled: busy,
        tooltip: "Publish branch to remote",
        badge: null,
      };
    }
    return {
      label: "Commit",
      icon: <GitCommit className="h-3.5 w-3.5" />,
      handler: handleCommit,
      disabled: true,
      tooltip: staged.length === 0 ? "No staged changes" : "Enter a commit message",
      badge: null,
    };
  })();

  const handleToggleExpand = (path: string, isStaged: boolean) => {
    if (expandedFile === path && expandedStaged === isStaged) {
      setExpandedFile(null);
    } else {
      setExpandedFile(path);
      setExpandedStaged(isStaged);
    }
  };

  const diffSetFile = useDiffStore((s) => s.setFile);
  const diffInitTab = useDiffStore((s) => s.initTab);
  const diffSetSection = useDiffStore((s) => s.setSection);
  const diffSetBaseBranch = useDiffStore((s) => s.setBaseBranch);

  // Find active diff tab and its current file for highlighting
  const activeDiffTab = workspace.tabs.find((t) => t.kind === "diff");
  const activeDiffState = useDiffStore((s) =>
    activeDiffTab ? s.getTab(activeDiffTab.tab_id) : undefined,
  );
  const activeDiffFile = activeDiffState?.filePath ?? null;

  const handleOpenDiff = useCallback(
    async (filePath: string, isStaged: boolean) => {
      const existingDiffTab = workspace.tabs.find((t) => t.kind === "diff");
      if (existingDiffTab) {
        await activateTab(workspace.workspace_id, existingDiffTab.tab_id).catch(
          console.error,
        );
        diffSetFile(existingDiffTab.tab_id, filePath, isStaged);
      } else {
        try {
          const tabId = await createTab(workspace.workspace_id, "diff");
          diffInitTab(tabId, { file: filePath, staged: isStaged });
        } catch (err) {
          console.error("Failed to create diff tab:", err);
        }
      }
    },
    [workspace, diffSetFile, diffInitTab],
  );

  const handleOpenBaseDiff = useCallback(
    async (filePath: string) => {
      const existingDiffTab = workspace.tabs.find((t) => t.kind === "diff");
      if (existingDiffTab) {
        await activateTab(workspace.workspace_id, existingDiffTab.tab_id).catch(console.error);
        diffSetFile(existingDiffTab.tab_id, filePath, false);
        diffSetBaseBranch(existingDiffTab.tab_id, baseBranch);
        diffSetSection(existingDiffTab.tab_id, "against_base");
      } else {
        try {
          const tabId = await createTab(workspace.workspace_id, "diff");
          diffInitTab(tabId, { file: filePath, staged: false });
          diffSetBaseBranch(tabId, baseBranch);
          diffSetSection(tabId, "against_base");
        } catch (err) {
          console.error("Failed to create diff tab:", err);
        }
      }
    },
    [workspace, baseBranch, diffSetFile, diffSetBaseBranch, diffSetSection, diffInitTab],
  );

  return (
    <>
    <TooltipProvider>
      <div className="flex h-full flex-col">
        {/* Toolbar row — above commit area, matches Superset layout */}
        <div className="flex items-center gap-0.5 px-2 py-1.5">
          {/* Base branch selector */}
          {remoteBranches.length > 1 ? (
            <DropdownMenu>
              <Tooltip>
                <TooltipTrigger asChild>
                  <DropdownMenuTrigger asChild>
                    <Button variant="ghost" size="icon-xs" className="size-6 p-0">
                      <GitBranch className="h-3.5 w-3.5" />
                    </Button>
                  </DropdownMenuTrigger>
                </TooltipTrigger>
                <TooltipContent side="bottom" className="text-xs">
                  Base: {baseBranch}
                </TooltipContent>
              </Tooltip>
              <DropdownMenuContent align="start" className="max-h-60 overflow-y-auto">
                {remoteBranches.map((b) => (
                  <DropdownMenuItem
                    key={b}
                    onClick={() => setBaseBranch(b)}
                    className="text-xs"
                  >
                    {b === baseBranch && <Check className="h-3 w-3 mr-1.5 shrink-0" />}
                    <span className={b !== baseBranch ? "pl-[18px]" : ""}>{b}</span>
                  </DropdownMenuItem>
                ))}
              </DropdownMenuContent>
            </DropdownMenu>
          ) : (
            <Tooltip>
              <TooltipTrigger asChild>
                <Button variant="ghost" size="icon-xs" className="size-6 p-0" disabled>
                  <GitBranch className="h-3.5 w-3.5" />
                </Button>
              </TooltipTrigger>
              <TooltipContent side="bottom" className="text-xs">
                Base: {baseBranch}
              </TooltipContent>
            </Tooltip>
          )}

          {/* Stash dropdown */}
          <DropdownMenu>
            <Tooltip>
              <TooltipTrigger asChild>
                <DropdownMenuTrigger asChild>
                  <Button variant="ghost" size="icon-xs" className="size-6 p-0" disabled={busy}>
                    <Archive className="h-3.5 w-3.5" />
                  </Button>
                </DropdownMenuTrigger>
              </TooltipTrigger>
              <TooltipContent side="bottom" className="text-xs">
                Stash
              </TooltipContent>
            </Tooltip>
            <DropdownMenuContent align="start" className="w-52">
              <DropdownMenuItem onClick={() => handleStash(false)} className="text-xs">
                <Archive className="h-3.5 w-3.5" />
                Stash Changes
              </DropdownMenuItem>
              <DropdownMenuItem onClick={() => handleStash(true)} className="text-xs">
                <Archive className="h-3.5 w-3.5" />
                Stash (Include Untracked)
              </DropdownMenuItem>
              <DropdownMenuSeparator />
              <DropdownMenuItem onClick={handleStashPop} className="text-xs">
                <ArchiveRestore className="h-3.5 w-3.5" />
                Pop Stash
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>

          {/* Merge dropdown */}
          <DropdownMenu>
            <Tooltip>
              <TooltipTrigger asChild>
                <DropdownMenuTrigger asChild>
                  <Button
                    variant="ghost"
                    size="icon-xs"
                    className="size-6 p-0"
                    disabled={busy || branchInfo?.branch === baseBranch}
                  >
                    {busyAction === "merge" ? (
                      <Loader2 className="h-3.5 w-3.5 animate-spin" />
                    ) : (
                      <GitMerge className="h-3.5 w-3.5" />
                    )}
                  </Button>
                </DropdownMenuTrigger>
              </TooltipTrigger>
              <TooltipContent side="bottom" className="text-xs">
                {branchInfo?.branch === baseBranch ? "Already on base branch" : "Merge"}
              </TooltipContent>
            </Tooltip>
            <DropdownMenuContent align="start" className="w-52">
              <DropdownMenuItem
                onClick={handleMergeBranch}
                disabled={isMerging}
                className="text-xs"
              >
                <GitMerge className="h-3.5 w-3.5" />
                Merge {baseBranch} into current
              </DropdownMenuItem>
              <DropdownMenuItem
                onClick={() => setShowMergeIntoDialog(true)}
                disabled={isMerging || !!mergeIntoBaseState}
                className="text-xs"
              >
                <ArrowUpToLine className="h-3.5 w-3.5" />
                Merge into {baseBranch}
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>

          {/* View mode toggle */}
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon-xs"
                className="size-6 p-0"
                onClick={() => setViewMode(viewMode === "grouped" ? "flat" : "grouped")}
              >
                {viewMode === "grouped" ? <FolderTree className="h-3.5 w-3.5" /> : <List className="h-3.5 w-3.5" />}
              </Button>
            </TooltipTrigger>
            <TooltipContent side="bottom" className="text-xs">
              {viewMode === "grouped" ? "Switch to flat view" : "Switch to grouped view"}
            </TooltipContent>
          </Tooltip>

          {/* Refresh */}
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon-xs"
                className="size-6 p-0"
                onClick={() => { refresh(); refreshBaseDiff(); }}
              >
                <RefreshCw className="h-3.5 w-3.5" />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="bottom" className="text-xs">
              Refresh
            </TooltipContent>
          </Tooltip>
        </div>

        {/* Commit area */}
        <div className="flex flex-col gap-1.5 px-2 py-2 border-b border-border">
          {/* Textarea with AI sparkle overlay */}
          <div className="relative">
            <Textarea
              placeholder="Commit message"
              value={commitMsg}
              onChange={(e) => setCommitMsg(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && (e.metaKey || e.ctrlKey) && !primaryAction.disabled) {
                  e.preventDefault();
                  primaryAction.handler();
                }
              }}
              disabled={isGenerating}
              className="min-h-[52px] resize-none text-xs bg-background pr-7"
            />
            {aiEnabled && (
              <Tooltip>
                <TooltipTrigger asChild>
                  <span className="absolute top-1.5 right-1.5 shrink-0" tabIndex={0}>
                    <Button
                      variant="ghost"
                      size="icon-xs"
                      className="h-5 w-5"
                      disabled={staged.length === 0 || isGenerating || claudeReady === false}
                      onClick={handleGenerateCommitMsg}
                    >
                      {isGenerating
                        ? <Loader2 className="h-3 w-3 animate-spin" />
                        : <Sparkles className="h-3 w-3" />}
                    </Button>
                  </span>
                </TooltipTrigger>
                <TooltipContent side="top" className="text-xs">
                  {claudeReady === false
                    ? "Claude CLI not found"
                    : staged.length === 0
                      ? "Stage files first"
                      : "Generate commit message"}
                </TooltipContent>
              </Tooltip>
            )}
          </div>

          {/* Smart ButtonGroup: primary action + dropdown */}
          <div data-slot="button-group" className="flex w-full items-stretch">
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  variant="secondary"
                  size="sm"
                  className="flex-1 gap-1.5 h-7 text-xs rounded-r-none border-r-0"
                  onClick={primaryAction.handler}
                  disabled={primaryAction.disabled}
                >
                  {primaryAction.icon}
                  <span>{primaryAction.label}</span>
                  {primaryAction.badge && (
                    <span className="text-[10px] opacity-70">{primaryAction.badge}</span>
                  )}
                </Button>
              </TooltipTrigger>
              <TooltipContent side="bottom" className="text-xs">{primaryAction.tooltip}</TooltipContent>
            </Tooltip>
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button
                  variant="secondary"
                  size="sm"
                  className="h-7 px-1.5 rounded-l-none"
                  disabled={busy}
                >
                  <ChevronDown className="h-3.5 w-3.5" />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" className="w-48 text-xs">
                <DropdownMenuItem
                  onClick={handleCommit}
                  disabled={!commitMsg.trim() || staged.length === 0 || busy || conflicted.length > 0}
                  className="text-xs"
                >
                  <GitCommit className="h-3.5 w-3.5" />
                  Commit
                </DropdownMenuItem>
                <DropdownMenuItem
                  onClick={() => {
                    if (!commitMsg.trim() || staged.length === 0 || busy) return;
                    (async () => {
                      try {
                        await gitCommitChanges(cwd, commitMsg.trim());
                        setCommitMsg("");
                        await handlePush();
                      } catch (err) { setGitError(String(err)); }
                      refresh();
                      refreshBaseDiff();
                    })();
                  }}
                  disabled={!commitMsg.trim() || staged.length === 0 || busy}
                  className="text-xs"
                >
                  <ArrowUp className="h-3.5 w-3.5" />
                  Commit & Push
                </DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem
                  onClick={handlePush}
                  disabled={busy || (branchInfo?.has_upstream && (branchInfo?.ahead ?? 0) === 0)}
                  className="text-xs"
                >
                  <ArrowUp className="h-3.5 w-3.5" />
                  <span className="flex-1">{!branchInfo?.has_upstream ? "Publish Branch" : "Push"}</span>
                  {(branchInfo?.ahead ?? 0) > 0 && (
                    <span className="text-[10px] text-muted-foreground">{branchInfo?.ahead}</span>
                  )}
                </DropdownMenuItem>
                <DropdownMenuItem
                  onClick={handlePull}
                  disabled={busy || !branchInfo?.has_upstream || (branchInfo?.behind ?? 0) === 0}
                  className="text-xs"
                >
                  <ArrowDown className="h-3.5 w-3.5" />
                  <span className="flex-1">Pull</span>
                  {(branchInfo?.behind ?? 0) > 0 && (
                    <span className="text-[10px] text-muted-foreground">{branchInfo?.behind}</span>
                  )}
                </DropdownMenuItem>
                <DropdownMenuItem
                  onClick={handleSync}
                  disabled={busy || ((branchInfo?.ahead ?? 0) === 0 && (branchInfo?.behind ?? 0) === 0)}
                  className="text-xs"
                >
                  <ArrowDownUp className="h-3.5 w-3.5" />
                  Sync
                </DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem onClick={handleFetch} disabled={busy} className="text-xs">
                  <Download className="h-3.5 w-3.5" />
                  Fetch
                </DropdownMenuItem>
                <DropdownMenuItem onClick={handleFetchAndPull} disabled={busy} className="text-xs">
                  <Download className="h-3.5 w-3.5" />
                  Fetch & Pull
                </DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem onClick={() => { refresh(); refreshBaseDiff(); }} className="text-xs">
                  <RefreshCw className="h-3.5 w-3.5" />
                  Refresh
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>

          {gitError && (
            <p className="text-[10px] text-destructive break-words px-0.5">{gitError}</p>
          )}
        </div>

        {/* Merge conflict banner */}
        {isMerging && mergeIntoBaseState?.active ? (
          <div className="px-1.5 py-1.5 bg-primary/10 border-b border-primary/20">
            <div className="flex items-center gap-1.5 mb-1">
              <ArrowUpToLine className="h-3.5 w-3.5 text-primary shrink-0" />
              <span className="text-xs text-primary font-medium">
                Merging &ldquo;{mergeIntoBaseState.sourceBranch}&rdquo; into &ldquo;{mergeIntoBaseState.baseBranch}&rdquo;
                {conflicted.length > 0 && ` — ${conflicted.length} conflict${conflicted.length !== 1 ? "s" : ""}`}
              </span>
            </div>
            <div className="flex gap-1">
              {conflicted.length === 0 ? (
                <Button
                  size="xs"
                  className="text-[10px] h-5 flex-1 bg-success/20 text-success hover:bg-success/30"
                  onClick={handleCompleteMergeIntoBase}
                  disabled={busy}
                >
                  <GitMerge className="h-3 w-3 mr-0.5" />
                  {busyAction === "merge" ? "Completing..." : `Complete Merge into ${mergeIntoBaseState.baseBranch}`}
                </Button>
              ) : null}
              <Button
                size="xs"
                variant="ghost"
                className="text-[10px] h-5 text-danger hover:text-danger"
                onClick={handleAbortMergeIntoBase}
                disabled={busy}
              >
                <XCircle className="h-3 w-3 mr-0.5" />
                Abort
              </Button>
            </div>
          </div>
        ) : isMerging ? (
          <div className="px-1.5 py-1.5 bg-danger/10 border-b border-danger/20">
            <div className="flex items-center gap-1.5 mb-1">
              <AlertTriangle className="h-3.5 w-3.5 text-danger shrink-0" />
              <span className="text-xs text-danger font-medium">
                {mergeState?.is_rebasing ? "Rebase" : "Merge"} in progress
                {conflicted.length > 0 && ` — ${conflicted.length} conflict${conflicted.length !== 1 ? "s" : ""}`}
              </span>
            </div>
            <div className="flex gap-1">
              {conflicted.length === 0 ? (
                <Button
                  size="xs"
                  className="text-[10px] h-5 flex-1 bg-success/20 text-success hover:bg-success/30"
                  onClick={handleCompleteMerge}
                  disabled={busy}
                >
                  <GitMerge className="h-3 w-3 mr-0.5" />
                  {busyAction === "commit" ? "Completing..." : "Complete Merge"}
                </Button>
              ) : null}
              <Button
                size="xs"
                variant="ghost"
                className="text-[10px] h-5 text-danger hover:text-danger"
                onClick={handleAbortMerge}
                disabled={busy}
              >
                <XCircle className="h-3 w-3 mr-0.5" />
                Abort
              </Button>
            </div>
          </div>
        ) : null}

        {/* No changes message */}
        {files.length === 0 && !isMerging && (
          <div className="flex flex-col items-center justify-center min-h-[120px] text-muted-foreground">
            <Check className="h-5 w-5 mb-1.5 opacity-40" />
            <p className="text-xs">No changes</p>
          </div>
        )}

        {/* File list */}
        <div className="flex-1 min-h-0 overflow-y-auto">
          <div className="px-1">
            {(conflicted.length > 0 || resolver.status !== "idle") && (
              <ConflictsSection
                files={conflicted}
                cwd={cwd}
                onRefresh={refresh}
                onOpenDiff={handleOpenDiff}
                resolverEnabled={config?.ai_resolver_enabled ?? false}
                resolverStatus={resolver.status}
                resolverError={resolver.error}
                onStartResolve={handleStartResolve}
                onApproveResolve={handleApproveResolve}
                onRejectResolve={handleRejectResolve}
              />
            )}

            {staged.length > 0 && (
              <FileSection
                label="Staged"
                files={staged}
                staged
                cwd={cwd}
                expandedFile={expandedFile}
                expandedStaged={expandedStaged}
                onToggleExpand={handleToggleExpand}
                onRefresh={refresh}
                onBulkAction={handleUnstageAll}
                bulkLabel="Unstage all"
                onOpenDiff={handleOpenDiff}
                activeDiffFile={activeDiffFile}
                flat={viewMode === "flat"}
              />
            )}

            {unstaged.length > 0 && (
              <FileSection
                label="Unstaged"
                files={unstaged}
                staged={false}
                cwd={cwd}
                expandedFile={expandedFile}
                expandedStaged={expandedStaged}
                onToggleExpand={handleToggleExpand}
                onRefresh={refresh}
                onBulkAction={handleStageAll}
                bulkLabel="Stage all"
                onOpenDiff={handleOpenDiff}
                activeDiffFile={activeDiffFile}
                flat={viewMode === "flat"}
              />
            )}

            {mergeSuccess && (
              <div className="px-2 py-1">
                <p className="text-[10px] text-success flex items-center gap-1">
                  <CheckCircle2 className="h-3 w-3 shrink-0" />
                  {mergeSuccess}
                </p>
              </div>
            )}

            {/* Against Base — files changed vs base branch (hidden when on the base branch itself) */}
            {baseBranchFiles.length > 0 && branchInfo?.branch !== baseBranch && (
              <div className="py-1">
                <SectionHeader
                  title={`Against ${baseBranch}`}
                  count={baseBranchFiles.length}
                  expanded={baseBranchExpanded}
                  onToggle={() => setBaseBranchExpanded(!baseBranchExpanded)}
                  actions={
                    <>
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <Button
                            variant="ghost"
                            size="icon-xs"
                            className="h-6 w-6"
                            onClick={handleMergeBranch}
                            disabled={busy || isMerging}
                          >
                            {busyAction === "merge" ? (
                              <Loader2 className="h-3.5 w-3.5 animate-spin" />
                            ) : (
                              <GitMerge className="h-3.5 w-3.5" />
                            )}
                          </Button>
                        </TooltipTrigger>
                        <TooltipContent side="left">
                          Merge {baseBranch} into current branch
                        </TooltipContent>
                      </Tooltip>
                      {branchInfo?.branch !== baseBranch && (
                        <Tooltip>
                          <TooltipTrigger asChild>
                            <Button
                              variant="ghost"
                              size="icon-xs"
                              className="h-6 w-6"
                              onClick={() => setShowMergeIntoDialog(true)}
                              disabled={busy || isMerging || !!mergeIntoBaseState}
                            >
                              <ArrowUpToLine className="h-3.5 w-3.5" />
                            </Button>
                          </TooltipTrigger>
                          <TooltipContent side="left">
                            Merge current branch into {baseBranch}
                          </TooltipContent>
                        </Tooltip>
                      )}
                    </>
                  }
                />
                {baseBranchExpanded && (
                  <div>
                    {viewMode === "flat" ? baseBranchFiles.map((f) => (
                      <FileRow
                        key={f.path}
                        file={f}
                        staged={false}
                        cwd={cwd}
                        expanded={false}
                        onToggleExpand={() => {}}
                        onRefresh={refreshBaseDiff}
                        onOpenDiff={(filePath) => handleOpenBaseDiff(filePath)}
                        activeDiffFile={activeDiffFile}
                      />
                    )) : groupByDirectory(baseBranchFiles).map((group) => (
                      <DirectoryGroup
                        key={group.dir}
                        dir={group.dir}
                        files={group.files}
                        staged={false}
                        cwd={cwd}
                        expandedFile={null}
                        expandedStaged={false}
                        onToggleExpand={() => {}}
                        onRefresh={refreshBaseDiff}
                        onOpenDiff={(filePath) => handleOpenBaseDiff(filePath)}
                        activeDiffFile={activeDiffFile}
                      />
                    ))}
                  </div>
                )}
              </div>
            )}

            {/* Commits — collapsed by default */}
            {commits.length > 0 && (
              <div className="py-1">
                <SectionHeader
                  title="Commits"
                  count={commits.length}
                  expanded={commitsExpanded}
                  onToggle={() => setCommitsExpanded(!commitsExpanded)}
                />
                {commitsExpanded &&
                  commits.map((commit) => (
                    <CommitRow
                      key={commit.hash}
                      commit={commit}
                      cwd={cwd}
                      expanded={expandedCommits.has(commit.hash)}
                      onToggle={() => {
                        setExpandedCommits((prev) => {
                          const next = new Set(prev);
                          if (next.has(commit.hash)) next.delete(commit.hash);
                          else next.add(commit.hash);
                          return next;
                        });
                      }}
                    />
                  ))}
              </div>
            )}
          </div>
        </div>
      </div>
    </TooltipProvider>

    {/* Merge into base confirmation dialog */}
    <AlertDialog open={showMergeIntoDialog} onOpenChange={setShowMergeIntoDialog}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Merge into {baseBranch}</AlertDialogTitle>
          <AlertDialogDescription>
            This will merge your changes from &ldquo;{branchInfo?.branch}&rdquo; into &ldquo;{baseBranch}&rdquo;.
            A temporary branch will be used to ensure {baseBranch} is not modified until the merge is verified clean.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <label className="flex items-center gap-2 py-2 cursor-pointer">
          <input
            type="checkbox"
            checked={mergeIntoDeleteBranch}
            onChange={(e) => setMergeIntoDeleteBranch(e.target.checked)}
            className="rounded border-border"
          />
          <span className="text-sm text-muted-foreground">
            Delete &ldquo;{branchInfo?.branch}&rdquo; after merge
          </span>
        </label>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction onClick={handleMergeIntoBase} className="bg-foreground text-background hover:bg-foreground/90">
            Merge into {baseBranch}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
    </>
  );
}
