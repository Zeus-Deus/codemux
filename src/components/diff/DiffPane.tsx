import { useState, useEffect, useCallback, useRef, useMemo } from "react";
import {
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  ChevronUp,
  GitCompare,
} from "lucide-react";
import { getGitDiff, getGitStatus, getBaseBranchDiff, getBaseBranchFileDiff } from "@/tauri/commands";
import { useDiffStore } from "@/stores/diff-store";
import { parseDiff } from "@/lib/diff-parser";
import { keepIfUnchanged } from "@/lib/poll-equality";
import { Button } from "@/components/ui/button";
import { DiffToolbar } from "./DiffToolbar";
import { DiffUnifiedView, type DiffViewHandle } from "./DiffUnifiedView";
import { DiffSplitView } from "./DiffSplitView";
import type { DiffLine } from "@/lib/diff-parser";
import type { WorkspaceSnapshot, GitFileStatus } from "@/tauri/types";
import { markPaneReady } from "@/lib/perf/interaction-trace";
import { cn } from "@/lib/utils";

interface Props {
  tabId: string;
  workspace: WorkspaceSnapshot;
  /** Hosted in the right-panel deck. The dense toolbar is replaced by a
   *  one-row file header (path, counts, hunk and file navigation); the
   *  deck's shared pane bar carries the layout toggle and "open in a
   *  tab". Focus mode stays on the main-area diff tab. */
  embedded?: boolean;
  /** Soft-wrap long lines. Unified layout only: split columns are
   *  separate scrollers whose rows have to stay level. */
  wrap?: boolean;
  /** Escape from the focused diff. The deck uses it to return to the
   *  Changes list the file was picked from. */
  onBack?: () => void;
}

/** The diff on screen and which read it came from. Kept together so a
 *  file switch can leave the previous diff up until the next one lands. */
interface LoadedDiff {
  /** The file, side (staged or not) and base the diff was read for. A
   *  new diff with the same source is a live refresh; anything else is a
   *  different diff. */
  source: string;
  path: string;
  raw: string;
  lines: DiffLine[];
}

/** How often the open file's status and diff are read again. */
const POLL_MS = 5000;

function splitPath(path: string): { dir: string; name: string } {
  const i = path.lastIndexOf("/");
  return i >= 0
    ? { dir: path.slice(0, i + 1), name: path.slice(i + 1) }
    : { dir: "", name: path };
}

export function DiffPane({
  tabId,
  workspace,
  embedded = false,
  wrap = false,
  onBack,
}: Props) {
  const cwd = workspace.worktree_path ?? workspace.cwd;
  const tab = useDiffStore((s) => s.getTab(tabId));
  const initTab = useDiffStore((s) => s.initTab);
  const setFile = useDiffStore((s) => s.setFile);
  const setFileIndex = useDiffStore((s) => s.setFileIndex);

  const [diff, setDiff] = useState<LoadedDiff | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [retryKey, setRetryKey] = useState(0);
  // Bumped when the open file's diff changes under you; keys the
  // "Updated" note so each refresh shows it again.
  const [updatedTick, setUpdatedTick] = useState(0);
  // Bumped by the poll: re-reads the open diff quietly.
  const [pollTick, setPollTick] = useState(0);
  const [files, setFiles] = useState<GitFileStatus[]>([]);
  const [baseFiles, setBaseFiles] = useState<GitFileStatus[]>([]);
  const viewRef = useRef<DiffViewHandle>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const prevDiffRef = useRef<LoadedDiff | null>(null);
  // The last read that finished. Reading the same source again is a
  // quiet re-read: no loading bar, and a failure leaves the view alone.
  const lastReadRef = useRef<string | null>(null);

  // Initialize tab state if not exists
  useEffect(() => {
    if (!tab) initTab(tabId);
  }, [tab, tabId, initTab]);

  // Opening a file from the Changes list unmounts the row that had focus.
  // Take it, so the diff's keys work straight away — but never from
  // somewhere the user is typing.
  useEffect(() => {
    if (!embedded) return;
    const active = document.activeElement;
    if (!active || active === document.body) {
      rootRef.current?.focus({ preventScroll: true });
    }
  }, [embedded]);

  // Fetch file list on mount and periodically
  useEffect(() => {
    const fetchFiles = () => {
      // Keep the previous array when the working tree hasn't moved: this
      // runs every 5 s and an unchanged status must not re-render the file
      // list and the diff view under it.
      getGitStatus(cwd)
        .then((next) => setFiles((prev) => keepIfUnchanged(prev, next)))
        .catch(console.error);
    };
    fetchFiles();
    const interval = setInterval(() => {
      fetchFiles();
      // The status row can stay the same while an agent rewrites a line
      // that was already changed (+1 −1 either way), so the open diff is
      // read again on every tick. An unchanged read renders nothing.
      setPollTick((n) => n + 1);
    }, POLL_MS);
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

  const againstBase = tab?.section === "against_base" && !!tab?.baseBranch;
  const source = tab?.filePath
    ? `${cwd}\0${tab.filePath}\0${againstBase ? `base:${tab.baseBranch}` : tab.staged ? "staged" : "unstaged"}`
    : null;

  // Read the diff when its source changes, on Retry, and quietly on
  // every poll tick.
  useEffect(() => {
    if (!tab?.filePath || !source) {
      lastReadRef.current = null;
      setDiff(null);
      setError(null);
      if (tab) markPaneReady("diff", { target: workspace.workspace_id });
      return;
    }
    const path = tab.filePath;
    const readKey = `${source}\0${retryKey}`;
    const quiet = lastReadRef.current === readKey;
    let cancelled = false;
    if (!quiet) {
      setLoading(true);
      // An error belongs to the read that failed, not to the next file.
      setError(null);
    }
    const fetchDiff =
      againstBase && tab.baseBranch
        ? getBaseBranchFileDiff(cwd, tab.baseBranch, path)
        : getGitDiff(cwd, path, tab.staged);
    fetchDiff
      .then((raw) => {
        if (cancelled) return;
        setError(null);
        // An unchanged re-read keeps the same object, so nothing renders.
        setDiff((prev) =>
          prev?.source === source && prev.raw === raw
            ? prev
            : { source, path, raw, lines: parseDiff(raw) },
        );
      })
      .catch((err: unknown) => {
        // A failed quiet re-read leaves the diff that is up alone; the
        // next tick tries again.
        if (!cancelled && !quiet) setError(String(err));
      })
      .finally(() => {
        if (!cancelled) {
          lastReadRef.current = readKey;
          setLoading(false);
        }
        if (!quiet) markPaneReady("diff", { target: workspace.workspace_id });
      });
    return () => {
      cancelled = true;
    };
    // `source` folds in every tab field that picks which diff this is.
  }, [source, workspace.workspace_id, retryKey, pollTick]);

  // A new diff from the same source is a live refresh: say so. Stepping
  // to another file, or to the other side of the same file, is not.
  useEffect(() => {
    const prev = prevDiffRef.current;
    prevDiffRef.current = diff;
    if (prev && diff && prev !== diff && prev.source === diff.source) {
      setUpdatedTick((n) => n + 1);
    }
  }, [diff]);

  // The "Updated" note is a beat, not a state: it clears itself.
  useEffect(() => {
    if (updatedTick === 0) return;
    const timer = setTimeout(() => setUpdatedTick(0), 1500);
    return () => clearTimeout(timer);
  }, [updatedTick]);

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

  // j/k or ]/[ step through hunks, J/K through files, Escape goes back.
  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      const target = e.target as HTMLElement;
      if (target.closest("input, textarea, select, [contenteditable='true']")) return;
      // Shift picks files, not the letter's case: with Caps Lock on, `j`
      // arrives as "J" and still means the next change.
      const key = e.key.length === 1 ? e.key.toLowerCase() : e.key;
      const action =
        key === "j"
          ? e.shiftKey
            ? handleNextFile
            : handleNextHunk
          : key === "k"
            ? e.shiftKey
              ? handlePrevFile
              : handlePrevHunk
            : key === "]"
              ? handleNextHunk
              : key === "["
                ? handlePrevHunk
                : key === "Escape"
                  ? onBack
                  : undefined;
      if (!action) return;
      e.preventDefault();
      e.stopPropagation();
      action();
    },
    [handleNextHunk, handlePrevHunk, handleNextFile, handlePrevFile, onBack],
  );

  if (!tab) return null;

  const toolbar = !embedded && (
    <DiffToolbar
      tabId={tabId}
      workspaceId={workspace.workspace_id}
      tabs={workspace.tabs}
      tab={tab}
      fileCount={filteredFiles.length}
      fileIndex={tab.fileIndex}
      onPrevHunk={handlePrevHunk}
      onNextHunk={handleNextHunk}
      onPrevFile={handlePrevFile}
      onNextFile={handleNextFile}
    />
  );

  // Empty state
  if (!tab.filePath) {
    return (
      <div className="flex h-full w-full flex-col bg-card">
        {toolbar}
        <div className="flex flex-1 flex-col items-center justify-center gap-2 text-muted-foreground">
          <GitCompare className="size-8 opacity-30" />
          <p className="text-label">Select a file to view changes</p>
          {filteredFiles.length > 0 && (
            <p className="text-caption text-muted-foreground/60">
              {filteredFiles.length} file{filteredFiles.length !== 1 ? "s" : ""}{" "}
              with changes
            </p>
          )}
        </div>
      </div>
    );
  }

  // `diff` can still be the previous file's while the next one loads.
  const current = diff?.source === source;
  const entry = filteredFiles.find((f) => f.path === tab.filePath);
  const { dir, name } = splitPath(tab.filePath);
  const fileCount = filteredFiles.length;

  let body: React.ReactNode;
  if (error) {
    body = (
      <div
        role="alert"
        className="flex flex-1 flex-col items-center justify-center gap-2 px-4 text-center text-muted-foreground"
      >
        <p className="text-label">Couldn&apos;t read the diff — {error}</p>
        <Button
          size="xs"
          variant="ghost"
          className="bg-surface-2 text-foreground hover:bg-surface-3"
          onClick={() => setRetryKey((n) => n + 1)}
          disabled={loading}
        >
          Retry
        </Button>
      </div>
    );
  } else if (!diff) {
    body = (
      <div className="flex flex-1 items-center justify-center text-muted-foreground">
        <p className="text-label">Loading diff…</p>
      </div>
    );
  } else if (current && diff.lines.length === 0) {
    body = (
      <div className="flex flex-1 flex-col items-center justify-center gap-2 text-muted-foreground">
        <GitCompare className="size-6 opacity-30" />
        <p className="text-label">No changes in this file</p>
      </div>
    );
  } else {
    // Keyed by the file the lines belong to: a file switch fades the new
    // diff in, while a refresh of the same file keeps the view mounted and
    // its scroll position with it.
    body = (
      <div
        key={diff.source}
        className={cn(
          "flex min-h-0 flex-1 flex-col transition-opacity duration-150 motion-safe:animate-in motion-safe:fade-in-0",
          !current && "opacity-50",
        )}
      >
        {tab.layout === "split" ? (
          <DiffSplitView ref={viewRef} lines={diff.lines} />
        ) : (
          <DiffUnifiedView ref={viewRef} lines={diff.lines} wrap={wrap} />
        )}
      </div>
    );
  }

  return (
    <div
      ref={rootRef}
      // The pane takes focus as a whole so its keys work after a click
      // anywhere in it; the file header says which pane that is.
      tabIndex={-1}
      onKeyDown={handleKeyDown}
      data-testid="diff-pane"
      className="flex h-full w-full flex-col overflow-hidden bg-card outline-none"
    >
      {toolbar}
      {embedded && (
        <div
          data-testid="diff-file-header"
          className="flex h-7 shrink-0 items-center gap-1 border-b border-hairline pr-1 pl-2.5"
        >
          {/* The directory truncates first, and from its start: the file
              name and the folders nearest it are what tell two files apart
              in a narrow panel. The inner ltr span keeps the trailing slash
              where it belongs inside the rtl truncation. */}
          <span className="flex min-w-0 flex-1 font-mono text-label" title={tab.filePath}>
            <span dir="rtl" className="truncate text-muted-foreground/70">
              <span dir="ltr">{dir}</span>
            </span>
            <span className="shrink-0 text-foreground">{name}</span>
          </span>
          {entry && (entry.additions > 0 || entry.deletions > 0) && (
            <span className="flex shrink-0 gap-1 font-mono text-caption tabular-nums">
              {entry.additions > 0 && <span className="text-success">+{entry.additions}</span>}
              {entry.deletions > 0 && <span className="text-danger">−{entry.deletions}</span>}
            </span>
          )}
          <Button
            size="icon-xs"
            variant="ghost"
            onClick={handlePrevHunk}
            aria-label="Previous change"
            title="Previous change (k)"
          >
            <ChevronUp className="size-3" />
          </Button>
          <Button
            size="icon-xs"
            variant="ghost"
            onClick={handleNextHunk}
            aria-label="Next change"
            title="Next change (j)"
          >
            <ChevronDown className="size-3" />
          </Button>
          <Button
            size="icon-xs"
            variant="ghost"
            onClick={handlePrevFile}
            disabled={fileCount <= 1}
            aria-label="Previous file"
            title="Previous file (Shift+K)"
          >
            <ChevronLeft className="size-3" />
          </Button>
          <span
            data-testid="diff-file-position"
            className="min-w-7 text-center text-caption tabular-nums text-muted-foreground"
          >
            {entry ? `${tab.fileIndex + 1}/${fileCount}` : `–/${fileCount}`}
          </span>
          <Button
            size="icon-xs"
            variant="ghost"
            onClick={handleNextFile}
            disabled={fileCount <= 1}
            aria-label="Next file"
            title="Next file (Shift+J)"
          >
            <ChevronRight className="size-3" />
          </Button>
        </div>
      )}
      <div className="relative flex min-h-0 flex-1 flex-col">
        {/* A re-read after the first load keeps the diff on screen and
            marks the wait here, instead of blanking the view. */}
        {loading && diff && (
          <div
            data-testid="diff-loading-bar"
            className="pointer-events-none absolute inset-x-0 top-0 z-10 h-0.5 bg-primary/60 motion-safe:animate-pulse"
          />
        )}
        {/* Always mounted, so screen readers announce the text when it
            changes; a live region that arrives already filled is often
            skipped. */}
        <span
          role="status"
          data-testid="diff-updated"
          className="pointer-events-none absolute top-1.5 right-3 z-10"
        >
          {updatedTick > 0 && !error && (
            <span
              key={updatedTick}
              className="block rounded-sm bg-surface-3 px-1.5 text-caption text-muted-foreground motion-safe:animate-in motion-safe:fade-in-0 motion-safe:duration-150"
            >
              Updated
            </span>
          )}
        </span>
        {body}
      </div>
    </div>
  );
}
