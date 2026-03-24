import { useState, useEffect, useCallback, useRef } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Plus, Minus, GitCommit, ArrowUp, RefreshCw, Check } from "lucide-react";
import {
  getGitStatus,
  getGitDiff,
  getGitBranchInfo,
  gitStageFiles,
  gitUnstageFiles,
  gitCommitChanges,
  gitPushChanges,
} from "@/tauri/commands";
import type { WorkspaceSnapshot, GitFileStatus, GitBranchInfo } from "@/tauri/types";

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

function FileRow({
  file,
  staged,
  cwd,
  expanded,
  onToggleExpand,
  onRefresh,
}: {
  file: GitFileStatus;
  staged: boolean;
  cwd: string;
  expanded: boolean;
  onToggleExpand: () => void;
  onRefresh: () => void;
}) {
  const [diff, setDiff] = useState<string | null>(null);

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

  return (
    <div>
      <button
        className="group flex w-full items-center gap-1 rounded-sm px-1.5 py-0.5 text-left hover:bg-accent transition-colors"
        onClick={onToggleExpand}
      >
        <span
          className={`shrink-0 w-4 text-center text-[10px] font-bold ${STATUS_COLOR[file.status] ?? "text-muted-foreground"}`}
        >
          {STATUS_LABEL[file.status] ?? "?"}
        </span>
        <span className="flex-1 truncate text-xs text-foreground">
          {file.path.split("/").pop()}
        </span>
        <span className="hidden truncate text-[10px] text-muted-foreground group-hover:inline max-w-[80px]">
          {file.path}
        </span>
        <button
          className="shrink-0 opacity-0 group-hover:opacity-100 rounded-sm p-0.5 hover:bg-muted"
          onClick={handleStageToggle}
          title={staged ? "Unstage" : "Stage"}
        >
          {staged ? (
            <Minus className="h-3 w-3 text-muted-foreground" />
          ) : (
            <Plus className="h-3 w-3 text-muted-foreground" />
          )}
        </button>
      </button>
      {expanded && diff !== null && (
        <pre className="mx-1.5 mb-1 max-h-48 overflow-auto rounded-sm bg-card p-1.5 text-[10px] leading-normal font-mono text-muted-foreground">
          {diff || "(empty diff)"}
        </pre>
      )}
    </div>
  );
}

export function ChangesPanel({ workspace }: Props) {
  const cwd = workspace.cwd;
  const [files, setFiles] = useState<GitFileStatus[]>([]);
  const [branchInfo, setBranchInfo] = useState<GitBranchInfo | null>(null);
  const [expandedFile, setExpandedFile] = useState<string | null>(null);
  const [expandedStaged, setExpandedStaged] = useState(false);
  const [commitMsg, setCommitMsg] = useState("");
  const [busy, setBusy] = useState(false);
  const refreshRef = useRef<ReturnType<typeof setInterval> | null>(null);

  const refresh = useCallback(() => {
    if (!cwd) return;
    Promise.all([
      getGitStatus(cwd).catch(() => []),
      getGitBranchInfo(cwd).catch(() => null),
    ]).then(([status, info]) => {
      setFiles(status);
      if (info) setBranchInfo(info);
    });
  }, [cwd]);

  // Initial load + auto-refresh every 3s
  useEffect(() => {
    refresh();
    refreshRef.current = setInterval(refresh, 3000);
    return () => { if (refreshRef.current) clearInterval(refreshRef.current); };
  }, [refresh]);

  const staged = files.filter((f) => f.is_staged);
  const unstaged = files.filter((f) => f.is_unstaged);

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

  const handleStageAll = async () => {
    const paths = unstaged.map((f) => f.path);
    if (paths.length === 0) return;
    await gitStageFiles(cwd, paths).catch(console.error);
    refresh();
  };

  if (files.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center py-8 text-muted-foreground">
        <Check className="h-6 w-6 mb-2 opacity-40" />
        <p className="text-xs">No changes</p>
      </div>
    );
  }

  return (
    <div className="flex h-full flex-col">
      {/* Commit bar */}
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

      <ScrollArea className="flex-1">
        {/* Staged */}
        {staged.length > 0 && (
          <div className="p-1.5">
            <div className="flex items-center justify-between px-1.5 py-0.5">
              <span className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
                Staged
              </span>
              <span className="text-[10px] tabular-nums text-muted-foreground">
                {staged.length}
              </span>
            </div>
            {staged.map((f) => (
              <FileRow
                key={`staged-${f.path}`}
                file={f}
                staged
                cwd={cwd}
                expanded={expandedFile === f.path && expandedStaged}
                onToggleExpand={() => {
                  if (expandedFile === f.path && expandedStaged) {
                    setExpandedFile(null);
                  } else {
                    setExpandedFile(f.path);
                    setExpandedStaged(true);
                  }
                }}
                onRefresh={refresh}
              />
            ))}
          </div>
        )}

        {/* Unstaged */}
        {unstaged.length > 0 && (
          <div className="p-1.5">
            <div className="flex items-center justify-between px-1.5 py-0.5">
              <span className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
                Changes
              </span>
              <div className="flex items-center gap-1.5">
                <span className="text-[10px] tabular-nums text-muted-foreground">
                  {unstaged.length}
                </span>
                <button
                  className="text-[10px] text-primary hover:underline"
                  onClick={handleStageAll}
                >
                  Stage all
                </button>
              </div>
            </div>
            {unstaged.map((f) => (
              <FileRow
                key={`unstaged-${f.path}`}
                file={f}
                staged={false}
                cwd={cwd}
                expanded={expandedFile === f.path && !expandedStaged}
                onToggleExpand={() => {
                  if (expandedFile === f.path && !expandedStaged) {
                    setExpandedFile(null);
                  } else {
                    setExpandedFile(f.path);
                    setExpandedStaged(false);
                  }
                }}
                onRefresh={refresh}
              />
            ))}
          </div>
        )}
      </ScrollArea>
    </div>
  );
}
