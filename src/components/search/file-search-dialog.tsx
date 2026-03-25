import { useState, useEffect, useRef, useCallback } from "react";
import {
  Dialog,
  DialogContent,
  DialogTitle,
  DialogDescription,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Loader2, FileText } from "lucide-react";
import { useUIStore } from "@/stores/ui-store";
import { useActiveWorkspace } from "@/stores/app-store";
import { searchFileNames, detectEditors, openInEditor } from "@/tauri/commands";

export function FileSearchDialog() {
  const open = useUIStore((s) => s.showFileSearch);
  const setOpen = useUIStore((s) => s.setShowFileSearch);
  const workspace = useActiveWorkspace();
  const cwd = workspace?.cwd ?? "";

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
      try {
        const editors = await detectEditors();
        if (editors.length > 0) {
          const fullPath = filePath.startsWith("/") ? filePath : `${cwd}/${filePath}`;
          await openInEditor(editors[0].id, fullPath);
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
      <DialogContent className="max-w-[560px] p-0 gap-0" onKeyDown={handleKeyDown}>
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
            const parts = filePath.split("/");
            const fileName = parts.pop() || filePath;
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
                <FileText className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
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
