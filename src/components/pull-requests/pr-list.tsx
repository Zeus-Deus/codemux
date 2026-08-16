import { useEffect, useMemo, useRef, useState } from "react";
import { RefreshCw, Search } from "lucide-react";

import { cn } from "@/lib/utils";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { resolveProvider } from "@/lib/source-control";
import { shortAge } from "@/components/workspace/review/review-ui";
import {
  applyPlan,
  freezePlan,
  groupRows,
  matchesPrSearch,
  parsePrSearch,
  planOf,
  rowKey,
  GROUP_LABEL,
  type PlanEntry,
  type PrGroup,
  type PrRow as PrRowData,
} from "@/lib/pr-overview";
import type { PrStateFilter, RootFailure } from "@/lib/pr-overview-query";
import { PrRow } from "./pr-row";

/**
 * Past this many rows the list stops rendering and says so.
 *
 * Not a silent truncation: a list that quietly stops at 200 is a list
 * that lies about what you have. The line at the bottom names the number
 * it is not showing, and the search field above it is how you get to
 * them (numbers, not vibes).
 */
const MAX_RENDERED = 200;

const EMPTY_MOVED: Set<string> = new Set();

const STATE_LABEL: Record<PrStateFilter, string> = {
  open: "Open",
  closed: "Closed",
  all: "All",
};

export interface PrListProps {
  rows: PrRowData[];
  viewerByRoot: Map<string, string | null>;
  /** `projectRoot\0branch` → workspace id, resolved once by the page. */
  workspaceByBranch: Map<string, string>;
  failures: RootFailure[];
  hostCount: number;
  updatedAt: number | null;
  isLoading: boolean;
  selectedKey: string | null;
  stateFilter: PrStateFilter;
  onStateFilter: (state: PrStateFilter) => void;
  onSelect: (row: PrRowData) => void;
  /** ↵ — hand the keyboard to the detail column. */
  onOpenDetail: () => void;
  onRefresh: () => void;
}

export function PrList({
  rows,
  viewerByRoot,
  workspaceByBranch,
  failures,
  hostCount,
  updatedAt,
  isLoading,
  selectedKey,
  stateFilter,
  onStateFilter,
  onSelect,
  onOpenDetail,
  onRefresh,
}: PrListProps) {
  const [query, setQuery] = useState("");
  const [watchingOpen, setWatchingOpen] = useState(false);
  const [listFocused, setListFocused] = useState(false);
  const [pointerInside, setPointerInside] = useState(false);

  const filtered = useMemo(() => {
    const search = parsePrSearch(query);
    return rows.filter((row) => matchesPrSearch(row, search));
  }, [rows, query]);

  const freshGroups = useMemo(
    () => groupRows(filtered, viewerByRoot),
    [filtered, viewerByRoot],
  );
  const freshPlan = useMemo(() => planOf(freshGroups), [freshGroups]);

  // ── Rule 03: polling can update, but not rearrange ──
  //
  // While the pointer is over the list or the keyboard is in it, the
  // order you can see is the order that stays. A poll may change what a
  // row *says* — its checks, its label — but not where it is, because
  // the click you are about to make is aimed at a position.
  const hold = pointerInside || listFocused;
  const snapshot = useRef<PlanEntry[] | null>(null);
  const [applyToken, setApplyToken] = useState(0);

  const { entries, moved } = useMemo(() => {
    if (!hold) return { entries: freshPlan, moved: EMPTY_MOVED };
    return freezePlan(freshPlan, snapshot.current);
    // applyToken is a deliberate re-run trigger: adopting the new order
    // is an action, not a data change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [hold, freshPlan, applyToken]);

  useEffect(() => {
    if (!hold) snapshot.current = freshPlan;
  }, [hold, freshPlan]);

  /** A deliberate action: take whatever the poll wanted, now. */
  const applyPendingOrder = () => {
    snapshot.current = freshPlan;
    setApplyToken((token) => token + 1);
  };

  const groups = useMemo(() => applyPlan(filtered, entries), [filtered, entries]);

  const byId = (id: PrGroup["id"]) => groups.find((g) => g.id === id)?.rows ?? [];
  const review = byId("review");
  const yours = byId("yours");
  const watching = byId("watching");

  // What the keyboard walks: exactly what is on screen, folded groups
  // excluded — an arrow key that lands on a row you can't see is a
  // selection that appears to come from nowhere.
  const navigable = useMemo(
    () => [...review, ...yours, ...(watchingOpen ? watching : [])].slice(0, MAX_RENDERED),
    [review, yours, watching, watchingOpen],
  );

  const total = filtered.length;
  const hidden = Math.max(0, review.length + yours.length + watching.length - MAX_RENDERED);

  const move = (delta: number) => {
    if (navigable.length === 0) return;
    const current = navigable.findIndex((row) => rowKey(row) === selectedKey);
    const next = Math.min(
      Math.max(current < 0 ? 0 : current + delta, 0),
      navigable.length - 1,
    );
    applyPendingOrder();
    onSelect(navigable[next]);
  };

  const onKeyDown = (event: React.KeyboardEvent) => {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      move(1);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      move(-1);
    } else if (event.key === "Enter") {
      event.preventDefault();
      onOpenDetail();
    }
  };

  const select = (row: PrRowData) => {
    // Choosing a row is deliberate: the order the poll has been holding
    // applies now, while you are looking at the row you just picked.
    applyPendingOrder();
    onSelect(row);
  };

  const renderRow = (row: PrRowData, dense = false) => {
    const key = rowKey(row);
    const branchKey = row.head_branch ? `${row.projectRoot}\0${row.head_branch}` : "";
    return (
      <PrRow
        key={key}
        row={row}
        provider={resolveProvider(row.providerKind)}
        selected={key === selectedKey}
        focused={key === selectedKey && listFocused}
        moved={moved.has(key)}
        existingWorkspaceId={workspaceByBranch.get(branchKey) ?? null}
        dense={dense}
        onSelect={() => select(row)}
      />
    );
  };

  const age = updatedAt ? shortAge(Date.now() - updatedAt) : null;

  return (
    <div className="flex h-full min-h-0 w-full flex-col" data-testid="pr-list">
      <div className="flex shrink-0 items-center gap-2 border-b border-border/40 px-3 py-2">
        <span className="text-[12.5px] font-semibold text-foreground">Pull requests</span>
        <span className="flex-1" />
        {age && (
          <span className="text-[10.5px] text-muted-foreground" data-testid="pr-list-age">
            {age} ago
          </span>
        )}
        <button
          type="button"
          aria-label="Refresh pull requests"
          data-testid="pr-list-refresh"
          onClick={() => {
            applyPendingOrder();
            onRefresh();
          }}
          className="flex size-5 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-accent/50 hover:text-foreground"
        >
          <RefreshCw className={cn("size-3", isLoading && "animate-spin")} />
        </button>
      </div>

      <div className="flex shrink-0 items-center gap-1.5 border-b border-border/40 px-3 py-2">
        <span className="flex flex-1 items-center gap-1.5 rounded-[7px] bg-muted/40 px-2 py-1">
          <Search className="size-3 shrink-0 text-muted-foreground" />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search, or is:draft ci:failing"
            data-testid="pr-search"
            className="min-w-0 flex-1 bg-transparent text-[11px] text-foreground outline-none placeholder:text-muted-foreground"
          />
        </span>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              data-testid="pr-state-filter"
              className="flex shrink-0 items-center gap-1 rounded-[7px] bg-muted/40 px-2 py-1 text-[11px] text-foreground/90 transition-colors hover:bg-accent/50"
            >
              {STATE_LABEL[stateFilter]}
              <span className="text-[9px] text-muted-foreground">▾</span>
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="min-w-28">
            {(["open", "closed", "all"] as PrStateFilter[]).map((state) => (
              <DropdownMenuItem key={state} onSelect={() => onStateFilter(state)}>
                {STATE_LABEL[state]}
              </DropdownMenuItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
      </div>

      <div
        role="listbox"
        aria-label="Pull requests"
        tabIndex={0}
        data-testid="pr-list-rows"
        data-held={hold}
        className="min-h-0 flex-1 overflow-y-auto outline-none"
        onKeyDown={onKeyDown}
        onFocus={() => setListFocused(true)}
        onBlur={() => setListFocused(false)}
        onMouseEnter={() => setPointerInside(true)}
        onMouseLeave={() => setPointerInside(false)}
      >
        {total === 0 ? (
          <EmptyList
            hasRows={rows.length > 0}
            hasHosts={hostCount > 0}
            query={query}
            onClearQuery={() => setQuery("")}
          />
        ) : (
          <>
            {review.length > 0 && (
              <>
                <GroupHeader id="review" count={review.length} />
                {review.map((row) => renderRow(row))}
              </>
            )}

            {yours.length > 0 && (
              <>
                <GroupHeader id="yours" count={yours.length} />
                {yours.map((row) => renderRow(row))}
              </>
            )}

            {watching.length > 0 && (
              <>
                <button
                  type="button"
                  data-testid="pr-group-watching-toggle"
                  aria-expanded={watchingOpen}
                  onClick={() => setWatchingOpen((open) => !open)}
                  className="mt-2 flex w-full items-center gap-1.5 border-t border-border/40 px-3 py-1.5 text-left"
                >
                  <span className="text-[10px] text-muted-foreground">
                    {watchingOpen ? "▾" : "▸"}
                  </span>
                  <span className="font-mono text-[9.5px] font-semibold uppercase tracking-[0.07em] text-muted-foreground">
                    {GROUP_LABEL.watching}
                  </span>
                  <span className="flex-1" />
                  <span className="font-mono text-[9.5px] text-muted-foreground">
                    {watching.length}
                  </span>
                </button>
                {watchingOpen ? (
                  watching.map((row) => renderRow(row, true))
                ) : (
                  <p className="px-3 pb-1 text-[10.5px] leading-relaxed text-muted-foreground">
                    Repositories you have open but aren't involved in.
                  </p>
                )}
              </>
            )}

            {hidden > 0 && (
              <p className="px-3 py-2 text-[10.5px] text-muted-foreground">
                Showing the first {MAX_RENDERED} of {review.length + yours.length + watching.length}.
                Narrow it with the search field above.
              </p>
            )}
          </>
        )}
      </div>

      <div className="flex shrink-0 items-center gap-2 border-t border-border/40 px-3 py-1.5">
        <span className="min-w-0 flex-1 truncate text-[10.5px] text-muted-foreground">
          {hostCount} {hostCount === 1 ? "repository" : "repositories"}
          {failures.length > 0 && (
            <>
              {" · "}
              <span
                className="text-status-working"
                data-testid="pr-list-failures"
                title={failures.map((f) => `${f.root.name}: ${f.message}`).join("\n")}
              >
                {failures.length} unreachable
              </span>
            </>
          )}
        </span>
        <span className="shrink-0 font-mono text-[9.5px] text-muted-foreground">
          ↑↓ move · ↵ open
        </span>
      </div>
    </div>
  );
}

function GroupHeader({ id, count }: { id: "review" | "yours"; count: number }) {
  return (
    <div
      className="flex items-center gap-1.5 px-3 pb-1 pt-2.5"
      data-testid={`pr-group-${id}`}
    >
      {id === "review" && (
        <span aria-hidden className="size-1.5 shrink-0 rounded-full bg-accent-ember" />
      )}
      <span className="font-mono text-[9.5px] font-semibold uppercase tracking-[0.07em] text-foreground/80">
        {GROUP_LABEL[id]}
      </span>
      <span className="h-px flex-1 bg-border/60" />
      <span className="font-mono text-[9.5px] text-muted-foreground">{count}</span>
    </div>
  );
}

/**
 * Five ways to have nothing to show, and each one gets the sentence
 * that fits it plus the thing to do next (rule 05).
 */
function EmptyList({
  hasRows,
  hasHosts,
  query,
  onClearQuery,
}: {
  hasRows: boolean;
  hasHosts: boolean;
  query: string;
  onClearQuery: () => void;
}) {
  if (query.trim() && hasRows) {
    return (
      <div className="flex flex-col items-start gap-2 px-3 py-8">
        <p className="text-xs text-foreground">Nothing matches that search.</p>
        <p className="text-[11px] text-muted-foreground">
          Tokens are <span className="font-mono">is:draft</span>,{" "}
          <span className="font-mono">ci:failing</span> and{" "}
          <span className="font-mono">author:name</span>; everything else is title text.
        </p>
        <button
          type="button"
          onClick={onClearQuery}
          className="rounded-md bg-card px-2.5 py-1 text-[11px] text-foreground/90 hover:bg-accent/50"
        >
          Clear the search
        </button>
      </div>
    );
  }

  if (!hasHosts) {
    return (
      <div className="flex flex-col items-start gap-2 px-3 py-8">
        <p className="text-xs text-foreground">No projects open.</p>
        <p className="text-[11px] leading-relaxed text-muted-foreground">
          This page lists pull requests across the projects you have open. Open one and
          its repository shows up here.
        </p>
      </div>
    );
  }

  return (
    <div className="flex flex-col items-start gap-2 px-3 py-8">
      <p className="text-xs text-foreground">No open pull requests.</p>
      <p className="text-[11px] leading-relaxed text-muted-foreground">
        Nothing is open on the projects you have here. If you expected some, check that
        the host CLI is signed in under Settings ▸ Source Control.
      </p>
    </div>
  );
}
