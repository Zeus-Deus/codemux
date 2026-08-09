import { useCallback, useEffect, useRef, useState } from "react";
import {
  ArrowUp,
  ChevronRight,
  File as FileIcon,
  Folder,
  RefreshCw,
} from "lucide-react";

import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { getHomeDir, listDirectory } from "@/tauri/commands";
import type { FileEntry } from "@/tauri/types";
import { useHomeDir } from "@/stores/app-store";
import { cn } from "@/lib/utils";

import {
  useRemotePathPickerStore,
  type PathPickerMode,
} from "./remote-path-picker-store";
import { isRootPath, parentPath, pathBreadcrumbs } from "./path-utils";

/**
 * Web-remote replacement for the native folder/file OS dialog.
 *
 * On the desktop this component is never mounted (the native `pick_*_dialog`
 * commands run instead). On the web remote client `src/lib/file-dialog.ts`
 * routes through {@link useRemotePathPickerStore}, which opens this modal. It
 * navigates the *host* filesystem over the existing `list_directory` command
 * so a browser on another machine can open/create projects, attach files, etc.
 *
 * It returns exactly the shape the native path returns — a single absolute
 * folder path (folder mode) or a list of absolute file paths (files mode) —
 * so every caller (workspace creation, clone destination, chat attachments)
 * works unchanged.
 */
export function RemotePathPicker() {
  const request = useRemotePathPickerStore((s) => s.request);
  const finish = useRemotePathPickerStore((s) => s.finish);
  const cachedHome = useHomeDir();

  const [currentPath, setCurrentPath] = useState<string>("");
  const [pathInput, setPathInput] = useState<string>("");
  const [entries, setEntries] = useState<FileEntry[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showHidden, setShowHidden] = useState(false);

  // Monotonic token so a slow listing for a stale path can't clobber the
  // result of a newer navigation.
  const navToken = useRef(0);

  const open = request !== null;
  const mode: PathPickerMode = request?.mode ?? "folder";
  const isFileMode = mode === "files";

  const loadDir = useCallback(
    async (path: string, hidden: boolean) => {
      const token = ++navToken.current;
      setLoading(true);
      setError(null);
      try {
        const result = await listDirectory(path, hidden);
        if (token !== navToken.current) return;
        setEntries(result);
        setCurrentPath(path);
        setPathInput(path);
      } catch (err) {
        if (token !== navToken.current) return;
        const message =
          err instanceof Error
            ? err.message
            : typeof err === "string"
              ? err
              : String(err);
        setError(message);
      } finally {
        if (token === navToken.current) setLoading(false);
      }
    },
    [],
  );

  // Initialize on each fresh open: reset selection, start at the user's home
  // directory (cached from App mount, else fetched on demand).
  const requestId = request?.id;
  useEffect(() => {
    if (requestId === undefined) return;
    setSelected(new Set());
    setShowHidden(false);
    void (async () => {
      let start = cachedHome;
      if (!start) {
        try {
          start = await getHomeDir();
        } catch {
          start = "/";
        }
      }
      await loadDir(start, false);
    })();
    // Only re-run when a genuinely new request opens.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [requestId]);

  const handleOpenChange = (next: boolean) => {
    if (!next) finish(null);
  };

  const navigateTo = (path: string) => {
    void loadDir(path, showHidden);
  };

  const toggleHidden = () => {
    const next = !showHidden;
    setShowHidden(next);
    void loadDir(currentPath, next);
  };

  const handleEntryClick = (entry: FileEntry) => {
    if (entry.is_dir) {
      navigateTo(entry.path);
      return;
    }
    if (!isFileMode) return;
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(entry.path)) next.delete(entry.path);
      else next.add(entry.path);
      return next;
    });
  };

  const handleManualGo = () => {
    const target = pathInput.trim();
    if (target) navigateTo(target);
  };

  const handleConfirm = () => {
    if (isFileMode) {
      if (selected.size === 0) return;
      finish([...selected]);
    } else {
      finish([currentPath]);
    }
  };

  const crumbs = currentPath ? pathBreadcrumbs(currentPath) : [];
  const atRoot = currentPath ? isRootPath(currentPath) : true;
  const selectDisabled = isFileMode ? selected.size === 0 : !currentPath;
  const selectLabel = isFileMode
    ? selected.size > 0
      ? `Select ${selected.size} file${selected.size === 1 ? "" : "s"}`
      : "Select files"
    : "Use this folder";

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent
        showCloseButton
        className="sm:max-w-[560px] bg-popover p-0 gap-0 overflow-hidden"
      >
        <DialogHeader className="px-5 pt-4 pb-2">
          <DialogTitle className="text-sm">
            {request?.title ?? (isFileMode ? "Select files" : "Select folder")}
          </DialogTitle>
          <DialogDescription className="text-xs text-muted-foreground">
            {isFileMode
              ? "Browse the host and choose one or more files."
              : "Browse the host and choose a folder."}
          </DialogDescription>
        </DialogHeader>

        <div className="px-5 pb-4 space-y-3">
          {/* Breadcrumb + up */}
          <div className="flex items-center gap-1.5">
            <Button
              variant="outline"
              size="icon-sm"
              className="shrink-0"
              onClick={() => navigateTo(parentPath(currentPath))}
              disabled={atRoot || loading}
              aria-label="Up one level"
            >
              <ArrowUp className="h-3.5 w-3.5" />
            </Button>
            <div className="flex min-w-0 flex-1 items-center overflow-x-auto whitespace-nowrap rounded-md border border-border bg-background/40 px-2 py-1 text-xs text-muted-foreground">
              {crumbs.map((crumb, i) => (
                <span key={crumb.path} className="flex items-center">
                  {i > 0 && (
                    <ChevronRight className="mx-0.5 h-3 w-3 opacity-50" />
                  )}
                  <button
                    type="button"
                    className={cn(
                      "rounded px-1 py-0.5 hover:bg-muted hover:text-foreground",
                      i === crumbs.length - 1 && "text-foreground",
                    )}
                    onClick={() => navigateTo(crumb.path)}
                  >
                    {crumb.name}
                  </button>
                </span>
              ))}
            </div>
          </div>

          {/* Manual path entry */}
          <div className="flex gap-1.5">
            <Input
              value={pathInput}
              onChange={(e) => setPathInput(e.target.value)}
              placeholder="/absolute/path"
              className="h-8 text-sm flex-1 font-mono"
              onKeyDown={(e) => {
                if (e.key === "Enter") handleManualGo();
              }}
            />
            <Button
              variant="outline"
              size="sm"
              className="h-8 shrink-0"
              onClick={handleManualGo}
              disabled={loading}
            >
              Go
            </Button>
            <Button
              variant="outline"
              size="icon-sm"
              className="h-8 w-8 shrink-0"
              onClick={() => navigateTo(currentPath)}
              disabled={loading || !currentPath}
              aria-label="Refresh"
            >
              <RefreshCw
                className={cn("h-3.5 w-3.5", loading && "animate-spin")}
              />
            </Button>
          </div>

          {/* Listing */}
          <div className="rounded-md border border-border bg-background/40">
            <ScrollArea className="h-64">
              <div className="p-1">
                {error ? (
                  <div className="p-3 text-xs text-destructive">{error}</div>
                ) : entries.length === 0 && !loading ? (
                  <div className="p-3 text-xs text-muted-foreground">
                    This folder is empty.
                  </div>
                ) : (
                  entries.map((entry) => {
                    const selectable = entry.is_dir || isFileMode;
                    const isSelected = selected.has(entry.path);
                    return (
                      <button
                        key={entry.path}
                        type="button"
                        disabled={!selectable}
                        onClick={() => handleEntryClick(entry)}
                        className={cn(
                          "flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-sm",
                          selectable
                            ? "hover:bg-muted"
                            : "cursor-default opacity-40",
                          isSelected && "bg-primary/15 hover:bg-primary/20",
                        )}
                      >
                        {entry.is_dir ? (
                          <Folder className="h-3.5 w-3.5 shrink-0 text-status-remote/80" />
                        ) : (
                          <FileIcon className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                        )}
                        <span
                          className={cn(
                            "truncate",
                            entry.is_gitignored && "text-muted-foreground/70",
                          )}
                        >
                          {entry.name}
                        </span>
                        {entry.is_dir && (
                          <ChevronRight className="ml-auto h-3.5 w-3.5 shrink-0 opacity-30" />
                        )}
                      </button>
                    );
                  })
                )}
              </div>
            </ScrollArea>
          </div>

          {/* Footer */}
          <div className="flex items-center justify-between pt-1">
            <label className="flex select-none items-center gap-1.5 text-xs text-muted-foreground">
              <input
                type="checkbox"
                checked={showHidden}
                onChange={toggleHidden}
                className="h-3 w-3 accent-primary"
              />
              Show hidden
            </label>
            <div className="flex gap-2">
              <Button variant="outline" size="sm" onClick={() => finish(null)}>
                Cancel
              </Button>
              <Button
                size="sm"
                onClick={handleConfirm}
                disabled={selectDisabled}
                className="bg-foreground text-background hover:bg-foreground/90"
              >
                {selectLabel}
              </Button>
            </div>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
