import { useState, useEffect, useRef, useCallback } from "react";
import {
  Dialog,
  DialogContent,
  DialogTitle,
  DialogDescription,
  DIALOG_CRISP_POSITION,
} from "@/components/ui/dialog";
import { cn } from "@/lib/utils";
import { Input } from "@/components/ui/input";
import { Loader2 } from "lucide-react";
import { FileTypeIcon } from "@/components/icons/file-type-icon";
import { useUIStore } from "@/stores/ui-store";
import { useEditorStore } from "@/stores/editor-store";
import { useActiveWorkspaceCwd, useAppStore } from "@/stores/app-store";
import { searchFileNames } from "@/tauri/commands";
import { docEditorTabId } from "@/components/layout/right-panel/doc-pane";
import { docPaneId } from "@/components/layout/right-panel/pane-registry";
import { openEditorTab } from "@/lib/open-editor-tab";
import { basename } from "@/lib/path";

export function FileSearchDialog() {
  const open = useUIStore((s) => s.showFileSearch);
  const setOpen = useUIStore((s) => s.setShowFileSearch);
  // Subscribe only to cwd (a primitive string) instead of the whole
  // workspace object. The dialog used to re-render on every backend
  // tick because the workspace ref churns; now it only re-renders when
  // cwd actually changes (i.e. workspace switch).
  const cwd = useActiveWorkspaceCwd() ?? "";

  const [query, setQuery] = useState("");
  const [results, setResults] = useState<string[]>([]);
  const [loading, setLoading] = useState(false);
  const [selectedIndex, setSelectedIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  // Reset state when dialog opens
  useEffect(() => {
    if (open) {
      setQuery("");
      setResults([]);
      setSelectedIndex(0);
      setTimeout(() => inputRef.current?.focus(), 0);
    }
  }, [open]);

  // Debounced search
  useEffect(() => {
    if (!open || !cwd || !query.trim()) {
      setResults([]);
      return;
    }
    setLoading(true);
    if (debounceRef.current) clearTimeout(debounceRef.current);
    debounceRef.current = setTimeout(() => {
      searchFileNames(cwd, query.trim(), 20)
        .then((files) => {
          setResults(files);
          setSelectedIndex(0);
        })
        .catch(() => setResults([]))
        .finally(() => setLoading(false));
    }, 200);
    return () => {
      if (debounceRef.current) clearTimeout(debounceRef.current);
    };
  }, [open, cwd, query]);

  const openFile = useCallback(
    async (filePath: string) => {
      // Pull the live workspace at click time via getState so the
      // dialog doesn't subscribe to the workspace ref (which churns on
      // every backend tick). On user click, latency from a single
      // getState read is irrelevant.
      const appState = useAppStore.getState().appState;
      const ws = appState?.workspaces.find(
        (w) => w.workspace_id === appState.active_workspace_id,
      );
      if (!ws) return;
      try {
        const fullPath = filePath.startsWith("/") ? filePath : `${cwd}/${filePath}`;
        // "Open file…" from the right panel's `+` menu targets the deck:
        // the pick becomes a closable doc pane there instead of yanking
        // the user to a main-area editor tab.
        if (useUIStore.getState().fileSearchTarget === "right-panel") {
          useEditorStore
            .getState()
            .initTab(docEditorTabId(ws.workspace_id, fullPath), {
              filePath: fullPath,
            });
          useUIStore.getState().setRightPanelTab(ws.workspace_id, docPaneId(fullPath));
        } else {
          await openEditorTab(ws.workspace_id, ws.tabs, fullPath);
        }
      } catch (err) {
        console.error("Failed to open file:", err);
      }
      setOpen(false);
    },
    [cwd, setOpen],
  );

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setSelectedIndex((i) => Math.min(i + 1, results.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setSelectedIndex((i) => Math.max(i - 1, 0));
    } else if (e.key === "Enter" && results[selectedIndex]) {
      e.preventDefault();
      openFile(results[selectedIndex]);
    }
  };

  // Scroll selected item into view
  useEffect(() => {
    const list = listRef.current;
    if (!list) return;
    const item = list.children[selectedIndex] as HTMLElement | undefined;
    item?.scrollIntoView({ block: "nearest" });
  }, [selectedIndex]);

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      {/* `DIALOG_CRISP_POSITION`: this dialog is nothing but text, and the
       *  default translate-based centering was rasterizing it on a half
       *  pixel — see the constant for the mechanism.
       *
       *  No `×`: this app's overlays close on Escape or a backdrop click and
       *  pass `showCloseButton={false}` — the palette, clone, new-workspace,
       *  confirm-push, archive and settings dialogs all do. This one never
       *  opted out, so it inherited the shared absolutely-positioned button,
       *  which landed on the search input's top-right corner because the
       *  content here is `p-0` rather than the `p-4` that button assumes. */}
      <DialogContent
        className={cn(DIALOG_CRISP_POSITION, "gap-0 p-0")}
        showCloseButton={false}
        onKeyDown={handleKeyDown}
      >
        <DialogTitle className="sr-only">Search Files</DialogTitle>
        <DialogDescription className="sr-only">Find files by name</DialogDescription>
        <div className="p-3 pb-0">
          <Input
            ref={inputRef}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search files by name..."
            className="h-9 text-sm"
          />
        </div>
        <div ref={listRef} className="max-h-[50vh] overflow-y-auto p-1.5">
          {!query.trim() && (
            <p className="text-xs text-muted-foreground text-center py-8">
              Type a file name to search
            </p>
          )}
          {query.trim() && loading && (
            <div className="flex justify-center py-8">
              <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
            </div>
          )}
          {query.trim() && !loading && results.length === 0 && (
            <p className="text-xs text-muted-foreground text-center py-8">
              No matching files
            </p>
          )}
          {results.map((filePath, idx) => {
            const fileName = basename(filePath);
            const parts = filePath.split(/[\\/]/);
            parts.pop();
            const dirPath = parts.join("/");
            return (
              <button
                key={filePath}
                className={`flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm ${
                  idx === selectedIndex ? "bg-accent" : "hover:bg-accent/50"
                }`}
                onClick={() => openFile(filePath)}
                onMouseEnter={() => setSelectedIndex(idx)}
              >
                <FileTypeIcon filename={fileName} className="h-3.5 w-3.5 opacity-75" />
                <div className="min-w-0 flex-1">
                  <span className="font-medium">{fileName}</span>
                  {dirPath && (
                    <span className="ml-2 text-xs text-muted-foreground truncate">
                      {dirPath}
                    </span>
                  )}
                </div>
              </button>
            );
          })}
        </div>
      </DialogContent>
    </Dialog>
  );
}
