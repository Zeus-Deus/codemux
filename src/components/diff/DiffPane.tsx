import { useState, useEffect, useCallback, useRef, useMemo } from "react";
import { GitCompare } from "lucide-react";
import { getGitDiff, getGitStatus, getBaseBranchDiff, getBaseBranchFileDiff } from "@/tauri/commands";
import { useDiffStore } from "@/stores/diff-store";
import { parseDiff } from "@/lib/diff-parser";
import { DiffToolbar } from "./DiffToolbar";
import { DiffUnifiedView, type DiffViewHandle } from "./DiffUnifiedView";
import { DiffSplitView } from "./DiffSplitView";
import type { DiffLine } from "@/lib/diff-parser";
import type { WorkspaceSnapshot, GitFileStatus } from "@/tauri/types";

interface Props {
  tabId: string;
  workspace: WorkspaceSnapshot;
}

export function DiffPane({ tabId, workspace }: Props) {
  const cwd = workspace.worktree_path ?? workspace.cwd;
  const tab = useDiffStore((s) => s.getTab(tabId));
  const initTab = useDiffStore((s) => s.initTab);
  const setFile = useDiffStore((s) => s.setFile);
  const setFileIndex = useDiffStore((s) => s.setFileIndex);

  const [lines, setLines] = useState<DiffLine[]>([]);
  const [files, setFiles] = useState<GitFileStatus[]>([]);
  const [baseFiles, setBaseFiles] = useState<GitFileStatus[]>([]);
  const [loading, setLoading] = useState(false);
  const viewRef = useRef<DiffViewHandle>(null);

  // Initialize tab state if not exists
  useEffect(() => {
    if (!tab) initTab(tabId);
  }, [tab, tabId, initTab]);

  // Fetch file list on mount and periodically
  useEffect(() => {
    const fetchFiles = () => {
      getGitStatus(cwd)
        .then(setFiles)
        .catch(console.error);
    };
    fetchFiles();
    const interval = setInterval(fetchFiles, 5000);
    return () => clearInterval(interval);
  }, [cwd]);

  // Fetch against-base file list when in that mode
  useEffect(() => {
    if (tab?.section !== "against_base" || !tab?.baseBranch) {
      setBaseFiles([]);
      return;
    }
    getBaseBranchDiff(cwd, tab.baseBranch)
      .then((result) => setBaseFiles(result.files))
      .catch(() => setBaseFiles([]));
  }, [cwd, tab?.section, tab?.baseBranch]);

  // Filter files based on section
  const filteredFiles = useMemo(() => {
    if (!tab) return files;
    switch (tab.section) {
      case "staged":
        return files.filter((f) => f.is_staged);
      case "unstaged":
        return files.filter((f) => f.is_unstaged);
      case "against_base":
        return baseFiles;
      default:
        return files;
    }
  }, [files, baseFiles, tab?.section]);

  // Fetch diff when file changes
  useEffect(() => {
    if (!tab?.filePath) {
      setLines([]);
      return;
    }
    setLoading(true);
    const fetchDiff =
      tab.section === "against_base" && tab.baseBranch
        ? getBaseBranchFileDiff(cwd, tab.baseBranch, tab.filePath)
        : getGitDiff(cwd, tab.filePath, tab.staged);
    fetchDiff
      .then((raw) => setLines(parseDiff(raw)))
      .catch(() => setLines([]))
      .finally(() => setLoading(false));
  }, [cwd, tab?.filePath, tab?.staged, tab?.section, tab?.baseBranch]);

  // Sync fileIndex when filePath changes
  useEffect(() => {
    if (!tab?.filePath) return;
    const idx = filteredFiles.findIndex((f) => f.path === tab.filePath);
    if (idx >= 0 && idx !== tab.fileIndex) {
      setFileIndex(tabId, idx);
    }
  }, [tab?.filePath, filteredFiles, tabId, setFileIndex, tab?.fileIndex]);

  const handlePrevFile = useCallback(() => {
    if (!tab || filteredFiles.length === 0) return;
    const newIdx =
      (tab.fileIndex - 1 + filteredFiles.length) % filteredFiles.length;
    const file = filteredFiles[newIdx];
    const staged = tab.section === "staged" ? true : tab.section === "unstaged" ? false : file.is_staged;
    setFile(tabId, file.path, staged);
    setFileIndex(tabId, newIdx);
  }, [tab, filteredFiles, tabId, setFile, setFileIndex]);

  const handleNextFile = useCallback(() => {
    if (!tab || filteredFiles.length === 0) return;
    const newIdx = (tab.fileIndex + 1) % filteredFiles.length;
    const file = filteredFiles[newIdx];
    const staged = tab.section === "staged" ? true : tab.section === "unstaged" ? false : file.is_staged;
    setFile(tabId, file.path, staged);
    setFileIndex(tabId, newIdx);
  }, [tab, filteredFiles, tabId, setFile, setFileIndex]);

  const handlePrevHunk = useCallback(() => {
    viewRef.current?.scrollToHunk(-1);
  }, []);

  const handleNextHunk = useCallback(() => {
    viewRef.current?.scrollToHunk(1);
  }, []);

  if (!tab) return null;

  // Empty state
  if (!tab.filePath) {
    return (
      <div className="flex h-full w-full flex-col bg-card">
        <DiffToolbar
          tabId={tabId}
          workspaceId={workspace.workspace_id}
          tab={tab}
          fileCount={filteredFiles.length}
          fileIndex={tab.fileIndex}
          onPrevHunk={handlePrevHunk}
          onNextHunk={handleNextHunk}
          onPrevFile={handlePrevFile}
          onNextFile={handleNextFile}
        />
        <div className="flex flex-1 flex-col items-center justify-center gap-2 text-muted-foreground">
          <GitCompare className="h-8 w-8 opacity-30" />
          <p className="text-xs">Select a file to view changes</p>
          {filteredFiles.length > 0 && (
            <p className="text-[10px] text-muted-foreground/60">
              {filteredFiles.length} file{filteredFiles.length !== 1 ? "s" : ""}{" "}
              with changes
            </p>
          )}
        </div>
      </div>
    );
  }

  return (
    <div className="flex h-full w-full flex-col bg-card overflow-hidden">
      <DiffToolbar
        tabId={tabId}
        workspaceId={workspace.workspace_id}
        tab={tab}
        fileCount={filteredFiles.length}
        fileIndex={tab.fileIndex}
        onPrevHunk={handlePrevHunk}
        onNextHunk={handleNextHunk}
        onPrevFile={handlePrevFile}
        onNextFile={handleNextFile}
      />
      {loading ? (
        <div className="flex flex-1 items-center justify-center text-muted-foreground">
          <p className="text-xs">Loading diff...</p>
        </div>
      ) : lines.length === 0 ? (
        <div className="flex flex-1 flex-col items-center justify-center gap-2 text-muted-foreground">
          <GitCompare className="h-6 w-6 opacity-30" />
          <p className="text-xs">No changes in this file</p>
        </div>
      ) : tab.layout === "split" ? (
        <DiffSplitView ref={viewRef} lines={lines} />
      ) : (
        <DiffUnifiedView ref={viewRef} lines={lines} />
      )}
    </div>
  );
}
