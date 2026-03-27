import { useState, useEffect, useCallback, useRef, useMemo } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
} from "@/components/ui/dropdown-menu";
import {
  Tooltip,
  TooltipTrigger,
  TooltipContent,
  TooltipProvider,
} from "@/components/ui/tooltip";
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
  Folder,
  ChevronRight,
  File,
  Sparkles,
  AlertTriangle,
  GitMerge,
  XCircle,
  CheckCircle2,
  ChevronDown,
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
  createTab,
  activateTab,
  checkClaudeAvailable,
  getBaseBranchDiff,
  getDefaultBranch,
  listBranches,
} from "@/tauri/commands";
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
} from "@/tauri/types";

interface Props {
  workspace: WorkspaceSnapshot;
}

const STATUS_LABEL: Record<string, string> = {
  added: "A",
  modified: "M",
  deleted: "D",
  renamed: "R",
  untracked: "U",
  copied: "C",
  conflicted: "!",
};

const STATUS_COLOR: Record<string, string> = {
  added: "text-success",
  modified: "text-warning",
  deleted: "text-danger",
  renamed: "text-primary",
  untracked: "text-muted-foreground",
  copied: "text-muted-foreground",
  conflicted: "text-danger",
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
  indented,
}: {
  file: GitFileStatus;
  staged: boolean;
  cwd: string;
  expanded: boolean;
  onToggleExpand: () => void;
  onRefresh: () => void;
  onOpenDiff?: (filePath: string, staged: boolean) => void;
  activeDiffFile?: string | null;
  indented?: boolean;
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
            className={`group flex w-full items-center gap-1 rounded-sm py-0.5 text-left hover:bg-accent/50 transition-colors cursor-default ${indented ? "pl-5 pr-1" : "px-1"} ${activeDiffFile === file.path ? "bg-accent/30" : ""}`}
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
            <span
              className={`shrink-0 w-3.5 text-center text-[10px] font-bold leading-none ${STATUS_COLOR[file.status] ?? "text-muted-foreground"}`}
            >
              {STATUS_LABEL[file.status] ?? "?"}
            </span>
            <File className="h-3 w-3 shrink-0 text-muted-foreground/50" />
            <span className="flex-1 truncate text-xs text-foreground">{name}</span>
            {(file.additions > 0 || file.deletions > 0) && (
              <span className="flex items-center gap-0.5 shrink-0 text-[10px] tabular-nums">
                {file.additions > 0 && (
                  <span className="text-success">+{file.additions}</span>
                )}
                {file.deletions > 0 && (
                  <span className="text-danger">-{file.deletions}</span>
                )}
              </span>
            )}
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
          className="group flex w-full items-center gap-1 rounded-sm py-0.5 px-1 text-left hover:bg-danger/10 transition-colors cursor-default"
          onClick={() => onOpenDiff?.(file.path, false)}
        >
          <span className="shrink-0 w-3.5 text-center text-[10px] font-bold leading-none text-danger">
            !
          </span>
          <File className="h-3 w-3 shrink-0 text-danger/50" />
          <span className="flex-1 truncate text-xs text-foreground">{name}</span>
          {file.conflict_type && (
            <span className="shrink-0 text-[9px] text-danger/70 bg-danger/10 px-1 rounded">
              {CONFLICT_TYPE_LABEL[file.conflict_type] ?? file.conflict_type}
            </span>
          )}
          <div className="flex items-center shrink-0 gap-0.5 opacity-0 group-hover:opacity-100 transition-opacity">
            <Button
              variant="ghost"
              size="icon-xs"
              className="text-primary"
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
      <div className="py-1 px-1.5 space-y-1.5">
        <div className="flex items-center justify-between py-0.5">
          <span className="text-[10px] font-medium uppercase tracking-wider text-danger">
            Conflicts
          </span>
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

  return (
    <div className="py-1">
      <div className="flex items-center justify-between px-1.5 py-0.5">
        <span className="text-[10px] font-medium uppercase tracking-wider text-danger">
          Conflicts
        </span>
        <span className="text-[10px] tabular-nums text-danger">{files.length}</span>
      </div>
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
  const [collapsed, setCollapsed] = useState(false);

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
      <Button
        variant="ghost"
        className="w-full justify-start gap-1 px-1 py-0.5 h-auto text-left hover:bg-accent/30 rounded-sm"
        onClick={() => setCollapsed(!collapsed)}
      >
        <ChevronRight
          className={`h-3 w-3 shrink-0 text-muted-foreground transition-transform ${!collapsed ? "rotate-90" : ""}`}
        />
        <Folder className="h-3 w-3 shrink-0 text-muted-foreground/50" />
        <span className="truncate text-[10px] text-muted-foreground">{dir}</span>
      </Button>
      {!collapsed &&
        files.map((f) => (
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
            indented
          />
        ))}
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
}) {
  const groups = useMemo(() => groupByDirectory(files), [files]);

  return (
    <div className="py-1">
      <div className="flex items-center justify-between px-1.5 py-0.5">
        <span className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
          {label}
        </span>
        <div className="flex items-center gap-1.5">
          <span className="text-[10px] tabular-nums text-muted-foreground">{files.length}</span>
          <Button variant="ghost" size="xs" className="h-5 px-1.5 text-[10px]" onClick={onBulkAction}>
            {bulkLabel}
          </Button>
        </div>
      </div>
      {groups.map((group) => (
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
  const [busyAction, setBusyAction] = useState<"commit" | "push" | "pull" | "merge" | null>(null);
  const [gitError, setGitError] = useState<string | null>(null);
  const [commitsExpanded, setCommitsExpanded] = useState(false);
  const [claudeReady, setClaudeReady] = useState<boolean | null>(null);
  const refreshRef = useRef<ReturnType<typeof setInterval> | null>(null);

  // Against-base state
  const [baseBranch, setBaseBranch] = useState<string>("main");
  const [baseBranchFiles, setBaseBranchFiles] = useState<GitFileStatus[]>([]);
  const [baseBranchExpanded, setBaseBranchExpanded] = useState(true);
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
    <TooltipProvider>
      <div className="flex h-full flex-col">
        {/* Commit bar — pinned at top */}
        <div className="p-1.5 space-y-1 border-b border-border">
          <div className="flex gap-1 items-center">
            <Input
              placeholder="Commit message"
              value={commitMsg}
              onChange={(e) => setCommitMsg(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && !e.shiftKey && handleCommit()}
              disabled={isGenerating}
              className="h-7 text-xs flex-1"
            />
            {aiEnabled && (
              <Tooltip>
                <TooltipTrigger asChild>
                  <span className="shrink-0" tabIndex={0}>
                    <Button
                      variant="ghost"
                      size="icon-xs"
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
          <div className="flex gap-1">
            <Button
              size="xs"
              className="flex-1 text-xs h-6"
              disabled={!commitMsg.trim() || staged.length === 0 || busy || conflicted.length > 0}
              onClick={handleCommit}
            >
              {busyAction === "commit"
                ? <Loader2 className="h-3 w-3 animate-spin mr-1" />
                : <GitCommit className="h-3 w-3 mr-1" />}
              Commit
            </Button>
            {branchInfo && branchInfo.branch && (() => {
              const pushDisabled = busy || (branchInfo.has_upstream && branchInfo.ahead === 0);
              return (
                <Tooltip>
                  <TooltipTrigger asChild>
                    <span>
                      <Button
                        size="xs"
                        variant="secondary"
                        className={`text-xs h-6${pushDisabled ? " opacity-50" : ""}`}
                        disabled={pushDisabled}
                        onClick={handlePush}
                      >
                        {busyAction === "push"
                          ? <Loader2 className="h-3 w-3 animate-spin mr-1" />
                          : <ArrowUp className="h-3 w-3 mr-1" />}
                        {!branchInfo.has_upstream
                          ? "Publish"
                          : branchInfo.ahead > 0
                            ? `Push ${branchInfo.ahead}`
                            : "Push"}
                      </Button>
                    </span>
                  </TooltipTrigger>
                  <TooltipContent side="bottom" className="text-xs">
                    {branchInfo.has_upstream && branchInfo.ahead === 0
                      ? "Nothing to push"
                      : !branchInfo.has_upstream
                        ? "Publish branch to remote"
                        : `Push ${branchInfo.ahead} commit${branchInfo.ahead !== 1 ? "s" : ""} to remote`}
                  </TooltipContent>
                </Tooltip>
              );
            })()}
            {branchInfo && branchInfo.has_upstream && branchInfo.behind > 0 && (
              <Button
                size="xs"
                variant="secondary"
                className="text-xs h-6"
                disabled={busy}
                onClick={handlePull}
              >
                {busyAction === "pull"
                  ? <Loader2 className="h-3 w-3 animate-spin mr-1" />
                  : <ArrowDown className="h-3 w-3 mr-1" />}
                Pull {branchInfo.behind}
              </Button>
            )}
            <Button
              size="xs"
              variant="ghost"
              className="h-6 w-6 p-0"
              onClick={refresh}
              title="Refresh"
            >
              <RefreshCw className="h-3 w-3" />
            </Button>
          </div>
          {gitError && (
            <p className="text-[10px] text-destructive break-words px-0.5">{gitError}</p>
          )}
        </div>

        {/* Merge conflict banner */}
        {isMerging && (
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
        )}

        {/* No changes message — outside ScrollArea to avoid scrollbar shift */}
        {files.length === 0 && !isMerging && (
          <div className="flex flex-col items-center justify-center min-h-[120px] text-muted-foreground">
            <Check className="h-5 w-5 mb-1.5 opacity-40" />
            <p className="text-xs">No changes</p>
          </div>
        )}

        {/* File list */}
        <ScrollArea className="flex-1">
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
              />
            )}

            {unstaged.length > 0 && (
              <FileSection
                label="Changes"
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
              />
            )}

            {/* Against Base — files changed vs base branch */}
            {baseBranchFiles.length > 0 && (
              <div className="py-1">
                <div className="flex items-center justify-between px-1.5 py-0.5">
                  <div className="flex items-center gap-1">
                    <Button
                      variant="ghost"
                      size="icon-xs"
                      className="hover:bg-accent/30 -ml-0.5"
                      onClick={() => setBaseBranchExpanded(!baseBranchExpanded)}
                    >
                      <ChevronRight
                        className={`h-3 w-3 shrink-0 text-muted-foreground transition-transform ${baseBranchExpanded ? "rotate-90" : ""}`}
                      />
                    </Button>
                    <span className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
                      Against
                    </span>
                    {remoteBranches.length > 1 ? (
                      <DropdownMenu>
                        <DropdownMenuTrigger asChild>
                          <Button
                            variant="ghost"
                            size="xs"
                            className="gap-0.5 px-1 py-0.5 text-[10px] font-medium text-muted-foreground hover:bg-accent/50 hover:text-foreground"
                          >
                            {baseBranch}
                            <ChevronDown className="h-2.5 w-2.5 opacity-60" />
                          </Button>
                        </DropdownMenuTrigger>
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
                      <span className="text-[10px] font-medium text-muted-foreground">
                        {baseBranch}
                      </span>
                    )}
                  </div>
                  <div className="flex items-center gap-1">
                    <span className="text-[10px] tabular-nums text-muted-foreground">
                      {baseBranchFiles.length}
                    </span>
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <Button
                          variant="ghost"
                          size="icon-xs"
                          className="h-4 w-4 hover:bg-accent/50"
                          onClick={handleMergeBranch}
                          disabled={busy || isMerging}
                        >
                          {busyAction === "merge" ? (
                            <Loader2 className="h-3 w-3 animate-spin" />
                          ) : (
                            <GitMerge className="h-3 w-3" />
                          )}
                        </Button>
                      </TooltipTrigger>
                      <TooltipContent side="left">
                        <p>Merge {baseBranch} into current branch — update your branch with latest changes</p>
                      </TooltipContent>
                    </Tooltip>
                  </div>
                </div>
                {mergeSuccess && (
                  <div className="px-2 py-1">
                    <p className="text-[10px] text-success flex items-center gap-1">
                      <CheckCircle2 className="h-3 w-3 shrink-0" />
                      {mergeSuccess}
                    </p>
                  </div>
                )}
                {baseBranchExpanded && (
                  <div>
                    {groupByDirectory(baseBranchFiles).map((group) => (
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

            {/* Recent Commits — collapsed by default */}
            {commits.length > 0 && (
              <div className="py-1">
                <Button
                  variant="ghost"
                  className="w-full justify-between px-1.5 py-0.5 h-auto hover:bg-accent/30 rounded-sm"
                  onClick={() => setCommitsExpanded(!commitsExpanded)}
                >
                  <div className="flex items-center gap-1">
                    <ChevronRight
                      className={`h-3 w-3 shrink-0 text-muted-foreground transition-transform ${commitsExpanded ? "rotate-90" : ""}`}
                    />
                    <span className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
                      Recent Commits
                    </span>
                  </div>
                  <span className="text-[10px] tabular-nums text-muted-foreground">
                    {commits.length}
                  </span>
                </Button>
                {commitsExpanded &&
                  commits.map((commit, idx) => {
                    const prevCommit = idx > 0 ? commits[idx - 1] : null;
                    const showSeparator = prevCommit && !prevCommit.is_pushed && commit.is_pushed;
                    return (
                      <div key={commit.hash}>
                        {showSeparator && (
                          <div className="text-[9px] text-muted-foreground/50 text-center py-0.5">
                            — pushed —
                          </div>
                        )}
                        <Tooltip>
                          <TooltipTrigger asChild>
                            <div className="flex items-start gap-1.5 rounded-sm px-1.5 py-1 hover:bg-accent/50 transition-colors">
                              {!commit.is_pushed && (
                                <ArrowUp className="h-2.5 w-2.5 shrink-0 text-warning mt-0.5" />
                              )}
                              <span className="shrink-0 text-[10px] font-mono text-primary">
                                {commit.short_hash}
                              </span>
                              <div className="flex-1 min-w-0">
                                <p className="truncate text-xs text-foreground">{commit.message}</p>
                                <p className="text-[10px] text-muted-foreground">
                                  {commit.author} · {commit.time_ago}
                                </p>
                              </div>
                            </div>
                          </TooltipTrigger>
                          <TooltipContent side="left" className="text-xs max-w-64">
                            {commit.message}
                          </TooltipContent>
                        </Tooltip>
                      </div>
                    );
                  })}
              </div>
            )}
          </div>
        </ScrollArea>
      </div>
    </TooltipProvider>
  );
}
