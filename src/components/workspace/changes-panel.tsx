import { useState, useEffect, useCallback, useRef, useMemo } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
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
  Check,
  Trash2,
  Folder,
  ChevronRight,
  File,
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
  createTab,
  activateTab,
} from "@/tauri/commands";
import { useDiffStore } from "@/stores/diff-store";
import type {
  WorkspaceSnapshot,
  GitFileStatus,
  GitBranchInfo,
  GitLogEntry,
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
};

const STATUS_COLOR: Record<string, string> = {
  added: "text-success",
  modified: "text-warning",
  deleted: "text-danger",
  renamed: "text-primary",
  untracked: "text-muted-foreground",
  copied: "text-muted-foreground",
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
      <button
        className="flex w-full items-center gap-1 px-1 py-0.5 text-left hover:bg-accent/30 transition-colors rounded-sm"
        onClick={() => setCollapsed(!collapsed)}
      >
        <ChevronRight
          className={`h-3 w-3 shrink-0 text-muted-foreground transition-transform ${!collapsed ? "rotate-90" : ""}`}
        />
        <Folder className="h-3 w-3 shrink-0 text-muted-foreground/50" />
        <span className="truncate text-[10px] text-muted-foreground">{dir}</span>
      </button>
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
  console.log("[ChangesPanel] cwd:", cwd, "worktree_path:", workspace.worktree_path, "workspace.cwd:", workspace.cwd);
  const [files, setFiles] = useState<GitFileStatus[]>([]);
  const [branchInfo, setBranchInfo] = useState<GitBranchInfo | null>(null);
  const [commits, setCommits] = useState<GitLogEntry[]>([]);
  const [expandedFile, setExpandedFile] = useState<string | null>(null);
  const [expandedStaged, setExpandedStaged] = useState(false);
  const [commitMsg, setCommitMsg] = useState("");
  const [busy, setBusy] = useState(false);
  const [commitsExpanded, setCommitsExpanded] = useState(false);
  const refreshRef = useRef<ReturnType<typeof setInterval> | null>(null);

  const refresh = useCallback(() => {
    if (!cwd) return;
    Promise.all([
      getGitStatus(cwd).catch((e) => { console.error("[ChangesPanel] git status failed:", e); return [] as GitFileStatus[]; }),
      getGitBranchInfo(cwd).catch((e) => { console.error("[ChangesPanel] branch info failed:", e); return null; }),
      gitLogEntries(cwd, 10).catch((e) => { console.error("[ChangesPanel] git log failed:", e); return [] as GitLogEntry[]; }),
    ]).then(([status, info, log]) => {
      console.log("[ChangesPanel] status:", status.length, "files, branch:", info?.branch, "commits:", log.length);
      setFiles(status);
      if (info) setBranchInfo(info);
      setCommits(log);
    });
  }, [cwd]);

  useEffect(() => {
    refresh();
    refreshRef.current = setInterval(refresh, 3000);
    return () => {
      if (refreshRef.current) clearInterval(refreshRef.current);
    };
  }, [refresh]);

  const staged = useMemo(() => files.filter((f) => f.is_staged), [files]);
  const unstaged = useMemo(() => files.filter((f) => f.is_unstaged), [files]);

  const handleCommit = async () => {
    if (!commitMsg.trim() || staged.length === 0 || busy) return;
    setBusy(true);
    try {
      await gitCommitChanges(cwd, commitMsg.trim());
      setCommitMsg("");
      setExpandedFile(null);
      refresh();
    } catch (err) {
      console.error("Commit failed:", err);
    } finally {
      setBusy(false);
    }
  };

  const handlePush = async () => {
    if (busy) return;
    setBusy(true);
    try {
      await gitPushChanges(cwd);
      refresh();
    } catch (err) {
      console.error("Push failed:", err);
    } finally {
      setBusy(false);
    }
  };

  const handlePull = async () => {
    if (busy) return;
    setBusy(true);
    try {
      await gitPullChanges(cwd);
      refresh();
    } catch (err) {
      console.error("Pull failed:", err);
    } finally {
      setBusy(false);
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

  return (
    <TooltipProvider>
      <div className="flex h-full flex-col">
        {/* Commit bar — pinned at top */}
        <div className="p-1.5 space-y-1 border-b border-border">
          <Input
            placeholder="Commit message"
            value={commitMsg}
            onChange={(e) => setCommitMsg(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && !e.shiftKey && handleCommit()}
            className="h-7 text-xs"
          />
          <div className="flex gap-1">
            <Button
              size="xs"
              className="flex-1 text-xs h-6"
              disabled={!commitMsg.trim() || staged.length === 0 || busy}
              onClick={handleCommit}
            >
              <GitCommit className="h-3 w-3 mr-1" />
              Commit
            </Button>
            {branchInfo && branchInfo.behind > 0 && (
              <Button
                size="xs"
                variant="secondary"
                className="text-xs h-6"
                disabled={busy}
                onClick={handlePull}
              >
                <ArrowDown className="h-3 w-3 mr-1" />
                Pull {branchInfo.behind}
              </Button>
            )}
            {branchInfo && branchInfo.ahead > 0 && (
              <Button
                size="xs"
                variant="secondary"
                className="text-xs h-6"
                disabled={busy}
                onClick={handlePush}
              >
                <ArrowUp className="h-3 w-3 mr-1" />
                Push {branchInfo.ahead}
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
        </div>

        {/* No changes message — outside ScrollArea to avoid scrollbar shift */}
        {files.length === 0 && (
          <div className="flex flex-col items-center justify-center min-h-[120px] text-muted-foreground">
            <Check className="h-5 w-5 mb-1.5 opacity-40" />
            <p className="text-xs">No changes</p>
          </div>
        )}

        {/* File list */}
        <ScrollArea className="flex-1">
          <div className="px-1">
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

            {/* Recent Commits — collapsed by default */}
            {commits.length > 0 && (
              <div className="py-1">
                <button
                  className="flex w-full items-center justify-between px-1.5 py-0.5 hover:bg-accent/30 rounded-sm transition-colors"
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
                </button>
                {commitsExpanded &&
                  commits.map((commit) => (
                    <Tooltip key={commit.hash}>
                      <TooltipTrigger asChild>
                        <div className="flex items-start gap-1.5 rounded-sm px-1.5 py-1 hover:bg-accent/50 transition-colors">
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
                  ))}
              </div>
            )}
          </div>
        </ScrollArea>
      </div>
    </TooltipProvider>
  );
}
