import { useState, useEffect, useRef, useCallback, useMemo } from "react";
import { cn } from "@/lib/utils";
import { CircleDot, CircleCheck, Search, Loader2 } from "lucide-react";
import { listGithubIssues, listGithubIssuesByPath } from "@/tauri/commands";
import type { GitHubIssue } from "@/tauri/types";

function fuzzyMatch(text: string, query: string): boolean {
  const lower = text.toLowerCase();
  const q = query.toLowerCase();
  let qi = 0;
  for (let i = 0; i < lower.length && qi < q.length; i++) {
    if (lower[i] === q[qi]) qi++;
  }
  return qi === q.length;
}

function IssueRow({
  issue,
  isFocused,
  onSelect,
  onMouseEnter,
}: {
  issue: GitHubIssue;
  isFocused: boolean;
  onSelect: () => void;
  onMouseEnter: () => void;
}) {
  return (
    <div
      role="option"
      aria-selected={isFocused}
      className={cn(
        "group/row flex items-center gap-2 px-2 py-1.5 cursor-pointer rounded-sm transition-colors",
        isFocused ? "bg-accent" : "hover:bg-accent/50",
      )}
      onClick={onSelect}
      onMouseEnter={onMouseEnter}
    >
      {issue.state === "Open" ? (
        <CircleDot className="size-3.5 shrink-0 text-success" />
      ) : (
        <CircleCheck className="size-3.5 shrink-0 text-muted-foreground" />
      )}
      <span className="text-muted-foreground text-[0.75rem] shrink-0 font-mono tabular-nums">
        #{issue.number}
      </span>
      <span className="text-[0.8rem] text-foreground truncate min-w-0 flex-1">
        {issue.title}
      </span>
      <span
        className={cn(
          "text-muted-foreground text-[0.7rem] shrink-0 select-none transition-opacity",
          isFocused ? "opacity-100" : "opacity-0 group-hover/row:opacity-100",
        )}
      >
        Link ↵
      </span>
    </div>
  );
}

function SkeletonRow() {
  return (
    <div className="flex items-center gap-2 px-2 py-1.5">
      <div className="size-3.5 rounded-full bg-muted animate-pulse shrink-0" />
      <div className="h-3 w-8 rounded bg-muted animate-pulse shrink-0" />
      <div className="h-3 flex-1 rounded bg-muted animate-pulse" />
    </div>
  );
}

/** Inner panel — exported for testing without Radix portal issues */
export function IssuePickerPanel({
  workspaceId,
  projectPath,
  open,
  onSelect,
  onClose,
}: {
  workspaceId?: string;
  projectPath?: string;
  open: boolean;
  onSelect: (issue: GitHubIssue) => void;
  onClose: () => void;
}) {
  const [issues, setIssues] = useState<GitHubIssue[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [focusIndex, setFocusIndex] = useState(0);
  const [serverResults, setServerResults] = useState<GitHubIssue[] | null>(null);
  const [serverSearching, setServerSearching] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const debounceRef = useRef<ReturnType<typeof setTimeout>>(undefined);

  const fetchIssues = useCallback(
    (search?: string) => {
      if (workspaceId) return listGithubIssues(workspaceId, search);
      if (projectPath) return listGithubIssuesByPath(projectPath, search);
      return Promise.reject("No workspace or project path");
    },
    [workspaceId, projectPath],
  );

  useEffect(() => {
    if (!open || (!workspaceId && !projectPath)) return;
    let cancelled = false;

    setLoading(true);
    setError(null);
    setIssues([]);
    setSearch("");
    setFocusIndex(0);
    setServerResults(null);

    fetchIssues()
      .then((result) => {
        if (!cancelled) {
          setIssues(result);
          setLoading(false);
        }
      })
      .catch((err) => {
        if (!cancelled) {
          setError(String(err));
          setLoading(false);
        }
      });

    // Focus input after panel renders — use rAF to ensure DOM is ready
    requestAnimationFrame(() => {
      inputRef.current?.focus();
    });

    return () => { cancelled = true; };
  }, [open, workspaceId, projectPath, fetchIssues]);

  const filteredIssues = useMemo(() => {
    if (!search.trim()) return issues;
    const q = search.replace(/^#/, "");
    return issues.filter(
      (i) => String(i.number).includes(q) || fuzzyMatch(i.title, q),
    );
  }, [issues, search]);

  const displayIssues = useMemo(() => {
    if (serverResults && filteredIssues.length === 0) return serverResults;
    if (serverResults && search.trim()) {
      const localNumbers = new Set(filteredIssues.map((i) => i.number));
      const serverOnly = serverResults.filter((i) => !localNumbers.has(i.number));
      return [...filteredIssues, ...serverOnly];
    }
    return filteredIssues;
  }, [filteredIssues, serverResults, search]);

  useEffect(() => {
    setFocusIndex(0);
  }, [displayIssues.length]);

  const triggerServerSearch = useCallback(
    (query: string) => {
      if (debounceRef.current) clearTimeout(debounceRef.current);
      if (!query.trim()) {
        setServerResults(null);
        setServerSearching(false);
        return;
      }
      debounceRef.current = setTimeout(() => {
        setServerSearching(true);
        fetchIssues(query.replace(/^#/, ""))
          .then((results) => {
            setServerResults(results);
            setServerSearching(false);
          })
          .catch(() => setServerSearching(false));
      }, 300);
    },
    [fetchIssues],
  );

  const handleSearchChange = (value: string) => {
    setSearch(value);
    triggerServerSearch(value);
  };

  const handleSelectIssue = useCallback(
    (issue: GitHubIssue) => {
      onSelect(issue);
      onClose();
    },
    [onSelect, onClose],
  );

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onClose();
      return;
    }
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setFocusIndex((i) => Math.min(i + 1, displayIssues.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setFocusIndex((i) => Math.max(i - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (displayIssues[focusIndex]) {
        handleSelectIssue(displayIssues[focusIndex]);
      }
    }
  };

  return (
    <div onKeyDown={handleKeyDown} data-testid="issue-picker-panel">
      {/* Header */}
      <div className="flex items-center justify-between px-3 pt-2.5 pb-1">
        <span className="text-[0.7rem] font-semibold uppercase tracking-wider text-muted-foreground">
          Open Issues
        </span>
      </div>

      {/* Search */}
      <div className="px-2 pb-1.5">
        <div className="flex items-center gap-1.5 rounded-md border border-border bg-muted/50 px-2 h-7">
          <Search className="size-3 text-muted-foreground shrink-0" />
          <input
            ref={inputRef}
            value={search}
            onChange={(e) => handleSearchChange(e.target.value)}
            placeholder="Search issues..."
            className="flex-1 bg-transparent text-xs text-foreground placeholder:text-muted-foreground/50 outline-none min-w-0"
          />
        </div>
      </div>

      {/* Issue list */}
      <div className="max-h-[280px] overflow-y-auto px-1 pb-1" role="listbox">
        {loading ? (
          <>
            <SkeletonRow />
            <SkeletonRow />
            <SkeletonRow />
            <SkeletonRow />
          </>
        ) : error ? (
          <div className="px-3 py-4 text-center">
            <p className="text-xs text-muted-foreground">
              {error.includes("not authenticated") || error.includes("auth")
                ? "Connect GitHub to link issues"
                : error.includes("not installed")
                  ? "Install GitHub CLI (gh) to link issues"
                  : "Failed to load issues"}
            </p>
          </div>
        ) : displayIssues.length === 0 ? (
          <div className="px-3 py-4 text-center text-xs text-muted-foreground">
            {search.trim() ? "No issues found" : "No open issues"}
          </div>
        ) : (
          displayIssues.map((issue, idx) => (
            <IssueRow
              key={issue.number}
              issue={issue}
              isFocused={idx === focusIndex}
              onSelect={() => handleSelectIssue(issue)}
              onMouseEnter={() => setFocusIndex(idx)}
            />
          ))
        )}
        {serverSearching && displayIssues.length > 0 && (
          <div className="flex items-center justify-center gap-1.5 py-2 text-muted-foreground/60 text-[0.65rem]">
            <Loader2 className="h-3 w-3 animate-spin" />
            Searching...
          </div>
        )}
      </div>
    </div>
  );
}
