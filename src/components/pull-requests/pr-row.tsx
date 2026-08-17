import { memo, useState } from "react";

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
 * The CI colour that leads the row.
 *
 * A draft's dot is dashed and grey whatever CI says: a draft is not
 * asking for a verdict yet, and a green dot on one reads as "ready".
 *
 * `checks === null` is the row painting before its rollup has arrived.
 * It gets a placeholder — dimmer than "no checks", not a colour and not
 * a spinner, because a spinner would claim CI is running and a colour
 * would claim a verdict. It is the shape the answer will take, holding
 * the space the answer will fill.
 */
function StateDot({ checks, draft }: { checks: string | null; draft: boolean }) {
  if (draft) {
    return (
      <span
        aria-hidden
        data-state="draft"
        className="size-2.5 shrink-0 rounded-full border-[1.5px] border-dashed border-muted-foreground/70"
      />
    );
  }
  if (checks === "pending") {
    return (
      <span
        aria-hidden
        data-state="pending"
        className="size-2.5 shrink-0 animate-spin rounded-full border-[1.6px] border-status-working border-r-transparent"
      />
    );
  }
  if (checks === "failing") {
    return (
      <span
        aria-hidden
        data-state="failing"
        className="size-2.5 shrink-0 rounded-full bg-destructive"
      />
    );
  }
  if (checks === "passing") {
    return (
      <span
        aria-hidden
        data-state="passing"
        className="size-2.5 shrink-0 rounded-full border-[1.6px] border-status-open bg-status-open/25"
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
  return (
    <span
      aria-hidden
      data-state="none"
      className="size-2.5 shrink-0 rounded-full border-[1.5px] border-border"
    />
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
        "size-[12px] shrink-0 rounded-[3px]",
        kind === "gitlab" ? "bg-accent-ember/80" : "bg-foreground/40",
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

  if (dense) {
    return (
      <div
        role="option"
        aria-selected={selected}
        data-testid={`pr-row-${row.projectRoot}-${row.number}`}
        data-focused={focused}
        onClick={onSelect}
        className={cn(
          "group flex cursor-default items-center gap-2 py-1.5 pr-2.5",
          selected
            ? "border-l-2 border-accent-ember bg-card pl-[9px]"
            : "pl-2.5 hover:bg-muted/30",
        )}
      >
        <StateDot checks={row.checks} draft={row.is_draft} />
        <span className={cn("shrink-0 font-mono text-muted-foreground", tzMetaNum)}>
          {providerRef(provider, row.number)}
        </span>
        <span className={cn("min-w-0 flex-1 truncate text-foreground", tzBodyLg)}>
          {row.title}
        </span>
        {moved && <MovedMark />}
        <span className={cn("shrink-0 text-muted-foreground", tzMetaNum)}>{row.author}</span>
        {age && <span className={cn("shrink-0 text-muted-foreground", tzMetaNum)}>{age}</span>}
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
        "group flex cursor-default flex-col gap-1.5 py-2 pr-2.5",
        selected
          ? "border-l-2 border-accent-ember bg-card pl-[9px]"
          : "pl-2.5 hover:bg-muted/30",
        row.is_draft && "opacity-70",
      )}
    >
      <div className="flex items-center gap-1.5">
        <StateDot checks={row.checks} draft={row.is_draft} />
        {/* Titles never wrap: a two-line title pushes the row below it
            off the fold and makes every row a different height. */}
        <span
          className={cn("min-w-0 flex-1 truncate font-semibold text-foreground", tzRowTitle)}
          title={row.title}
        >
          {row.title}
        </span>

        {row.is_draft && (
          <span
            className={cn(
              "shrink-0 rounded border border-border px-1.5 py-px text-muted-foreground",
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

        {/* The action's slot is held open whether or not the action is
            showing. It used to be inserted on hover, which pushed the
            title and slid the state label sideways — so running the
            pointer down the list made every row twitch as you passed it.
            The button is hidden by visibility, not by display: the row's
            geometry is now identical at rest and on hover. */}
        <span
          data-testid="pr-row-action-slot"
          className="flex w-[78px] shrink-0 justify-end"
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
                "invisible max-w-full shrink-0 truncate rounded-[5px] bg-card px-2 py-0.5 text-foreground/90",
                tzMeta,
                "transition-colors hover:bg-accent/60 disabled:opacity-60",
                "group-hover:visible group-data-[focused=true]:visible",
              )}
            >
              {existingWorkspaceId ? "Switch" : "Check out"}
            </button>
          )}
        </span>
      </div>

      <div
        className={cn(
          "flex min-w-0 items-center gap-1.5 text-muted-foreground",
          tzMetaNum,
        )}
      >
        <HostMark kind={row.providerKind} />
        <span className="shrink-0 font-mono">{providerRef(provider, row.number)}</span>
        <span className="shrink-0 opacity-40">·</span>
        <span className="min-w-0 max-w-[40%] truncate">{row.repo}</span>
        {row.author && (
          <>
            <span className="shrink-0 opacity-40">·</span>
            <span className="shrink-0 truncate">{row.author}</span>
          </>
        )}
        {existingWorkspaceId && (
          <>
            <span className="shrink-0 opacity-40">·</span>
            <span className="shrink-0 text-status-open">checked out</span>
          </>
        )}
        <span className="ml-auto flex shrink-0 items-center gap-1.5">
          {moved && <MovedMark />}
          {row.additions != null && row.additions > 0 && (
            <span className="font-mono text-status-open">+{groupDigits(row.additions)}</span>
          )}
          {row.deletions != null && row.deletions > 0 && (
            <span className="font-mono text-destructive">−{groupDigits(row.deletions)}</span>
          )}
          {age && <span>{age}</span>}
        </span>
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
