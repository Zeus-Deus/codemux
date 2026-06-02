import { useState, useEffect, useRef, useCallback, useMemo } from "react";
import { cn } from "@/lib/utils";
import { Search, Loader2 } from "lucide-react";
import { PrStatusIcon, type PrStatusState } from "@/components/github/pr-status-icon";
import { listPullRequests, getGithubPrByPath } from "@/tauri/commands";
import type { PullRequestInfo } from "@/tauri/types";

/**
 * Pull-request picker, mirroring `IssuePickerPanel`.
 *
 * Renders a search input plus a flat list of recent PRs from the
 * given repo path, fetched via the cached `gh pr list` wrapper. The
 * row treatment mirrors the issues picker: state-aware icon at the
 * left, `#N` number, title, and a hover-revealed "Link ↵" affordance
 * matching the rest of the picker family.
 *
 * The status icon + color reuse `PrStatusIcon` — the same component
 * used for the sidebar PR pill — so a row reads identically here and
 * in the sidebar: merged → purple GitMerge, open → green
 * GitPullRequest, draft → muted GitPullRequestDraft, closed → red
 * GitPullRequestClosed.
 */

function fuzzyMatch(text: string, query: string): boolean {
  const lower = text.toLowerCase();
  const q = query.toLowerCase();
  let qi = 0;
  for (let i = 0; i < lower.length && qi < q.length; i++) {
    if (lower[i] === q[qi]) qi++;
  }
  return qi === q.length;
}

/** Collapse the `state` string + `is_draft` flag into the single
 *  state token `PrStatusIcon` understands. A draft only matters while
 *  the PR is still open; merged/closed take precedence. */
function effectivePrState(pr: PullRequestInfo): PrStatusState {
  const upper = (pr.state ?? "OPEN").toUpperCase();
  if (upper === "MERGED") return "merged";
  if (upper === "CLOSED") return "closed";
  return pr.is_draft ? "draft" : "open";
}

function PrRow({
  pr,
  isFocused,
  onSelect,
  onMouseEnter,
}: {
  pr: PullRequestInfo;
  isFocused: boolean;
  onSelect: () => void;
  onMouseEnter: () => void;
}) {
  return (
    <div
      role="option"
      aria-selected={isFocused}
      data-state={pr.state.toLowerCase()}
      data-draft={pr.is_draft || undefined}
      className={cn(
        "group/row flex items-center gap-2 px-2 py-1.5 cursor-pointer rounded-sm transition-colors",
        isFocused ? "bg-accent" : "hover:bg-accent/50",
      )}
      onClick={onSelect}
      onMouseEnter={onMouseEnter}
    >
      <PrStatusIcon state={effectivePrState(pr)} className="shrink-0" />
      <span className="text-muted-foreground text-[0.75rem] shrink-0 font-mono tabular-nums">
        #{pr.number}
      </span>
      <span className="text-[0.8rem] text-foreground truncate min-w-0 flex-1">
        {pr.title}
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

export function PrPickerPanel({
  projectPath,
  open,
  onSelect,
  onClose,
}: {
  projectPath: string;
  open: boolean;
  onSelect: (pr: PullRequestInfo) => void;
  onClose: () => void;
}) {
  // Default the list-state filter to "open" — that's overwhelmingly
  // what users want to triage. The search box can dive into closed /
  // merged via fuzzy match if they pulled in earlier; numeric direct
  // input bypasses state entirely.
  const [prs, setPrs] = useState<PullRequestInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [focusIndex, setFocusIndex] = useState(0);
  // Direct-fetch result for numeric input (e.g. typing `42` resolves
  // PR #42 even if it isn't in the open list). Mirrors the issue
  // picker's server-search model but with a single result row.
  const [directHit, setDirectHit] = useState<PullRequestInfo | null>(null);
  const [directSearching, setDirectSearching] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const debounceRef = useRef<ReturnType<typeof setTimeout>>(undefined);

  useEffect(() => {
    if (!open || !projectPath) return;
    let cancelled = false;

    setLoading(true);
    setError(null);
    setPrs([]);
    setSearch("");
    setFocusIndex(0);
    setDirectHit(null);

    listPullRequests(projectPath, "open")
      .then((result) => {
        if (cancelled) return;
        // Sort by updatedAt desc — same convention as IssuePicker.
        const sorted = [...result].sort((a, b) => {
          const at = a.updated_at ?? "";
          const bt = b.updated_at ?? "";
          if (at === bt) return a.number - b.number;
          return at < bt ? 1 : -1;
        });
        setPrs(sorted);
        setLoading(false);
      })
      .catch((err) => {
        if (cancelled) return;
        setError(String(err));
        setLoading(false);
      });

    requestAnimationFrame(() => {
      inputRef.current?.focus();
    });

    return () => { cancelled = true; };
  }, [open, projectPath]);

  const filteredPrs = useMemo(() => {
    if (!search.trim()) return prs;
    const q = search.replace(/^[#!]/, "");
    return prs.filter(
      (p) => String(p.number).includes(q) || fuzzyMatch(p.title, q),
    );
  }, [prs, search]);

  const displayPrs = useMemo(() => {
    if (directHit && filteredPrs.length === 0) return [directHit];
    if (directHit && search.trim()) {
      const localNumbers = new Set(filteredPrs.map((p) => p.number));
      if (localNumbers.has(directHit.number)) return filteredPrs;
      return [directHit, ...filteredPrs];
    }
    return filteredPrs;
  }, [filteredPrs, directHit, search]);

  useEffect(() => {
    setFocusIndex(0);
  }, [displayPrs.length]);

  const triggerDirectFetch = useCallback(
    (query: string) => {
      if (debounceRef.current) clearTimeout(debounceRef.current);
      const stripped = query.replace(/^[#!]/, "").trim();
      if (!/^\d+$/.test(stripped)) {
        setDirectHit(null);
        setDirectSearching(false);
        return;
      }
      const num = Number.parseInt(stripped, 10);
      debounceRef.current = setTimeout(() => {
        setDirectSearching(true);
        getGithubPrByPath(projectPath, num)
          .then((pr) => {
            setDirectHit(pr);
            setDirectSearching(false);
          })
          .catch(() => setDirectSearching(false));
      }, 300);
    },
    [projectPath],
  );

  const handleSearchChange = (value: string) => {
    setSearch(value);
    triggerDirectFetch(value);
  };

  const handleSelectPr = useCallback(
    (pr: PullRequestInfo) => {
      onSelect(pr);
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
      setFocusIndex((i) => Math.min(i + 1, displayPrs.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setFocusIndex((i) => Math.max(i - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (displayPrs[focusIndex]) {
        handleSelectPr(displayPrs[focusIndex]);
      }
    }
  };

  return (
    <div onKeyDown={handleKeyDown} data-testid="pr-picker-panel">
      <div className="flex items-center justify-between px-3 pt-2.5 pb-1">
        <span className="text-[0.7rem] font-semibold uppercase tracking-wider text-muted-foreground">
          Open Pull Requests
        </span>
      </div>

      <div className="px-2 pb-1.5">
        <div className="flex items-center gap-1.5 rounded-md border border-border bg-muted/50 px-2 h-7">
          <Search className="size-3 text-muted-foreground shrink-0" />
          <input
            ref={inputRef}
            value={search}
            onChange={(e) => handleSearchChange(e.target.value)}
            placeholder="Search PRs… (or type a number)"
            className="flex-1 bg-transparent text-xs text-foreground placeholder:text-muted-foreground/50 outline-none min-w-0"
          />
        </div>
      </div>

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
                ? "Connect GitHub to link PRs"
                : error.includes("not installed")
                  ? "Install GitHub CLI (gh) to link PRs"
                  : "Failed to load PRs"}
            </p>
          </div>
        ) : displayPrs.length === 0 ? (
          <div className="px-3 py-4 text-center text-xs text-muted-foreground">
            {search.trim() ? "No PRs found" : "No open PRs"}
          </div>
        ) : (
          displayPrs.map((pr, idx) => (
            <PrRow
              key={pr.number}
              pr={pr}
              isFocused={idx === focusIndex}
              onSelect={() => handleSelectPr(pr)}
              onMouseEnter={() => setFocusIndex(idx)}
            />
          ))
        )}
        {directSearching && displayPrs.length > 0 && (
          <div className="flex items-center justify-center gap-1.5 py-2 text-muted-foreground/60 text-[0.65rem]">
            <Loader2 className="h-3 w-3 animate-spin" />
            Searching…
          </div>
        )}
      </div>
    </div>
  );
}
