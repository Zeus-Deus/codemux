/**
 * The workspace file tree.
 *
 * Deliberately quiet: 22px rows, an 11px indent per depth level, and text
 * that fades from folders to files so the *shape* of the tree is what you
 * read first. It carries no per-file size column — the right-aligned
 * `290b / 17K / 4K` numbers were the noisiest thing on the surface and
 * answered a question nobody was asking of a navigation list.
 *
 * It has no header either. Refresh and show-hidden-files live in the
 * right-panel deck's single row of chrome (`right-panel/pane-actions.tsx`).
 */
import { memo, useState, useEffect, useCallback } from "react";
import { ScrollArea } from "@/components/ui/scroll-area";
import { ChevronRight, Folder, Loader2 } from "lucide-react";
import { FileTypeIcon } from "@/components/icons/file-type-icon";
import { listDirectory } from "@/tauri/commands";
import { openEditorTab } from "@/lib/open-editor-tab";
import { cn } from "@/lib/utils";
import { useSyncedSettingsStore, selectShowHiddenFiles } from "@/stores/synced-settings-store";
import type { WorkspaceSnapshot, FileEntry } from "@/tauri/types";

/** Row geometry, in one place because the "empty" caption has to line up
 *  with the children it stands in for. */
const ROW_PAD_LEFT = 6;
const INDENT = 11;

function indentOf(depth: number): string {
  return `${ROW_PAD_LEFT + depth * INDENT}px`;
}

interface Props {
  workspace: WorkspaceSnapshot;
  /** Where a file click lands. Defaults to a main-area editor tab; the
   *  right-panel deck passes its own handler so a click opens a doc pane
   *  in the panel instead of yanking the user to another surface. */
  onOpenFile?: (filePath: string) => void;
  /** Bumped by the deck's Refresh action — the tree's controls live in the
   *  deck's tab row now, not in a header of its own. */
  refreshKey?: number;
  /** The file to draw as selected. With no header and no size column, this
   *  is the only thing tying the tree to the pane it opened. */
  selectedPath?: string | null;
}

// `TreeNode` is recursive and the file-tree panel sits in the always-
// visible right sidebar — every app-state-changed event would otherwise
// re-render every node in every expanded subtree. The expansion sets
// and child caches are passed through Sets/Maps that we update via
// `new Set(prev).add(...)` so reference equality on those props is a
// reliable changed-vs-unchanged signal. `entry` itself is a stable ref
// inside `dirContents`, so the default comparator is enough.
const TreeNode = memo(function TreeNode({
  entry,
  depth,
  expandedDirs,
  dirContents,
  loadingDirs,
  selectedPath,
  onToggleDir,
  onClickFile,
}: {
  entry: FileEntry;
  depth: number;
  expandedDirs: Set<string>;
  dirContents: Map<string, FileEntry[]>;
  loadingDirs: Set<string>;
  selectedPath: string | null;
  onToggleDir: (path: string) => void;
  onClickFile: (path: string) => void;
}) {
  const isExpanded = expandedDirs.has(entry.path);
  const isLoading = loadingDirs.has(entry.path);
  const children = dirContents.get(entry.path);

  const row =
    "flex h-[22px] w-full items-center gap-1.5 rounded-[5px] pr-1.5 text-left text-xs transition-colors duration-[120ms]";

  if (entry.is_dir) {
    return (
      <div>
        <button
          type="button"
          className={cn(
            row,
            "text-foreground/62 hover:bg-foreground/5 hover:text-foreground/80",
            entry.is_gitignored && "opacity-50",
          )}
          style={{ paddingLeft: indentOf(depth) }}
          onClick={() => onToggleDir(entry.path)}
        >
          <ChevronRight
            className={cn(
              "h-3 w-3 shrink-0 text-foreground/35 transition-transform",
              isExpanded && "rotate-90",
            )}
          />
          <Folder className="h-3.5 w-3.5 shrink-0 text-primary/70" />
          <span className="truncate">{entry.name}</span>
          {isLoading && (
            <Loader2 className="ml-auto h-2.5 w-2.5 shrink-0 animate-spin text-foreground/35" />
          )}
        </button>
        {isExpanded && children && (
          <div>
            {children.map((child) => (
              <TreeNode
                key={child.path}
                entry={child}
                depth={depth + 1}
                expandedDirs={expandedDirs}
                dirContents={dirContents}
                loadingDirs={loadingDirs}
                selectedPath={selectedPath}
                onToggleDir={onToggleDir}
                onClickFile={onClickFile}
              />
            ))}
            {children.length === 0 && (
              <p
                className="py-0.5 text-[10px] italic text-foreground/30"
                style={{ paddingLeft: indentOf(depth + 1) }}
              >
                empty
              </p>
            )}
          </div>
        )}
      </div>
    );
  }

  const selected = selectedPath === entry.path;
  return (
    <button
      type="button"
      data-testid="file-tree-row"
      data-selected={selected ? "true" : undefined}
      className={cn(
        row,
        selected
          ? "bg-foreground/8 text-foreground"
          : "text-foreground/44 hover:bg-foreground/5 hover:text-foreground/70",
        entry.is_gitignored && "opacity-50",
      )}
      style={{ paddingLeft: indentOf(depth) }}
      onClick={() => onClickFile(entry.path)}
    >
      <span className="w-3 shrink-0" />
      <FileTypeIcon filename={entry.name} className="h-3.5 w-3.5 shrink-0 opacity-80" />
      <span className="truncate">{entry.name}</span>
    </button>
  );
});

export function FileTreePanel({
  workspace,
  onOpenFile,
  refreshKey = 0,
  selectedPath = null,
}: Props) {
  const cwd = workspace.worktree_path ?? workspace.cwd;
  const showHidden = useSyncedSettingsStore(selectShowHiddenFiles);
  const [expandedDirs, setExpandedDirs] = useState<Set<string>>(new Set());
  const [dirContents, setDirContents] = useState<Map<string, FileEntry[]>>(new Map());
  const [loadingDirs, setLoadingDirs] = useState<Set<string>>(new Set());
  const [rootEntries, setRootEntries] = useState<FileEntry[]>([]);
  // Load root on mount, when showHidden changes, or when the pane bar's
  // Refresh bumps `refreshKey`.
  useEffect(() => {
    if (!cwd) return;
    setDirContents(new Map());
    setExpandedDirs(new Set());
    listDirectory(cwd, showHidden)
      .then(setRootEntries)
      .catch(() => setRootEntries([]));
  }, [cwd, showHidden, refreshKey]);

  const toggleDir = useCallback(
    async (path: string) => {
      const next = new Set(expandedDirs);
      if (next.has(path)) {
        next.delete(path);
        setExpandedDirs(next);
        return;
      }

      // Expand and fetch if not cached
      next.add(path);
      setExpandedDirs(next);

      if (!dirContents.has(path)) {
        setLoadingDirs((prev) => new Set(prev).add(path));
        try {
          const entries = await listDirectory(path, showHidden);
          setDirContents((prev) => new Map(prev).set(path, entries));
        } catch {
          setDirContents((prev) => new Map(prev).set(path, []));
        } finally {
          setLoadingDirs((prev) => {
            const n = new Set(prev);
            n.delete(path);
            return n;
          });
        }
      }
    },
    [expandedDirs, dirContents, showHidden],
  );

  const clickFile = useCallback(
    (path: string) => {
      if (onOpenFile) {
        onOpenFile(path);
        return;
      }
      openEditorTab(workspace.workspace_id, workspace.tabs, path).catch(console.error);
    },
    [onOpenFile, workspace.workspace_id, workspace.tabs],
  );

  // The folder label, hidden-files toggle and Refresh that used to sit in
  // this panel's own 28px header now live in the deck's tab row — one row
  // of chrome for every pane instead of one header per panel.
  return (
    <div className="flex h-full flex-col">
      <ScrollArea className="flex-1">
        {/* The body sits flush under the deck's one hairline; this small
            inset is what lets the rounded rows breathe, not a gap. */}
        <div className="px-[5px] pb-1 pt-[3px]">
          {rootEntries.map((entry) => (
            <TreeNode
              key={entry.path}
              entry={entry}
              depth={0}
              expandedDirs={expandedDirs}
              dirContents={dirContents}
              loadingDirs={loadingDirs}
              selectedPath={selectedPath}
              onToggleDir={toggleDir}
              onClickFile={clickFile}
            />
          ))}
          {rootEntries.length === 0 && (
            <p className="py-8 text-center text-xs text-foreground/40">No files</p>
          )}
        </div>
      </ScrollArea>
    </div>
  );
}
