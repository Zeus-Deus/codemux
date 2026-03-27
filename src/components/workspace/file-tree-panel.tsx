import { useState, useEffect, useCallback, useRef } from "react";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { ChevronRight, File, Folder, RefreshCw, Loader2 } from "lucide-react";
import { listDirectory, detectEditors, openInEditor } from "@/tauri/commands";
import type { WorkspaceSnapshot, FileEntry, EditorInfo } from "@/tauri/types";

interface Props {
  workspace: WorkspaceSnapshot;
}

function TreeNode({
  entry,
  depth,
  expandedDirs,
  dirContents,
  loadingDirs,
  onToggleDir,
  onClickFile,
}: {
  entry: FileEntry;
  depth: number;
  expandedDirs: Set<string>;
  dirContents: Map<string, FileEntry[]>;
  loadingDirs: Set<string>;
  onToggleDir: (path: string) => void;
  onClickFile: (path: string) => void;
}) {
  const isExpanded = expandedDirs.has(entry.path);
  const isLoading = loadingDirs.has(entry.path);
  const children = dirContents.get(entry.path);

  if (entry.is_dir) {
    return (
      <div>
        <Button
          variant="ghost"
          className="w-full justify-start gap-1 rounded-sm px-1.5 py-0.5 h-auto text-xs"
          style={{ paddingLeft: `${8 + depth * 16}px` }}
          onClick={() => onToggleDir(entry.path)}
        >
          <ChevronRight
            className={`h-3 w-3 shrink-0 text-muted-foreground transition-transform ${isExpanded ? "rotate-90" : ""}`}
          />
          <Folder className="h-3 w-3 shrink-0 text-primary/70" />
          <span className="truncate text-foreground">{entry.name}</span>
          {isLoading && <Loader2 className="h-2.5 w-2.5 animate-spin text-muted-foreground ml-auto" />}
        </Button>
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
                onToggleDir={onToggleDir}
                onClickFile={onClickFile}
              />
            ))}
            {children.length === 0 && (
              <p
                className="text-[10px] text-muted-foreground/50 py-0.5"
                style={{ paddingLeft: `${8 + (depth + 1) * 16}px` }}
              >
                (empty)
              </p>
            )}
          </div>
        )}
      </div>
    );
  }

  return (
    <Button
      variant="ghost"
      className="w-full justify-start gap-1 rounded-sm px-1.5 py-0.5 h-auto text-xs"
      style={{ paddingLeft: `${8 + depth * 16}px` }}
      onClick={() => onClickFile(entry.path)}
    >
      <span className="w-3 shrink-0" />
      <File className="h-3 w-3 shrink-0 text-muted-foreground" />
      <span className="truncate text-foreground">{entry.name}</span>
      {entry.size !== null && (
        <span className="ml-auto text-[10px] text-muted-foreground/50 tabular-nums shrink-0">
          {entry.size > 1024
            ? `${(entry.size / 1024).toFixed(0)}K`
            : `${entry.size}B`}
        </span>
      )}
    </Button>
  );
}

export function FileTreePanel({ workspace }: Props) {
  const cwd = workspace.worktree_path ?? workspace.cwd;
  const [expandedDirs, setExpandedDirs] = useState<Set<string>>(new Set());
  const [dirContents, setDirContents] = useState<Map<string, FileEntry[]>>(new Map());
  const [loadingDirs, setLoadingDirs] = useState<Set<string>>(new Set());
  const [rootEntries, setRootEntries] = useState<FileEntry[]>([]);
  const editorsRef = useRef<EditorInfo[]>([]);

  // Load root + detect editors on mount
  useEffect(() => {
    if (!cwd) return;
    listDirectory(cwd)
      .then(setRootEntries)
      .catch(() => setRootEntries([]));
    detectEditors()
      .then((eds) => { editorsRef.current = eds; })
      .catch(() => {});
  }, [cwd]);

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
          const entries = await listDirectory(path);
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
    [expandedDirs, dirContents],
  );

  const clickFile = useCallback((path: string) => {
    const editor = editorsRef.current[0];
    if (editor) {
      openInEditor(editor.id, path).catch(console.error);
    }
  }, []);

  const refreshRoot = () => {
    setDirContents(new Map());
    setExpandedDirs(new Set());
    listDirectory(cwd)
      .then(setRootEntries)
      .catch(() => setRootEntries([]));
  };

  return (
    <div className="flex h-full flex-col">
      <div className="flex items-center justify-end p-1.5 border-b border-border">
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="ghost"
              size="icon-xs"
              aria-label="Refresh"
              onClick={refreshRoot}
            >
              <RefreshCw className="h-3 w-3" />
            </Button>
          </TooltipTrigger>
          <TooltipContent side="bottom" sideOffset={4}>
            Refresh
          </TooltipContent>
        </Tooltip>
      </div>
      <ScrollArea className="flex-1">
        <div className="py-1">
          {rootEntries.map((entry) => (
            <TreeNode
              key={entry.path}
              entry={entry}
              depth={0}
              expandedDirs={expandedDirs}
              dirContents={dirContents}
              loadingDirs={loadingDirs}
              onToggleDir={toggleDir}
              onClickFile={clickFile}
            />
          ))}
          {rootEntries.length === 0 && (
            <p className="text-xs text-muted-foreground text-center py-6">
              No files found
            </p>
          )}
        </div>
      </ScrollArea>
    </div>
  );
}
