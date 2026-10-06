import { memo, useState } from "react";
import {
  CircleCheck,
  CircleX,
  GitMerge,
  GitPullRequest,
  GitPullRequestClosed,
  GitPullRequestDraft,
} from "lucide-react";

import { cn } from "@/lib/utils";
import { toast } from "@/lib/toast";
import { checkOutPr } from "@/lib/pr-checkout";
import { providerRef, type ProviderPresentation } from "@/lib/source-control";
import type { PrRow as PrRowData } from "@/lib/pr-overview";
import {
  groupDigits,
  shortAge,
  tzBodyLg,
  tzEyebrow,
  tzMeta,
  tzMetaNum,
  tzRowTitle,
} from "@/components/workspace/review/review-ui";

/** "5m" — the list has room for a magnitude, not a sentence. */
function compactAge(iso: string | null): string | null {
  if (!iso) return null;
  const then = Date.parse(iso);
  if (Number.isNaN(then)) return null;
  return shortAge(Date.now() - then);
}

/**
 * The pull request's own state, leading the row: open, draft, merged or
 * closed, in the colours the hosts use for them.
 */
function StateGlyph({ row }: { row: PrRowData }) {
  const state = row.state?.toUpperCase();
  const [Icon, tone, label] =
    state === "MERGED"
      ? [GitMerge, "text-accent-violet", "Merged"]
      : state === "CLOSED"
        ? [GitPullRequestClosed, "text-destructive", "Closed"]
        : row.is_draft
          ? [GitPullRequestDraft, "text-muted-foreground", "Draft"]
          : [GitPullRequest, "text-status-open", "Open"];
  return <Icon aria-label={label} className={cn("size-4 shrink-0", tone)} />;
}

/**
 * The CI verdict beside the title.
 *
 * A draft shows nothing whatever CI says: a draft is not asking for a
 * verdict yet, and a green check on one reads as "ready".
 *
 * `checks === null` is the row painting before its rollup has arrived.
 * It gets a placeholder — dimmer than "no checks", not a colour and not
 * a spinner, because a spinner would claim CI is running and a colour
 * would claim a verdict. It is the shape the answer will take, holding
 * the space the answer will fill.
 */
function ChecksMark({ checks, draft }: { checks: string | null; draft: boolean }) {
  if (draft) return null;
  if (checks === "pending") {
    return (
      <span
        role="img"
        aria-label="Checks running"
        data-state="pending"
        className="size-3 shrink-0 animate-spin rounded-full border-[1.5px] border-status-working border-r-transparent"
      />
    );
  }
  if (checks === "failing") {
    return (
      <CircleX
        aria-label="Checks failing"
        data-state="failing"
        className="size-3.5 shrink-0 text-destructive"
      />
    );
  }
  if (checks === "passing") {
    return (
      <CircleCheck
        aria-label="Checks passing"
        data-state="passing"
        className="size-3.5 shrink-0 text-status-open"
      />
    );
  }
  if (checks == null) {
    return (
      <span
        aria-hidden
        data-state="unknown"
        title="Checks are still loading"
        className="size-2.5 shrink-0 rounded-full bg-muted-foreground/20"
      />
    );
  }
  return null;
}

/** The author's initial in a round chip — the hosts' avatars aren't in
 *  the overview payload, and a letter is enough to tell people apart. */
function AuthorChip({ author }: { author: string }) {
  return (
    <span className="flex min-w-0 items-center gap-1">
      <span
        aria-hidden
        className="flex size-3.5 shrink-0 items-center justify-center rounded-full bg-muted text-[9px] font-semibold uppercase leading-none text-foreground/80"
      >
        {author.charAt(0)}
      </span>
      <span className="truncate">{author}</span>
    </span>
  );
}

/** The host mark. Ember for GitLab, neutral for GitHub — the colour is
 *  the only thing that has to survive at this size. */
function HostMark({ kind }: { kind: string }) {
  return (
    <span
      aria-hidden
      data-testid={`host-mark-${kind}`}
      className={cn(
        "size-2 shrink-0 rounded-sm",
        kind === "gitlab" ? "bg-accent-ember/80" : "bg-foreground/35",
      )}
    />
  );
}

/**
 * The state label on the right of the title.
 *
 * One label, in the order that decides what to do next: something is
 * blocking, it is ready, or it is not asking yet.
 */
function stateLabel(row: PrRowData): { text: string; className: string } | null {
  const state = row.state?.toUpperCase();
  if (state === "MERGED") return { text: "merged", className: "text-accent-violet" };
  if (state === "CLOSED") return { text: "closed", className: "text-muted-foreground" };
  if (row.is_draft) return null; // the Draft chip says it instead
  if (row.review_decision === "CHANGES_REQUESTED") {
    return { text: "changes requested", className: "text-status-working" };
  }
  // "Ready to merge" is a claim about CI as well as about approval, so
  // it waits for CI to have said something. Before the stats land the
  // row simply shows no label — an approved pull request whose build is
  // about to come back red must not be called ready in the meantime.
  if (row.review_decision === "APPROVED" && row.checks != null && row.checks !== "failing") {
    return { text: "ready to merge", className: "font-semibold text-status-open" };
  }
  return null;
}

export interface PrRowProps {
  row: PrRowData;
  provider: ProviderPresentation;
  selected: boolean;
  /** The row the keyboard is on — it shows its action like a hover. */
  focused: boolean;
  /** Rule 03: the poll wanted to move this row and was held off. */
  moved: boolean;
  /** Workspace already standing on this branch, when there is one. */
  existingWorkspaceId: string | null;
  /** One-line density, for the folded Watching group. */
  dense?: boolean;
  onSelect: () => void;
}

function PrRowImpl({
  row,
  provider,
  selected,
  focused,
  moved,
  existingWorkspaceId,
  dense = false,
  onSelect,
}: PrRowProps) {
  const [busy, setBusy] = useState(false);
  const age = compactAge(row.updated_at);
  const label = stateLabel(row);

  const checkOut = (event: React.MouseEvent) => {
    event.stopPropagation();
    if (busy) return;
    setBusy(true);
    checkOutPr({
      projectRoot: row.projectRoot,
      headBranch: row.head_branch,
      prNumber: row.number,
      existingWorkspaceId,
    })
      .catch((err) => toast.error(String(err)))
      .finally(() => setBusy(false));
  };

  const ref = providerRef(provider, row.number);

  if (dense) {
    return (
      <div
        role="option"
        aria-selected={selected}
        data-testid={`pr-row-${row.projectRoot}-${row.number}`}
        data-focused={focused}
        onClick={onSelect}
        className={cn(
          "group flex cursor-pointer items-center gap-2.5 rounded-md px-3 py-1.5 transition-colors duration-150",
          selected ? "bg-accent" : "hover:bg-accent/50",
        )}
      >
        <StateGlyph row={row} />
        <span className={cn("shrink-0 font-mono tabular-nums text-muted-foreground", tzMetaNum)}>
          {ref}
        </span>
        <span className={cn("min-w-0 flex-1 truncate text-foreground/90", tzBodyLg)}>
          {row.title}
        </span>
        {moved && <MovedMark />}
        <span className={cn("shrink-0 text-muted-foreground", tzMetaNum)}>{row.author}</span>
        {age && (
          <span className={cn("w-8 shrink-0 text-right tabular-nums text-muted-foreground", tzMetaNum)}>
            {age}
          </span>
        )}
      </div>
    );
  }

  return (
    <div
      role="option"
      aria-selected={selected}
      data-testid={`pr-row-${row.projectRoot}-${row.number}`}
      data-focused={focused}
      onClick={onSelect}
      className={cn(
        "group flex cursor-pointer items-start gap-2.5 rounded-md px-3 py-2.5 transition-colors duration-150",
        selected ? "bg-accent" : "hover:bg-accent/50",
      )}
    >
      <span className="mt-[3px] flex shrink-0">
        <StateGlyph row={row} />
      </span>

      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <div className="flex min-w-0 items-center gap-1.5">
          <span className={cn("shrink-0 font-mono tabular-nums text-muted-foreground", tzMetaNum)}>
            {ref}
          </span>
          {/* Titles never wrap: a two-line title pushes the row below it
              off the fold and makes every row a different height. */}
          <span
            className={cn(
              "min-w-0 truncate font-medium",
              row.is_draft ? "text-foreground/70" : "text-foreground",
              tzRowTitle,
            )}
            title={row.title}
          >
            {row.title}
          </span>
          <ChecksMark checks={row.checks} draft={row.is_draft} />

          {row.is_draft && (
            <span
              className={cn(
                "shrink-0 rounded-sm border border-border px-1.5 leading-4 text-muted-foreground",
                tzEyebrow,
              )}
            >
              Draft
            </span>
          )}
          {label && (
            <span
              data-testid="pr-row-state-label"
              className={cn("shrink-0", tzMeta, label.className)}
            >
              {label.text}
            </span>
          )}

          <span className="min-w-2 flex-1" />
          {/* The action's slot is held open whether or not the action is
              showing. It used to be inserted on hover, which pushed the
              title and slid the state label sideways — so running the
              pointer down the list made every row twitch as you passed it.
              The button is hidden by visibility, not by display: the row's
              geometry is now identical at rest and on hover. */}
          <span
            data-testid="pr-row-action-slot"
            className="-my-1 flex w-[78px] shrink-0 justify-end"
          >
            {row.head_branch && (
              <button
                type="button"
                data-testid="pr-row-checkout"
                disabled={busy}
                // Nothing to tab to while it is invisible.
                tabIndex={-1}
                onClick={checkOut}
                className={cn(
                  "invisible max-w-full shrink-0 truncate rounded-md border border-border bg-background px-2 py-0.5 text-foreground/90",
                  tzMeta,
                  "transition-colors duration-150 hover:bg-accent disabled:opacity-60",
                  "group-hover:visible group-data-[focused=true]:visible",
                )}
              >
                {existingWorkspaceId ? "Switch" : "Check out"}
              </button>
            )}
          </span>
          {moved && <MovedMark />}
          {(row.additions ?? 0) + (row.deletions ?? 0) > 0 && (
            <span className={cn("flex shrink-0 items-baseline gap-1 font-mono tabular-nums", tzMetaNum)}>
              {row.additions != null && row.additions > 0 && (
                <span className="text-status-open">+{groupDigits(row.additions)}</span>
              )}
              {row.deletions != null && row.deletions > 0 && (
                <span className="text-destructive">−{groupDigits(row.deletions)}</span>
              )}
            </span>
          )}
        </div>

        <div
          className={cn(
            "flex min-w-0 items-center gap-1.5 text-muted-foreground",
            tzMetaNum,
          )}
        >
          {row.author && <AuthorChip author={row.author} />}
          {row.author && <span className="shrink-0 opacity-40">·</span>}
          <HostMark kind={row.providerKind} />
          <span className="min-w-0 truncate">{row.repo}</span>
          {existingWorkspaceId && (
            <>
              <span className="shrink-0 opacity-40">·</span>
              <span className="shrink-0 text-status-open">checked out</span>
            </>
          )}
          <span className="min-w-2 flex-1" />
          {age && <span className="shrink-0 tabular-nums">{age}</span>}
        </div>
      </div>
    </div>
  );
}

/** The quiet mark rule 03 asks for: this row changed while you were
 *  reading, and its new position is waiting. */
function MovedMark() {
  return (
    <span
      data-testid="pr-row-moved"
      title="Updated — the list will re-sort when you're done"
      className="size-1.5 shrink-0 rounded-full bg-accent-ember"
    />
  );
}

// The list re-renders on every 30s poll and on every keyboard move;
// without this each of those walks all 50 rows.
export const PrRow = memo(PrRowImpl);
