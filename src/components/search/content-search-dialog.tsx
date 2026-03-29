import { useState, useEffect, useRef, useCallback, useMemo } from "react";
import {
  Dialog,
  DialogContent,
  DialogTitle,
  DialogDescription,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { Loader2, FileCode, CaseSensitive, Regex } from "lucide-react";
import { useUIStore } from "@/stores/ui-store";
import { useActiveWorkspace } from "@/stores/app-store";
import { searchInFiles } from "@/tauri/commands";
import { openEditorTab } from "@/lib/open-editor-tab";
import type { SearchResult } from "@/tauri/types";

interface GroupedResults {
  filePath: string;
  matches: SearchResult[];
}

export function ContentSearchDialog() {
  const open = useUIStore((s) => s.showContentSearch);
  const setOpen = useUIStore((s) => s.setShowContentSearch);
  const workspace = useActiveWorkspace();
  const cwd = workspace?.cwd ?? "";

  const [query, setQuery] = useState("");
  const [results, setResults] = useState<SearchResult[]>([]);
  const [loading, setLoading] = useState(false);
  const [caseSensitive, setCaseSensitive] = useState(false);
  const [useRegex, setUseRegex] = useState(false);
  const [selectedIndex, setSelectedIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  // Reset on open
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
      searchInFiles(cwd, query.trim(), useRegex, caseSensitive, 100)
        .then((res) => {
          setResults(res);
          setSelectedIndex(0);
        })
        .catch(() => setResults([]))
        .finally(() => setLoading(false));
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

  // Flat list of all matches for arrow key navigation
  const flatMatches = useMemo(() => results, [results]);

  const openFile = useCallback(
    async (filePath: string) => {
      if (!workspace) return;
      try {
        const fullPath = filePath.startsWith("/") ? filePath : `${cwd}/${filePath}`;
        await openEditorTab(workspace.workspace_id, workspace.tabs, fullPath);
      } catch (err) {
        console.error("Failed to open file:", err);
      }
      setOpen(false);
    },
    [workspace, cwd, setOpen],
  );

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setSelectedIndex((i) => Math.min(i + 1, flatMatches.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setSelectedIndex((i) => Math.max(i - 1, 0));
    } else if (e.key === "Enter" && flatMatches[selectedIndex]) {
      e.preventDefault();
      openFile(flatMatches[selectedIndex].file_path);
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

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogContent className="max-w-[640px] p-0 gap-0 max-h-[80vh] flex flex-col" onKeyDown={handleKeyDown}>
        <DialogTitle className="sr-only">Search in Files</DialogTitle>
        <DialogDescription className="sr-only">Search file contents</DialogDescription>
        <div className="p-3 pb-2 space-y-2 shrink-0">
          <Input
            ref={inputRef}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search in files..."
            className="h-9 text-sm"
          />
          <div className="flex items-center gap-1">
            <Button
              variant={caseSensitive ? "secondary" : "ghost"}
              size="icon-xs"
              title="Case Sensitive"
              onClick={() => setCaseSensitive(!caseSensitive)}
              className={caseSensitive ? "bg-primary/20 text-primary" : ""}
            >
              <CaseSensitive className="h-3.5 w-3.5" />
            </Button>
            <Button
              variant={useRegex ? "secondary" : "ghost"}
              size="icon-xs"
              title="Regular Expression"
              onClick={() => setUseRegex(!useRegex)}
              className={useRegex ? "bg-primary/20 text-primary" : ""}
            >
              <Regex className="h-3.5 w-3.5" />
            </Button>
            {results.length > 0 && (
              <span className="ml-2 text-[11px] text-muted-foreground">
                {results.length} result{results.length !== 1 ? "s" : ""} in {fileCount} file{fileCount !== 1 ? "s" : ""}
              </span>
            )}
          </div>
        </div>

        <div ref={listRef} className="flex-1 min-h-0 overflow-y-auto px-1.5 pb-1.5">
          {!query.trim() && (
            <p className="text-xs text-muted-foreground text-center py-8">
              Type to search across files
            </p>
          )}
          {query.trim() && loading && (
            <div className="flex justify-center py-8">
              <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
            </div>
          )}
          {query.trim() && !loading && results.length === 0 && (
            <p className="text-xs text-muted-foreground text-center py-8">
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
                  className={`flex w-full items-baseline gap-2 rounded px-2 py-0.5 text-left font-mono text-[12px] ${
                    idx === selectedIndex ? "bg-accent" : "hover:bg-accent/50"
                  }`}
                  onClick={() => openFile(match.file_path)}
                  onMouseEnter={() => setSelectedIndex(idx)}
                >
                  <span className="shrink-0 w-8 text-right text-muted-foreground/60 tabular-nums">
                    {match.line_number}
                  </span>
                  <span className="min-w-0 truncate">
                    <HighlightedLine
                      content={match.line_content}
                      matchStart={match.match_start}
                      matchEnd={match.match_end}
                    />
                  </span>
                </button>
              );
            });

            return (
              <div key={group.filePath} className="mb-1">
                <div className="flex items-center gap-1.5 px-2 py-1 sticky top-0 bg-card z-10">
                  <FileCode className="h-3 w-3 shrink-0 text-muted-foreground" />
                  <span className="text-xs text-muted-foreground truncate">{group.filePath}</span>
                  <span className="text-[10px] text-muted-foreground/50 shrink-0">
                    ({group.matches.length})
                  </span>
                </div>
                {fileMatches}
              </div>
            );
          })}
        </div>
      </DialogContent>
    </Dialog>
  );
}

function HighlightedLine({
  content,
  matchStart,
  matchEnd,
}: {
  content: string;
  matchStart: number;
  matchEnd: number;
}) {
  if (matchStart < 0 || matchEnd <= matchStart || matchStart >= content.length) {
    return <>{content}</>;
  }
  return (
    <>
      {content.substring(0, matchStart)}
      <mark className="bg-yellow-500/30 text-inherit rounded-sm px-px">
        {content.substring(matchStart, matchEnd)}
      </mark>
      {content.substring(matchEnd)}
    </>
  );
}
