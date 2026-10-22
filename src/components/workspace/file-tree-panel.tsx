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
import { memo, useState, useEffect, useCallback, useMemo, useRef, useId, type KeyboardEvent } from "react";
import { ScrollArea } from "@/components/ui/scroll-area";
import { ChevronRight, Folder } from "lucide-react";
import { FileTypeIcon } from "@/components/icons/file-type-icon";
import { listDirectory } from "@/tauri/commands";
import { openEditorTab } from "@/lib/open-editor-tab";
import { cn } from "@/lib/utils";
import { Empty, EmptyDescription } from "@/components/ui/empty";
import { useSyncedSettingsStore, selectShowHiddenFiles } from "@/stores/synced-settings-store";
import type { WorkspaceSnapshot, FileEntry } from "@/tauri/types";
import { Spinner } from "@/components/ui/spinner";

/** Row geometry, in one place because the "empty" caption has to line up
 *  with the children it stands in for. */
const ROW_PAD_LEFT = 6;
const INDENT = 11;

function indentOf(depth: number): string {
  return `${ROW_PAD_LEFT + depth * INDENT}px`;
}

function itemId(treeId: string, path: string): string {
  return `${treeId}-${encodeURIComponent(path)}`;
}

interface VisibleNode {
  entry: FileEntry;
  ancestors: string[];
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
  treeId,
  activePath,
  onFocusItem,
  onToggleDir,
  onClickFile,
}: {
  entry: FileEntry;
  depth: number;
  expandedDirs: Set<string>;
  dirContents: Map<string, FileEntry[]>;
  loadingDirs: Set<string>;
  selectedPath: string | null;
  treeId: string;
  activePath: string | null;
  onFocusItem: (path: string) => void;
  onToggleDir: (path: string) => void;
  onClickFile: (path: string) => void;
}) {
  const isExpanded = expandedDirs.has(entry.path);
  const isLoading = loadingDirs.has(entry.path);
  const children = dirContents.get(entry.path);
  const selected = selectedPath === entry.path;

  const row =
    "flex h-[22px] w-full items-center gap-1.5 rounded-sm pr-1.5 text-left text-label transition-colors duration-100";
  const activate = () => {
    onFocusItem(entry.path);
    if (entry.is_dir) onToggleDir(entry.path);
    else onClickFile(entry.path);
  };

  return (
    <div
      role="treeitem"
      id={itemId(treeId, entry.path)}
      aria-label={entry.name}
      aria-expanded={entry.is_dir ? isExpanded : undefined}
      aria-selected={entry.is_dir ? undefined : selected}
      aria-busy={entry.is_dir && isLoading ? true : undefined}
      onClick={(event) => {
        // Assistive technology can activate the treeitem itself. Row
        // clicks are handled below; descendant items handle their own.
        if (event.target === event.currentTarget) activate();
      }}
    >
      <div
        data-testid={entry.is_dir ? undefined : "file-tree-row"}
        data-selected={!entry.is_dir && selected ? "true" : undefined}
        className={cn(
          row,
          entry.is_dir
            ? "text-foreground/62 hover:bg-surface-2 hover:text-foreground/80"
            : selected
              ? "bg-surface-3 text-foreground"
              : "text-foreground/44 hover:bg-surface-2 hover:text-foreground/70",
          entry.is_gitignored && "opacity-50",
          activePath === entry.path && "group-focus-visible/files:ring-2 group-focus-visible/files:ring-inset group-focus-visible/files:ring-ring/60",
        )}
        style={{ paddingLeft: indentOf(depth) }}
        onClick={activate}
      >
        {entry.is_dir ? (
          <>
            <ChevronRight
              className={cn(
                "size-3 shrink-0 text-foreground/35 transition-transform duration-150",
                isExpanded && "rotate-90",
              )}
            />
            <Folder className="size-3.5 shrink-0 text-primary/70" />
            <span className="truncate">{entry.name}</span>
            {isLoading && (
              <Spinner className="ml-auto size-3 shrink-0 text-foreground/35" />
            )}
          </>
        ) : (
          <>
            <span className="w-3 shrink-0" />
            <FileTypeIcon filename={entry.name} className="size-3.5 shrink-0 opacity-80" />
            <span className="truncate">{entry.name}</span>
          </>
        )}
      </div>
      {entry.is_dir && isExpanded && (
        <div role="group">
          {children?.map((child) => (
            <TreeNode
              key={child.path}
              entry={child}
              depth={depth + 1}
              expandedDirs={expandedDirs}
              dirContents={dirContents}
              loadingDirs={loadingDirs}
              selectedPath={selectedPath}
              treeId={treeId}
              activePath={activePath}
              onFocusItem={onFocusItem}
              onToggleDir={onToggleDir}
              onClickFile={onClickFile}
            />
          ))}
          {children?.length === 0 && (
            <p
              className="py-0.5 text-caption italic text-foreground/30"
              style={{ paddingLeft: indentOf(depth + 1) }}
            >
              empty
            </p>
          )}
        </div>
      )}
    </div>
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
  const rootCwd = useRef(cwd);
  const loadGeneration = useRef(0);
  const pendingDirs = useRef(new Set<string>());
  const treeId = useId();
  const treeRef = useRef<HTMLDivElement>(null);
  const pointerFocus = useRef(false);
  const [focusTrail, setFocusTrail] = useState<string[]>([]);
  const visibleNodes = useMemo(() => {
    const nodes: VisibleNode[] = [];
    function visit(entries: FileEntry[], ancestors: string[]) {
      for (const entry of entries) {
        nodes.push({ entry, ancestors });
        if (entry.is_dir && expandedDirs.has(entry.path)) {
          visit(dirContents.get(entry.path) ?? [], [...ancestors, entry.path]);
        }
      }
    }
    visit(rootEntries, []);
    return nodes;
  }, [rootEntries, expandedDirs, dirContents]);
  // Keep a surviving active item, or its closest visible ancestor after
  // collapse/reload. Selection belongs to the opened file, not arrow focus.
  const activeNode = useMemo(() => {
    for (let i = focusTrail.length - 1; i >= 0; i--) {
      const node = visibleNodes.find(({ entry }) => entry.path === focusTrail[i]);
      if (node) return node;
    }
    return visibleNodes.find(({ entry }) => entry.path === selectedPath) ?? visibleNodes[0];
  }, [focusTrail, visibleNodes, selectedPath]);
  const activePath = activeNode?.entry.path ?? null;
  const activeId = activePath === null ? undefined : itemId(treeId, activePath);
  useEffect(() => {
    // Commit the fallback so reopening a folder cannot jump to its former
    // descendant. DOM focus stays on the tree even when an item disappears.
    if ((focusTrail[focusTrail.length - 1] ?? null) !== activePath) {
      setFocusTrail(activeNode ? [...activeNode.ancestors, activeNode.entry.path] : []);
    }
  }, [activeNode, activePath, focusTrail]);
  const scrollActiveIntoView = useCallback(() => {
    if (activeId && !pointerFocus.current && document.activeElement === treeRef.current) {
      document.getElementById(activeId)?.firstElementChild?.scrollIntoView({ block: "nearest" });
    }
  }, [activeId]);
  // Newly loaded children can move an active sibling without changing its ID.
  useEffect(scrollActiveIntoView, [scrollActiveIntoView, visibleNodes]);
  const focusItem = useCallback((path: string) => {
    const node = visibleNodes.find(({ entry }) => entry.path === path);
    if (node) setFocusTrail([...node.ancestors, path]);
    // Pointer focus precedes click. Scrolling the previous active item
    // during that focus can move the pressed row out from under the mouse.
    pointerFocus.current = true;
    treeRef.current?.focus({ preventScroll: true });
    pointerFocus.current = false;
  }, [visibleNodes]);
  // Load root on mount, when showHidden changes, or when the pane bar's
  // Refresh bumps `refreshKey`.
  useEffect(() => {
    const generation = ++loadGeneration.current;
    pendingDirs.current.clear();
    setDirContents(new Map());
    setExpandedDirs(new Set());
    setLoadingDirs(new Set());
    // Keep current-root rows during reload so Refresh/show-hidden cannot
    // flash "No files". A different root must not show the old root's rows.
    if (rootCwd.current !== cwd) {
      rootCwd.current = cwd;
      setRootEntries([]);
    }
    if (cwd) {
      listDirectory(cwd, showHidden)
        .then((entries) => {
          if (loadGeneration.current === generation) setRootEntries(entries);
        })
        .catch(() => {
          if (loadGeneration.current === generation) setRootEntries([]);
        });
    }
    // Directory reads cannot be aborted. Ignore replies from a previous
    // workspace, hidden-files setting, refresh, or unmounted tree.
    return () => { loadGeneration.current++; };
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

      if (!dirContents.has(path) && !pendingDirs.current.has(path)) {
        const generation = loadGeneration.current;
        pendingDirs.current.add(path);
        setLoadingDirs((prev) => new Set(prev).add(path));
        try {
          const entries = await listDirectory(path, showHidden);
          if (loadGeneration.current === generation) {
            setDirContents((prev) => new Map(prev).set(path, entries));
          }
        } catch {
          if (loadGeneration.current === generation) {
            setDirContents((prev) => new Map(prev).set(path, []));
          }
        } finally {
          if (loadGeneration.current === generation) {
            pendingDirs.current.delete(path);
            setLoadingDirs((prev) => {
              const n = new Set(prev);
              n.delete(path);
              return n;
            });
          }
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

  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.altKey || event.ctrlKey || event.metaKey || event.nativeEvent.isComposing) return;
    if (!activeNode) return;
    pointerFocus.current = false;
    const index = visibleNodes.indexOf(activeNode);
    let target: VisibleNode | undefined;
    switch (event.key) {
      case "ArrowDown": target = visibleNodes[index + 1]; break;
      case "ArrowUp": target = visibleNodes[index - 1]; break;
      case "Home": target = visibleNodes[0]; break;
      case "End": target = visibleNodes[visibleNodes.length - 1]; break;
      case "ArrowRight":
        if (activeNode.entry.is_dir) {
          if (!expandedDirs.has(activeNode.entry.path)) void toggleDir(activeNode.entry.path);
          else if (dirContents.get(activeNode.entry.path)?.length) {
            target = visibleNodes[index + 1];
          }
        }
        break;
      case "ArrowLeft":
        if (activeNode.entry.is_dir && expandedDirs.has(activeNode.entry.path)) {
          void toggleDir(activeNode.entry.path);
        } else {
          const parentPath = activeNode.ancestors[activeNode.ancestors.length - 1];
          target = visibleNodes.find(({ entry }) => entry.path === parentPath);
        }
        break;
      case "Enter":
      case " ":
        if (!event.repeat) {
          if (activeNode.entry.is_dir) void toggleDir(activeNode.entry.path);
          else clickFile(activeNode.entry.path);
        }
        break;
      default: return;
    }
    event.preventDefault();
    event.stopPropagation();
    if (target) setFocusTrail([...target.ancestors, target.entry.path]);
    // Boundary keys still reveal focus after the user wheel-scrolls away.
    if (!target || target.entry.path === activePath) scrollActiveIntoView();
  };

  // The folder label, hidden-files toggle and Refresh that used to sit in
  // this panel's own 28px header now live in the deck's tab row — one row
  // of chrome for every pane instead of one header per panel.
  return (
    <div className="flex h-full flex-col">
      <ScrollArea className="flex-1">
        {/* The body sits flush under the deck's one hairline; this small
            inset is what lets the rounded rows breathe, not a gap. */}
        <div
          ref={treeRef}
          role="tree"
          aria-label="Files"
          aria-activedescendant={activeId}
          tabIndex={0}
          className="group/files px-[5px] pb-1 pt-[3px] outline-none"
          onKeyDown={handleKeyDown}
          onPointerDownCapture={() => { pointerFocus.current = true; }}
          onPointerUpCapture={() => { pointerFocus.current = false; }}
          onPointerCancelCapture={() => { pointerFocus.current = false; }}
          onFocus={scrollActiveIntoView}
          onBlur={() => { pointerFocus.current = false; }}
        >
          {rootEntries.map((entry) => (
            <TreeNode
              key={entry.path}
              entry={entry}
              depth={0}
              expandedDirs={expandedDirs}
              dirContents={dirContents}
              loadingDirs={loadingDirs}
              selectedPath={selectedPath}
              treeId={treeId}
              activePath={activePath}
              onFocusItem={focusItem}
              onToggleDir={toggleDir}
              onClickFile={clickFile}
            />
          ))}
          {rootEntries.length === 0 && (
            <Empty>
              <EmptyDescription>No files</EmptyDescription>
            </Empty>
          )}
        </div>
      </ScrollArea>
    </div>
  );
}
