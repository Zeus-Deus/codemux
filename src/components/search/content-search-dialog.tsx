import { useState, useEffect, useRef, useCallback, useMemo } from "react";
import {
  Dialog,
  DialogContent,
  DialogTitle,
  DialogDescription,
  DIALOG_CRISP_POSITION,
} from "@/components/ui/dialog";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { FileCode, CaseSensitive, Regex } from "lucide-react";
import { useUIStore } from "@/stores/ui-store";
import { useEditorStore } from "@/stores/editor-store";
import { selectActiveWorkspaceId, useActiveWorkspaceCwd, useAppStore } from "@/stores/app-store";
import { searchInFiles } from "@/tauri/commands";
import { openEditorTab } from "@/lib/open-editor-tab";
import type { SearchResult } from "@/tauri/types";
import {
  MatchHighlight,
  ResultCapNotice,
  SearchFooter,
  SearchQueryInput,
  pathUnderRoot,
  staleListClass,
} from "./search-dialog-parts";

/** Matches asked of the backend per query. Hitting it means there may be more. */
export const CONTENT_SEARCH_LIMIT = 100;

interface GroupedResults {
  filePath: string;
  matches: SearchResult[];
}

export function ContentSearchDialog() {
  const open = useUIStore((s) => s.showContentSearch);
  const setOpen = useUIStore((s) => s.setShowContentSearch);
  // Subscribe only to cwd (a primitive string). The full-workspace
  // selector churns its reference on every backend tick.
  const cwd = useActiveWorkspaceCwd() ?? "";

  const [query, setQuery] = useState("");
  const [results, setResults] = useState<SearchResult[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [caseSensitive, setCaseSensitive] = useState(false);
  const [useRegex, setUseRegex] = useState(false);
  const [selectedIndex, setSelectedIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  // Bumped per query so a slow response can't overwrite a newer one.
  const requestRef = useRef(0);

  // Reset on open
  useEffect(() => {
    if (open) {
      setQuery("");
      setResults([]);
      setError(null);
      setSelectedIndex(0);
      setTimeout(() => inputRef.current?.focus(), 0);
    }
  }, [open]);

  // Debounced search
  useEffect(() => {
    const request = ++requestRef.current;
    if (!open || !cwd || !query.trim()) {
      setResults([]);
      setError(null);
      setLoading(false);
      return;
    }
    setLoading(true);
    if (debounceRef.current) clearTimeout(debounceRef.current);
    debounceRef.current = setTimeout(() => {
      searchInFiles(cwd, query.trim(), useRegex, caseSensitive, CONTENT_SEARCH_LIMIT)
        .then((res) => {
          if (request !== requestRef.current) return;
          setResults(res ?? []);
          setError(null);
          setSelectedIndex(0);
        })
        .catch((err: unknown) => {
          if (request !== requestRef.current) return;
          setResults([]);
          setError(String(err));
        })
        .finally(() => {
          if (request === requestRef.current) setLoading(false);
        });
    }, 300);
    return () => {
      if (debounceRef.current) clearTimeout(debounceRef.current);
    };
  }, [open, cwd, query, useRegex, caseSensitive]);

  // Group results by file
  const grouped = useMemo((): GroupedResults[] => {
    const map = new Map<string, SearchResult[]>();
    for (const r of results) {
      if (!map.has(r.file_path)) map.set(r.file_path, []);
      map.get(r.file_path)!.push(r);
    }
    return Array.from(map.entries()).map(([filePath, matches]) => ({
      filePath,
      matches,
    }));
  }, [results]);

  const fileCount = grouped.length;
  const capped = results.length >= CONTENT_SEARCH_LIMIT;

  const openMatch = useCallback(
    async (match: SearchResult) => {
      // Pull the live workspace at click time via getState so the
      // dialog doesn't subscribe to the workspace ref (which churns on
      // every backend tick).
      const appState = useAppStore.getState().appState;
      const ws = appState?.workspaces.find(
        (w) => w.workspace_id === selectActiveWorkspaceId(useAppStore.getState()),
      );
      if (!ws) return;
      try {
        const fullPath = match.file_path.startsWith("/")
          ? match.file_path
          : `${cwd}/${match.file_path}`;
        const tabId = await openEditorTab(ws.workspace_id, ws.tabs, fullPath);
        // Land on the hit itself, selected, rather than the top of the file.
        // Offsets are 0-based UTF-16; editor columns are 1-based.
        useEditorStore
          .getState()
          .requestReveal(tabId, match.line_number, match.match_start + 1, match.match_end + 1);
      } catch (err) {
        console.error("Failed to open file:", err);
      }
      setOpen(false);
    },
    [cwd, setOpen],
  );

  const handleKeyDown = (e: React.KeyboardEvent) => {
    // Alt+C / Alt+R mirror the editor-style toggles. `code`, not `key`:
    // Alt changes the produced character on some layouts.
    if (e.altKey && !e.ctrlKey && !e.metaKey && !e.shiftKey) {
      if (e.code === "KeyC") {
        e.preventDefault();
        setCaseSensitive((v) => !v);
        return;
      }
      if (e.code === "KeyR") {
        e.preventDefault();
        setUseRegex((v) => !v);
        return;
      }
    }
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setSelectedIndex((i) => Math.min(i + 1, results.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setSelectedIndex((i) => Math.max(i - 1, 0));
    } else if (e.key === "Enter" && results[selectedIndex]) {
      e.preventDefault();
      openMatch(results[selectedIndex]);
    }
  };

  // Scroll selected into view
  useEffect(() => {
    const list = listRef.current;
    if (!list) return;
    const item = list.querySelector(`[data-match-index="${selectedIndex}"]`) as HTMLElement | null;
    item?.scrollIntoView({ block: "nearest" });
  }, [selectedIndex]);

  let matchIndex = 0;
  const searching = !!query.trim();

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      {/* Same treatment as its sibling `file-search-dialog.tsx`: crisp
       *  transform-free positioning, and no inherited `×` (this app's
       *  overlays close on Escape or a backdrop click). */}
      <DialogContent
        className={cn(
          DIALOG_CRISP_POSITION,
          "flex max-h-[80vh] flex-col gap-0 overflow-hidden p-0",
        )}
        showCloseButton={false}
        onKeyDown={handleKeyDown}
      >
        <DialogTitle className="sr-only">Search in Files</DialogTitle>
        <DialogDescription className="sr-only">Search file contents</DialogDescription>
        <div className="p-3 pb-2 space-y-2 shrink-0">
          <SearchQueryInput
            inputRef={inputRef}
            value={query}
            onChange={setQuery}
            placeholder="Search in files..."
            loading={loading}
          />
          <div className="flex items-center gap-1">
            <SearchToggle
              label="Match case"
              shortcut="Alt+C"
              pressed={caseSensitive}
              onToggle={() => setCaseSensitive((v) => !v)}
            >
              <CaseSensitive className="size-3.5" />
            </SearchToggle>
            <SearchToggle
              label="Use regular expression"
              shortcut="Alt+R"
              pressed={useRegex}
              onToggle={() => setUseRegex((v) => !v)}
            >
              <Regex className="size-3.5" />
            </SearchToggle>
            {results.length > 0 && (
              <span className="ml-2 text-label text-muted-foreground tabular-nums">
                {capped ? "First " : ""}
                {results.length} result{results.length !== 1 ? "s" : ""} in {fileCount} file{fileCount !== 1 ? "s" : ""}
              </span>
            )}
          </div>
        </div>

        <div
          ref={listRef}
          className={cn(
            "flex-1 min-h-0 overflow-y-auto px-1.5 pb-1.5",
            staleListClass(loading),
          )}
        >
          {!searching && (
            <p className="text-label text-muted-foreground text-center py-8">
              Type to search across files
            </p>
          )}
          {searching && !loading && error && (
            <p role="alert" className="px-4 py-8 text-center text-label text-destructive">
              {useRegex ? "Invalid regular expression" : "Search failed"}: {error}
            </p>
          )}
          {searching && !loading && !error && results.length === 0 && (
            <p className="text-label text-muted-foreground text-center py-8">
              No results found
            </p>
          )}
          {grouped.map((group) => {
            const fileMatches = group.matches.map((match) => {
              const idx = matchIndex++;
              return (
                <button
                  key={`${match.file_path}:${match.line_number}:${idx}`}
                  data-match-index={idx}
                  className={cn(
                    "flex w-full items-baseline gap-2 rounded-sm px-2 py-0.5 text-left font-mono text-body-sm",
                    idx === selectedIndex ? "bg-accent" : "hover:bg-accent/50",
                  )}
                  onClick={() => openMatch(match)}
                  onMouseEnter={() => setSelectedIndex(idx)}
                >
                  <span className="shrink-0 w-8 text-right text-muted-foreground/60 tabular-nums">
                    {match.line_number}
                  </span>
                  <span className="min-w-0 truncate">
                    <MatchHighlight
                      text={match.line_content}
                      range={[match.match_start, match.match_end]}
                      className="bg-accent-ember/30 text-inherit rounded-sm px-px"
                    />
                  </span>
                </button>
              );
            });

            return (
              <div key={group.filePath} className="mb-1">
                <div className="flex items-center gap-1.5 px-2 py-1 sticky top-0 bg-popover z-10">
                  <FileCode className="size-3 shrink-0 text-muted-foreground" />
                  <span className="text-label text-muted-foreground truncate">
                    {pathUnderRoot(cwd, group.filePath)}
                  </span>
                  <span className="text-caption text-muted-foreground/50 shrink-0 tabular-nums">
                    ({group.matches.length})
                  </span>
                </div>
                {fileMatches}
              </div>
            );
          })}
          {capped && <ResultCapNotice limit={CONTENT_SEARCH_LIMIT} />}
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

/** A query option button that says whether it is on, to sighted users by
 *  fill and to assistive tech by `aria-pressed`. */
function SearchToggle({
  label,
  shortcut,
  pressed,
  onToggle,
  children,
}: {
  label: string;
  shortcut: string;
  pressed: boolean;
  onToggle: () => void;
  children: React.ReactNode;
}) {
  return (
    <Button
      variant={pressed ? "secondary" : "ghost"}
      size="icon-xs"
      title={`${label} (${shortcut})`}
      aria-label={label}
      aria-keyshortcuts={shortcut}
      aria-pressed={pressed}
      onClick={onToggle}
      className={pressed ? "bg-primary/20 text-primary" : ""}
    >
      {children}
    </Button>
  );
}
