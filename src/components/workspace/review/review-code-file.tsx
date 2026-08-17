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
import { groupDigits, tzBody, tzMeta, tzMetaNum } from "./review-ui";
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
      {/* Chevron, then the path immediately beside it — the two things
          you scan a file list by sit together on the left, and the
          numbers and the Viewed toggle collect on the right. The path
          used to take the whole free width, and because it truncates
          from the left (`dir="rtl"`) that pushed the text itself to the
          far right, where it read as part of the stats cluster. It now
          shrinks to its content and an empty span holds the gap. */}
      <div className="flex items-center gap-2 bg-muted/30 px-3 py-2">
        <button
          type="button"
          aria-label={collapsed ? `Expand ${file.path}` : `Collapse ${file.path}`}
          aria-expanded={!collapsed}
          onClick={() => setManuallyCollapsed((c) => !c)}
          className="shrink-0 text-muted-foreground hover:text-foreground"
        >
          {collapsed ? (
            <ChevronRight className="size-3.5" />
          ) : (
            <ChevronDown className="size-3.5" />
          )}
        </button>
        <span
          data-testid="code-file-path"
          className={cn("min-w-0 shrink truncate text-left font-mono text-foreground", tzBody)}
          title={file.path}
          dir="rtl"
        >
          {file.path}
        </span>
        {file.status !== "modified" && (
          <span className={cn("shrink-0 text-muted-foreground", tzMeta)}>{file.status}</span>
        )}
        {pendingNotes > 0 && (
          <span
            data-testid="file-pending-count"
            className={cn(
              "flex shrink-0 items-center gap-1 rounded bg-accent-ember/15 px-1.5 py-0.5 font-semibold text-accent-ember",
              tzMeta,
            )}
          >
            {pendingNotes} pending
          </span>
        )}
        <span className="flex-1" />
        <span className={cn("shrink-0 font-mono text-success", tzMetaNum)}>
          +{groupDigits(file.additions)}
        </span>
        <span className={cn("shrink-0 font-mono text-danger", tzMetaNum)}>
          −{groupDigits(file.deletions)}
        </span>
        <button
          type="button"
          data-testid="viewed-toggle"
          aria-pressed={viewed}
          onClick={onToggleViewed}
          className={cn(
            "shrink-0 rounded border-0 px-2 py-0.5 transition-colors",
            tzMetaNum,
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
            className={cn("px-3 py-2.5 text-muted-foreground", tzBody)}
          >
            {file.binary ? "Binary file" : "Generated file"} — not shown. Nothing here
            reads line by line.
          </p>
        ) : large && !loadedAnyway ? (
          <div
            data-testid="file-too-large"
            className={cn(
              "flex items-center gap-2 px-3 py-2.5 text-muted-foreground",
              tzBody,
            )}
          >
            <span className="flex-1">
              {groupDigits(changed)} changed lines — large enough to bury the rest of
              the diff.
            </span>
            <button
              type="button"
              data-testid="load-anyway"
              onClick={() => setLoadedAnyway(true)}
              className={cn(
                "shrink-0 rounded bg-card px-2.5 py-1 font-medium text-foreground hover:bg-accent/50",
                tzMetaNum,
              )}
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
