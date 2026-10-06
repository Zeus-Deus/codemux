import { useEffect, useMemo, useRef, useState } from "react";
import {
  ChevronDown,
  ChevronRight,
  Eye,
  ListFilter,
  PenLine,
  RefreshCw,
  Search,
  Users,
  type LucideIcon,
} from "lucide-react";

import { cn } from "@/lib/utils";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { resolveProvider } from "@/lib/source-control";
import {
  btnCard,
  shortAge,
  tzBody,
  tzEyebrow,
  tzMetaNum,
} from "@/components/workspace/review/review-ui";
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

/** `14:12` — the user's own clock, because the only question it answers
 *  is "when can I look again". */
export function clockTime(epochMs: number): string {
  return new Date(epochMs).toLocaleTimeString(undefined, {
    hour: "2-digit",
    minute: "2-digit",
  });
}

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
  /** Rows on screen came from the last session's snapshot. */
  carried?: boolean;
  /** When those rows were fetched — the age the header and the strip
   *  both print, because it is the age of what you are looking at. */
  carriedAt?: number | null;
  /** Every root's refresh failed. Partial failure stays in the footer;
   *  this is what raises the strip. */
  allRootsFailed?: boolean;
  /** The refresh cycle settled with nothing to show for itself. */
  refreshFailed?: boolean;
  /** Epoch ms the host's budget refills at, when a refusal for exceeding
   *  it has paused the polling; 0 when nothing is paused. It gets its
   *  own sentence because it is the one failure with an answer to "what
   *  do I do about it" that isn't "press the button again". */
  rateLimitedUntil?: number;
  isLoading: boolean;
  selectedKey: string | null;
  stateFilter: PrStateFilter;
  onStateFilter: (state: PrStateFilter) => void;
  /** ↑↓ — move the list's cursor to a row. */
  onSelect: (row: PrRowData) => void;
  /** A click on a row. Defaults to `onSelect`. */
  onOpen?: (row: PrRowData) => void;
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
  carried = false,
  carriedAt = null,
  allRootsFailed = false,
  refreshFailed = false,
  rateLimitedUntil = 0,
  isLoading,
  selectedKey,
  stateFilter,
  onStateFilter,
  onSelect,
  onOpen = onSelect,
  onOpenDetail,
  onRefresh,
}: PrListProps) {
  const [query, setQuery] = useState("");
  // null = the user hasn't chosen; the fold then follows the content:
  // Watching stays folded while Needs-your-review / Yours have rows,
  // but when it is the only populated group a folded header would be
  // the entire page — so it opens itself. An explicit toggle wins.
  const [watchingChoice, setWatchingChoice] = useState<boolean | null>(null);
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
  const watchingOpen =
    watchingChoice ?? (review.length === 0 && yours.length === 0 && watching.length > 0);

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
    onOpen(row);
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

  // What the header claims, and it claims only what is true. Carried
  // rows are hours old however recently the app started, so they say
  // "as of 2h" rather than the "0s" a naive mount timestamp would print
  // — the difference between stale-and-labelled and simply wrong.
  const dataAt = carried ? carriedAt : updatedAt;
  const age = dataAt ? shortAge(Date.now() - dataAt) : null;

  // The strip, not the footer, when the whole cycle failed or when what
  // you are reading is carried and the refresh behind it didn't land.
  // Partial failure keeps the quieter footer treatment: the other
  // repositories are current, and one line saying so is proportionate.
  // A budget that is spent raises the strip on its own, without waiting
  // for every root to have been refused individually: the polling is
  // already paused at that point, and a page that has quietly stopped
  // refreshing while looking perfectly normal is the one outcome worse
  // than saying so.
  const showStaleStrip =
    rateLimitedUntil > 0 || (total > 0 && (allRootsFailed || (carried && refreshFailed)));

  return (
    <div className="flex h-full min-h-0 w-full flex-col" data-testid="pr-list">
      <div className="thin-scrollbar min-h-0 flex-1 overflow-y-auto [scrollbar-gutter:stable]">
        <div className="mx-auto flex w-full max-w-5xl flex-col gap-4 px-5 pb-12 pt-3 sm:px-6">
          <div className="flex flex-wrap items-center gap-2">
            <label className="flex h-8 min-w-0 flex-1 basis-56 items-center gap-2 rounded-md border border-border bg-background px-2.5 transition-colors duration-150 focus-within:border-ring/60 dark:bg-input/30">
              <Search className="size-3.5 shrink-0 text-muted-foreground" />
              <input
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="Search pull requests, or is:draft ci:failing"
                aria-label="Search pull requests"
                data-testid="pr-search"
                className={cn(
                  "min-w-0 flex-1 bg-transparent text-foreground outline-none placeholder:text-muted-foreground",
                  tzBody,
                )}
              />
            </label>
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <button
                  type="button"
                  data-testid="pr-state-filter"
                  className={cn(TOOLBAR_BUTTON, "gap-1.5 px-2.5")}
                >
                  <ListFilter className="size-3.5 text-muted-foreground" />
                  {STATE_LABEL[stateFilter]}
                  <ChevronDown className="size-3 text-muted-foreground" />
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
            <button
              type="button"
              aria-label="Refresh pull requests"
              title="Refresh"
              data-testid="pr-list-refresh"
              disabled={rateLimitedUntil > 0}
              onClick={() => {
                applyPendingOrder();
                onRefresh();
              }}
              className={cn(TOOLBAR_BUTTON, "w-8 justify-center")}
            >
              <RefreshCw className={cn("size-3.5", isLoading && "animate-spin")} />
            </button>
          </div>

          {showStaleStrip && (
            <StaleStrip
              age={age}
              hasRows={total > 0}
              resumesAt={rateLimitedUntil > 0 ? rateLimitedUntil : null}
              onRetry={onRefresh}
            />
          )}

          <div
            role="listbox"
            aria-label="Pull requests"
            tabIndex={0}
            data-testid="pr-list-rows"
            data-held={hold}
            className="flex flex-col gap-4 rounded-md outline-none focus-visible:ring-1 focus-visible:ring-ring/40"
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
                isLoading={isLoading}
                unanswered={rateLimitedUntil > 0 && allRootsFailed}
                query={query}
                onClearQuery={() => setQuery("")}
              />
            ) : (
              <>
                {review.length > 0 && (
                  <section className="flex flex-col gap-0.5">
                    <GroupHeader id="review" count={review.length} />
                    {review.map((row) => renderRow(row))}
                  </section>
                )}

                {yours.length > 0 && (
                  <section className="flex flex-col gap-0.5">
                    <GroupHeader id="yours" count={yours.length} />
                    {yours.map((row) => renderRow(row))}
                  </section>
                )}

                {watching.length > 0 && (
                  <section className="flex flex-col gap-0.5">
                    <button
                      type="button"
                      data-testid="pr-group-watching-toggle"
                      aria-expanded={watchingOpen}
                      onClick={() => setWatchingChoice(!watchingOpen)}
                      className={cn(GROUP_HEADER, "rounded-sm text-left hover:text-foreground")}
                    >
                      {watchingOpen ? (
                        <ChevronDown aria-hidden className="size-3.5 shrink-0" />
                      ) : (
                        <ChevronRight aria-hidden className="size-3.5 shrink-0" />
                      )}
                      <Users aria-hidden className="size-3.5 shrink-0" />
                      <span className="shrink-0">{GROUP_LABEL.watching}</span>
                      <span className="shrink-0 tabular-nums text-muted-foreground/60">
                        {watching.length}
                      </span>
                      <span aria-hidden className="h-px min-w-2 flex-1 bg-border/60" />
                    </button>
                    {watchingOpen ? (
                      watching.map((row) => renderRow(row, true))
                    ) : (
                      <p className={cn("px-3 leading-relaxed text-muted-foreground/80", tzMetaNum)}>
                        Repositories you have open but aren't involved in.
                      </p>
                    )}
                  </section>
                )}

                {hidden > 0 && (
                  <p className={cn("px-3 text-muted-foreground", tzMetaNum)}>
                    Showing the first {MAX_RENDERED} of {review.length + yours.length + watching.length}.
                    Narrow it with the search field above.
                  </p>
                )}
              </>
            )}
          </div>

          <div className={cn("flex items-center gap-2 px-3 text-muted-foreground", tzMetaNum)}>
            <span className="min-w-0 flex-1 truncate">
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
              {age && (
                <>
                  {" · "}
                  <span data-testid="pr-list-age" data-carried={carried}>
                    {carried ? `as of ${age} ago` : `updated ${age} ago`}
                  </span>
                </>
              )}
            </span>
            <span className={cn("shrink-0 font-mono text-muted-foreground/70", tzEyebrow)}>
              ↑↓ move · ↵ open
            </span>
          </div>
        </div>
      </div>
    </div>
  );
}

/** The toolbar's outlined control shape, shared by the filter and refresh. */
const TOOLBAR_BUTTON =
  "flex h-8 shrink-0 items-center rounded-md border border-border bg-background text-body text-foreground/90 transition-colors duration-150 hover:bg-accent/60 disabled:pointer-events-none disabled:opacity-50 dark:bg-input/30";

const GROUP_HEADER =
  "flex w-full items-center gap-2 px-3 pb-1 text-label font-medium text-muted-foreground";

/**
 * The refresh failed and you are reading what was there before.
 *
 * Binding rule 2, given a shape: the rows stay, and one line above them
 * says what happened and how old what you can see is. It follows the
 * drift notice's anatomy — warn dot, sentence, action — because it is
 * the same idea one surface up: the world moved, here is what that means
 * for what you are reading, and here is the one thing to do about it.
 *
 * It is a strip and not a dialog, and it appears above the list rather
 * than over it, so nothing that was readable a moment ago stops being
 * readable.
 *
 * `resumesAt` swaps the sentence for the one case where "it failed"
 * would be a lie by omission: the host refused because the account's
 * hourly API budget is spent, and the page has stopped asking until it
 * refills. That is not a broken repository and pressing Retry is not
 * what fixes it — waiting is — so the strip says which, and until when.
 * Retry remains visible but disabled until the cooldown ends.
 *
 * A clock time rather than "in 15m", and the reason is specific to this
 * state: while the gate is up nothing is polling, so nothing re-renders,
 * so a countdown would sit frozen at whatever it read when the strip
 * first painted. "14:12" is still true an hour later.
 */
function StaleStrip({
  age,
  hasRows,
  resumesAt,
  onRetry,
}: {
  age: string | null;
  /** Whether there is in fact a list underneath. On a cold start that
   *  was refused there isn't one, and the clause about what you are
   *  looking at has to go — it would sit directly above an empty state
   *  saying the opposite. */
  hasRows: boolean;
  resumesAt: number | null;
  onRetry: () => void;
}) {
  // Three facts, and the sentence names only the ones that are true:
  // what happened, what you are looking at instead (nothing, on a cold
  // start), and when it will right itself (only the budget knows that).
  const headline = resumesAt == null ? "The latest refresh failed" : "Rate limit reached";
  const showing = !hasRows
    ? null
    : age
      ? `showing the list from ${age} ago`
      : "showing the last list loaded";
  const resuming = resumesAt == null ? null : `resuming at ${clockTime(resumesAt)}`;
  const rest = [showing, resuming].filter(Boolean).join(", ");
  const sentence = rest ? `${headline} · ${rest}` : headline;
  return (
    <div
      role="status"
      data-testid="pr-stale-strip"
      className="flex shrink-0 flex-wrap items-center gap-2 rounded-md border border-status-working/25 bg-status-working/10 px-3 py-2"
    >
      <span aria-hidden className="size-2 shrink-0 rounded-full bg-status-working" />
      <span className={cn("min-w-[11rem] flex-1 leading-snug text-foreground/80", tzBody)}>
        {sentence}
      </span>
      <button
        type="button"
        data-testid="pr-stale-retry"
        disabled={resumesAt != null}
        onClick={onRetry}
        className={cn(btnCard, "disabled:pointer-events-none disabled:opacity-50")}
      >
        Retry
      </button>
    </div>
  );
}

const GROUP_ICON: Record<"review" | "yours", LucideIcon> = {
  review: Eye,
  yours: PenLine,
};

function GroupHeader({ id, count }: { id: "review" | "yours"; count: number }) {
  const Icon = GROUP_ICON[id];
  return (
    <div className={GROUP_HEADER} data-testid={`pr-group-${id}`}>
      <Icon
        aria-hidden
        className={cn("size-3.5 shrink-0", id === "review" && "text-accent-ember")}
      />
      <h2 className="shrink-0">{GROUP_LABEL[id]}</h2>
      <span className="shrink-0 tabular-nums text-muted-foreground/60">{count}</span>
      <span aria-hidden className="h-px min-w-2 flex-1 bg-border/60" />
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
  isLoading,
  unanswered,
  query,
  onClearQuery,
}: {
  hasRows: boolean;
  hasHosts: boolean;
  isLoading: boolean;
  /** *Every* root refused and the page has stopped asking, so it has no
   *  answer to give — as opposed to having one that happens to be
   *  empty. One root being over budget while twenty others answered
   *  "nothing open" is an answer, and saying otherwise would be the same
   *  lie in the other direction. */
  unanswered: boolean;
  query: string;
  onClearQuery: () => void;
}) {
  if (query.trim() && hasRows) {
    return (
      <div className="flex flex-col items-start gap-2 px-3 py-8">
        <p className="text-label text-foreground">Nothing matches that search.</p>
        <p className={cn("text-muted-foreground", tzBody)}>
          Tokens are <span className="font-mono">is:draft</span>,{" "}
          <span className="font-mono">ci:failing</span> and{" "}
          <span className="font-mono">author:name</span>; everything else is title text.
        </p>
        <button
          type="button"
          onClick={onClearQuery}
          className={cn(
            "rounded-md bg-card px-3 py-1.5 text-foreground/90 hover:bg-accent/50",
            tzBody,
          )}
        >
          Clear the search
        </button>
      </div>
    );
  }

  if (!hasHosts) {
    return (
      <div className="flex flex-col items-start gap-2 px-3 py-8">
        <p className="text-label text-foreground">No projects open.</p>
        <p className={cn("leading-relaxed text-muted-foreground", tzBody)}>
          This page lists pull requests across the projects you have open. Open one and
          its repository shows up here.
        </p>
      </div>
    );
  }

  // Asked, and refused. The same rule as the loading case immediately
  // below, for the same reason: "no open pull requests" is an answer,
  // and we do not have one. The strip above this says when we will.
  if (unanswered) {
    return (
      <div className="flex flex-col items-start gap-2 px-3 py-8" data-testid="pr-list-unanswered">
        <p className="text-label text-foreground">Nothing to show yet.</p>
        <p className={cn("leading-relaxed text-muted-foreground", tzBody)}>
          The host is refusing requests until the account&apos;s API budget refills, so this
          page has not been able to ask what is open. It tries again on its own.
        </p>
      </div>
    );
  }

  // The one place a loading state is still correct: nothing has ever
  // been on this page, so there is nothing to keep instead (rule 02).
  // Saying "no open pull requests" here would be answering a question
  // the host has not been asked yet.
  if (isLoading) {
    return (
      <div className="flex flex-col items-start gap-2 px-3 py-8" data-testid="pr-list-first-load">
        <p className="text-label text-foreground">Looking for pull requests…</p>
        <p className={cn("leading-relaxed text-muted-foreground", tzBody)}>
          Asking each repository you have open. Rows appear as they answer.
        </p>
      </div>
    );
  }

  return (
    <div className="flex flex-col items-start gap-2 px-3 py-8">
      <p className="text-label text-foreground">No open pull requests.</p>
      <p className={cn("leading-relaxed text-muted-foreground", tzBody)}>
        Nothing is open on the projects you have here. If you expected some, check that
        the host CLI is signed in under Settings ▸ Source Control.
      </p>
    </div>
  );
}
