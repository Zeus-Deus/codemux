import { useState } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
import { cn } from "@/lib/utils";
import { DiffUnifiedView } from "@/components/diff/DiffUnifiedView";
import { DiffSplitView } from "@/components/diff/DiffSplitView";
import type { DiffSelection } from "@/components/diff/diff-row";
import {
  changedLines,
  ignoreWhitespace,
  isLargeFile,
  isRenderable,
  type PrDiffFile,
} from "@/lib/pr-diff";
import { groupDigits } from "./review-ui";
import type { DiffLayout } from "./pr-drafts";

interface Props {
  file: PrDiffFile;
  layout: DiffLayout;
  hideWhitespace: boolean;
  viewed: boolean;
  onToggleViewed: () => void;
  selection?: DiffSelection;
  /** Notes pending on this file — shown on the header while collapsed,
   *  so marking a file viewed never hides that you wrote something. */
  pendingNotes: number;
  /** Force the body open (the re-anchor flow scrolls here). */
  forceOpen?: boolean;
}

/**
 * One file of the diff.
 *
 * Collapsing is the header's whole job: a PR is read file by file, and
 * the ones you've decided about should stop taking vertical space
 * without leaving the list.
 */
export function ReviewCodeFile({
  file,
  layout,
  hideWhitespace,
  viewed,
  onToggleViewed,
  selection,
  pendingNotes,
  forceOpen = false,
}: Props) {
  const [manuallyCollapsed, setManuallyCollapsed] = useState(false);
  // Numbers, not vibes: a 2,000-line file is opened on request, not by
  // default — otherwise the file you came to read is ten screens down.
  const [loadedAnyway, setLoadedAnyway] = useState(false);

  const collapsed = !forceOpen && (viewed || manuallyCollapsed);
  const renderable = isRenderable(file);
  const large = isLargeFile(file);
  const changed = changedLines(file);

  const lines = hideWhitespace ? ignoreWhitespace(file.lines) : file.lines;

  return (
    <section
      data-testid="code-file"
      data-file-path={file.path}
      data-viewed={viewed ? "true" : undefined}
      className="border-b border-border/40 last:border-b-0"
    >
      <div className="flex items-center gap-2 bg-muted/30 px-3 py-1.5">
        <button
          type="button"
          aria-label={collapsed ? `Expand ${file.path}` : `Collapse ${file.path}`}
          aria-expanded={!collapsed}
          onClick={() => setManuallyCollapsed((c) => !c)}
          className="shrink-0 text-muted-foreground hover:text-foreground"
        >
          {collapsed ? <ChevronRight className="size-3" /> : <ChevronDown className="size-3" />}
        </button>
        <span
          className="min-w-0 flex-1 truncate font-mono text-[11px] text-foreground"
          title={file.path}
          dir="rtl"
        >
          {file.path}
        </span>
        {file.status !== "modified" && (
          <span className="shrink-0 text-[10px] text-muted-foreground">{file.status}</span>
        )}
        {pendingNotes > 0 && (
          <span
            data-testid="file-pending-count"
            className="flex shrink-0 items-center gap-1 rounded bg-accent-ember/15 px-1.5 py-px text-[10px] font-semibold text-accent-ember"
          >
            {pendingNotes} pending
          </span>
        )}
        <span className="shrink-0 font-mono text-[10.5px] text-success">
          +{groupDigits(file.additions)}
        </span>
        <span className="shrink-0 font-mono text-[10.5px] text-danger">
          −{groupDigits(file.deletions)}
        </span>
        <button
          type="button"
          data-testid="viewed-toggle"
          aria-pressed={viewed}
          onClick={onToggleViewed}
          className={cn(
            "shrink-0 rounded border-0 px-1.5 py-px text-[10.5px] transition-colors",
            viewed
              ? "bg-status-open/15 font-semibold text-status-open"
              : "bg-card text-muted-foreground hover:text-foreground",
          )}
        >
          Viewed
        </button>
      </div>

      {!collapsed &&
        (!renderable ? (
          <p
            data-testid="file-not-rendered"
            className="px-3 py-2 text-[11px] text-muted-foreground"
          >
            {file.binary ? "Binary file" : "Generated file"} — not shown. Nothing here
            reads line by line.
          </p>
        ) : large && !loadedAnyway ? (
          <div
            data-testid="file-too-large"
            className="flex items-center gap-2 px-3 py-2 text-[11px] text-muted-foreground"
          >
            <span className="flex-1">
              {groupDigits(changed)} changed lines — large enough to bury the rest of
              the diff.
            </span>
            <button
              type="button"
              data-testid="load-anyway"
              onClick={() => setLoadedAnyway(true)}
              className="shrink-0 rounded bg-card px-2 py-0.5 text-[10.5px] font-medium text-foreground hover:bg-accent/50"
            >
              Load anyway
            </button>
          </div>
        ) : layout === "split" ? (
          <DiffSplitView lines={lines} selection={selection} flow />
        ) : (
          <DiffUnifiedView lines={lines} selection={selection} flow />
        ))}
    </section>
  );
}
