import { useState, useEffect, useRef, useCallback } from "react";
import {
  Dialog,
  DialogContent,
  DialogTitle,
  DialogDescription,
  DIALOG_CRISP_POSITION,
} from "@/components/ui/dialog";
import { cn } from "@/lib/utils";
import { Eyebrow } from "@/components/ui/eyebrow";
import { FileTypeIcon } from "@/components/icons/file-type-icon";
import { useUIStore } from "@/stores/ui-store";
import { useEditorStore } from "@/stores/editor-store";
import { selectActiveWorkspaceId, useActiveWorkspaceCwd, useAppStore } from "@/stores/app-store";
import { getGitStatus, searchFileNames } from "@/tauri/commands";
import { openRightPanelDoc } from "@/lib/open-right-panel-doc";
import { openEditorTab } from "@/lib/open-editor-tab";
import { basename } from "@/lib/path";
import {
  MatchHighlight,
  ResultCapNotice,
  SearchFooter,
  SearchQueryInput,
  findMatchRange,
  relativeToRoot,
  staleListClass,
} from "./search-dialog-parts";

/** Results asked of the backend per query. Hitting it means there may be more. */
export const FILE_SEARCH_LIMIT = 20;
/** Per-section cap for the empty-query suggestions. */
const SUGGESTION_LIMIT = 8;

type SuggestionSection = "open" | "changed";
interface Item {
  path: string;
  section?: SuggestionSection;
}

const SECTION_LABEL: Record<SuggestionSection, string> = {
  open: "Open in editor",
  changed: "Changed",
};

function activeWorkspace() {
  const state = useAppStore.getState();
  const activeId = selectActiveWorkspaceId(state);
  return state.appState?.workspaces.find((w) => w.workspace_id === activeId);
}

/** Files already open in the active workspace's editor tabs. */
function openEditorPaths(cwd: string): string[] {
  const ws = activeWorkspace();
  if (!ws) return [];
  const editor = useEditorStore.getState();
  const paths = ws.tabs
    .filter((t) => t.kind === "editor")
    .map((t) => editor.getTab(t.tab_id)?.filePath)
    .filter((p): p is string => !!p)
    .map((p) => relativeToRoot(cwd, p));
  return [...new Set(paths)].slice(0, SUGGESTION_LIMIT);
}

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
  const [suggestions, setSuggestions] = useState<Item[]>([]);
  const [loading, setLoading] = useState(false);
  const [selectedIndex, setSelectedIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  // Bumped per query so a slow response can't overwrite a newer one.
  const requestRef = useRef(0);

  // Reset state when dialog opens, and offer the files the user most likely
  // wants before they type anything.
  useEffect(() => {
    if (!open) return;
    setQuery("");
    setResults([]);
    setSelectedIndex(0);
    setTimeout(() => inputRef.current?.focus(), 0);

    const opened = openEditorPaths(cwd);
    setSuggestions(opened.map((path) => ({ path, section: "open" })));
    if (!cwd) return;
    let cancelled = false;
    getGitStatus(cwd)
      .then((files) => {
        if (cancelled || !Array.isArray(files)) return;
        const changed = files
          .filter((f) => f.status !== "deleted" && !opened.includes(f.path))
          .slice(0, SUGGESTION_LIMIT)
          .map((f): Item => ({ path: f.path, section: "changed" }));
        setSuggestions((prev) => [...prev, ...changed]);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [open, cwd]);

  // Debounced search
  useEffect(() => {
    const request = ++requestRef.current;
    if (!open || !cwd || !query.trim()) {
      setResults([]);
      setLoading(false);
      return;
    }
    setLoading(true);
    if (debounceRef.current) clearTimeout(debounceRef.current);
    debounceRef.current = setTimeout(() => {
      searchFileNames(cwd, query.trim(), FILE_SEARCH_LIMIT)
        .then((files) => {
          if (request !== requestRef.current) return;
          setResults(files ?? []);
          setSelectedIndex(0);
        })
        .catch(() => {
          if (request === requestRef.current) setResults([]);
        })
        .finally(() => {
          if (request === requestRef.current) setLoading(false);
        });
    }, 200);
    return () => {
      if (debounceRef.current) clearTimeout(debounceRef.current);
    };
  }, [open, cwd, query]);

  const searching = !!query.trim();
  const items: Item[] = searching ? results.map((path) => ({ path })) : suggestions;

  const openFile = useCallback(
    async (filePath: string) => {
      // Pull the live workspace at click time via getState so the
      // dialog doesn't subscribe to the workspace ref (which churns on
      // every backend tick). On user click, latency from a single
      // getState read is irrelevant.
      const ws = activeWorkspace();
      if (!ws) return;
      try {
        const fullPath = filePath.startsWith("/") ? filePath : `${cwd}/${filePath}`;
        // "Open file…" from the right panel's `+` menu targets the deck:
        // the pick becomes a closable doc pane there instead of yanking
        // the user to a main-area editor tab.
        if (useUIStore.getState().fileSearchTarget === "right-panel") {
          openRightPanelDoc(ws.workspace_id, fullPath);
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
      setSelectedIndex((i) => Math.min(i + 1, items.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setSelectedIndex((i) => Math.max(i - 1, 0));
    } else if (e.key === "Enter" && items[selectedIndex]) {
      e.preventDefault();
      openFile(items[selectedIndex].path);
    }
  };

  // Scroll selected item into view
  useEffect(() => {
    const item = listRef.current?.querySelector<HTMLElement>(
      `[data-item-index="${selectedIndex}"]`,
    );
    item?.scrollIntoView({ block: "nearest" });
  }, [selectedIndex]);

  const capped = searching && results.length >= FILE_SEARCH_LIMIT;

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
        className={cn(DIALOG_CRISP_POSITION, "gap-0 overflow-hidden p-0")}
        showCloseButton={false}
        onKeyDown={handleKeyDown}
      >
        <DialogTitle className="sr-only">Search Files</DialogTitle>
        <DialogDescription className="sr-only">Find files by name</DialogDescription>
        <div className="p-3 pb-0">
          <SearchQueryInput
            inputRef={inputRef}
            value={query}
            onChange={setQuery}
            placeholder="Search files by name..."
            loading={loading}
          />
        </div>
        <div
          ref={listRef}
          className={cn("max-h-[50vh] overflow-y-auto p-1.5", staleListClass(loading))}
        >
          {!searching && items.length === 0 && (
            <p className="text-label text-muted-foreground text-center py-8">
              Type a file name to search
            </p>
          )}
          {searching && !loading && results.length === 0 && (
            <p className="text-label text-muted-foreground text-center py-8">
              No matching files
            </p>
          )}
          {items.map((item, idx) => {
            const fileName = basename(item.path);
            const parts = item.path.split(/[\\/]/);
            parts.pop();
            const dirPath = parts.join("/");
            const header =
              item.section && item.section !== items[idx - 1]?.section ? item.section : null;
            return (
              <div key={`${item.section ?? "result"}:${item.path}`}>
                {header && (
                  <Eyebrow className={cn("px-2 pb-1", idx === 0 ? "pt-1" : "pt-2.5")}>
                    {SECTION_LABEL[header]}
                  </Eyebrow>
                )}
                <button
                  data-item-index={idx}
                  className={cn(
                    "flex w-full min-w-0 items-center gap-2 rounded-md px-2 py-1.5 text-left text-body",
                    idx === selectedIndex ? "bg-accent" : "hover:bg-accent/50",
                  )}
                  onClick={() => openFile(item.path)}
                  onMouseEnter={() => setSelectedIndex(idx)}
                >
                  <FileTypeIcon filename={fileName} className="size-3.5 opacity-75" />
                  <span className="max-w-[70%] shrink-0 truncate font-medium">
                    <MatchHighlight
                      text={fileName}
                      range={searching ? findMatchRange(fileName, query) : null}
                      className="rounded-sm bg-accent-ember/15 text-foreground"
                    />
                  </span>
                  {dirPath && (
                    // RTL clips the start of a deep path, so the folder the
                    // file actually sits in stays visible; <bdi> keeps the
                    // path itself reading left to right.
                    <span
                      dir="rtl"
                      className="min-w-0 flex-1 truncate text-left text-label text-muted-foreground"
                    >
                      <bdi>{dirPath}</bdi>
                    </span>
                  )}
                </button>
              </div>
            );
          })}
          {capped && <ResultCapNotice limit={FILE_SEARCH_LIMIT} />}
        </div>
        <SearchFooter
          hints={[
            { keys: "↑↓", label: "navigate" },
            { keys: "↵", label: "open" },
            { keys: "esc", label: "close" },
          ]}
        />
      </DialogContent>
    </Dialog>
  );
}
