import { useRef, useCallback, useImperativeHandle, forwardRef, Fragment } from "react";
import type { DiffLine } from "@/lib/diff-parser";
import { buildSplitPairs } from "@/lib/diff-parser";
import { cn } from "@/lib/utils";
import { Separator } from "@/components/ui/separator";
import type { DiffViewHandle } from "./DiffUnifiedView";
import {
  diffRowStyle,
  SELECTED_NUMBER_CLASS,
  SELECTED_ROW_CLASS,
  type DiffRowSide,
  type DiffSelection,
} from "./diff-row";

interface Props {
  lines: DiffLine[];
  /** Line review. Absent on the Changes pane, which only reads. */
  selection?: DiffSelection;
  /** Natural height inside someone else's scroll container. In flow
   *  mode the two columns scroll together with the page, which is what
   *  a per-file section wants — there is nothing left to synchronise. */
  flow?: boolean;
}

/** One handle for both views — it was declared twice, identically. */
export type { DiffViewHandle };

function SplitSideLine({
  line,
  side,
  selection,
  flow = false,
}: {
  line: DiffLine | null;
  side: "left" | "right";
  selection?: DiffSelection;
  /** Review's taller leading. Applied to every row shape here — the
   *  placeholder included — or the two columns stop lining up. */
  flow?: boolean;
}) {
  const rowHeight = flow ? "min-h-[22px]" : "min-h-[18px]";
  const gutterSize = flow ? "text-[12px]" : "text-[11px]";

  if (!line) {
    return (
      <div className={cn("flex whitespace-pre bg-muted/5", rowHeight)}>
        <span className="w-10 shrink-0 select-none" />
        <span className="flex-1" />
      </div>
    );
  }

  if (line.type === "hunk-header") {
    return (
      <div
        data-diff-hunk
        className={cn("flex bg-muted/30 whitespace-pre mt-1 first:mt-0", rowHeight)}
      >
        <span className="w-10 shrink-0 select-none" />
        <span className={cn("text-muted-foreground/60 px-2 truncate", gutterSize)}>
          {line.content}
        </span>
      </div>
    );
  }

  const lineNum = side === "left" ? line.oldLine : line.newLine;
  // The column is the side. A context line appears in both columns and
  // is addressable from either — the one you clicked is the one you meant.
  const anchorSide: DiffRowSide = side === "left" ? "LEFT" : "RIGHT";
  const style = diffRowStyle(line);
  const selectable = !!selection && lineNum != null;
  const selected = selectable && selection.isSelected(line, anchorSide);
  const under = selection?.renderUnder?.(line, anchorSide);

  return (
    <Fragment>
      <div
        data-diff-row={selectable ? `${anchorSide}:${lineNum}` : undefined}
        data-selected={selected ? "true" : undefined}
        className={cn(
          "flex whitespace-pre",
          rowHeight,
          selected ? SELECTED_ROW_CLASS : style.bgClass,
        )}
      >
        <span
          data-diff-line={selectable ? `${anchorSide}:${lineNum}` : undefined}
          className={cn(
            "inline-block w-10 shrink-0 text-right pr-2 tabular-nums select-none",
            gutterSize,
            selected ? SELECTED_NUMBER_CLASS : "text-muted-foreground",
            selectable && "cursor-pointer hover:bg-accent-ember/10",
          )}
          onMouseDown={selectable ? (e) => { if (e.shiftKey) e.preventDefault(); } : undefined}
          onClick={
            selectable ? (e) => selection.onSelect(line, anchorSide, e.shiftKey) : undefined
          }
        >
          {lineNum ?? ""}
        </span>
        <span
          className={`inline-block w-4 shrink-0 text-center select-none ${style.prefixColor}`}
        >
          {style.prefixChar}
        </span>
        <span className="flex-1 min-w-0 pr-4">
          {style.isOursMarker && (
            <span className="text-[9px] font-bold text-primary mr-2">OURS</span>
          )}
          {style.isTheirsMarker && (
            <span className="text-[9px] font-bold text-accent-violet mr-2">THEIRS</span>
          )}
          {line.content}
        </span>
      </div>
      {under}
    </Fragment>
  );
}

export const DiffSplitView = forwardRef<DiffViewHandle, Props>(
  function DiffSplitView({ lines, selection, flow = false }, ref) {
    const leftRef = useRef<HTMLDivElement>(null);
    const rightRef = useRef<HTMLDivElement>(null);
    const isSyncing = useRef(false);

    const pairs = buildSplitPairs(lines);

    const scrollToHunk = useCallback(
      (direction: 1 | -1) => {
        const container = leftRef.current;
        if (!container) return;
        const hunks = Array.from(
          container.querySelectorAll<HTMLElement>("[data-diff-hunk]"),
        );
        const target =
          direction === 1
            ? hunks.find((hunk) => hunk.offsetTop > container.scrollTop + 1)
            : [...hunks]
                .reverse()
                .find((hunk) => hunk.offsetTop < container.scrollTop - 1);
        if (target) {
          const top = target.offsetTop;
          leftRef.current?.scrollTo({ top, behavior: "smooth" });
          rightRef.current?.scrollTo({ top, behavior: "smooth" });
        }
      },
      [],
    );

    useImperativeHandle(ref, () => ({ scrollToHunk }), [scrollToHunk]);

    const handleScroll =
      (source: "left" | "right") => (e: React.UIEvent<HTMLDivElement>) => {
        if (isSyncing.current) return;
        isSyncing.current = true;
        const target =
          source === "left" ? rightRef.current : leftRef.current;
        if (target) target.scrollTop = e.currentTarget.scrollTop;
        requestAnimationFrame(() => {
          isSyncing.current = false;
        });
      };

    const columnClass = flow ? "bg-card" : "relative overflow-auto bg-card";

    return (
      <div
        className={cn(
          "code-surface select-text grid grid-cols-[1fr_1px_1fr]",
          flow ? "bg-card" : "flex-1 min-h-0 overflow-hidden",
        )}
      >
        {/* Left — deletions / old. `relative` is load-bearing outside flow
            mode: it makes this the offsetParent, so each hunk's offsetTop is
            a scroll position rather than a page coordinate measured from the
            app shell. */}
        <div
          ref={leftRef}
          className={columnClass}
          onScroll={flow ? undefined : handleScroll("left")}
        >
          <div className="py-0.5">
            {pairs.map((pair, i) => (
              <SplitSideLine
                key={i}
                line={pair.left}
                side="left"
                selection={selection}
                flow={flow}
              />
            ))}
          </div>
        </div>

        {/* Divider */}
        <Separator orientation="vertical" className="bg-border/50" />

        {/* Right — additions / new */}
        <div
          ref={rightRef}
          className={columnClass}
          onScroll={flow ? undefined : handleScroll("right")}
        >
          <div className="py-0.5">
            {pairs.map((pair, i) => (
              <SplitSideLine
                key={i}
                line={pair.right}
                side="right"
                selection={selection}
                flow={flow}
              />
            ))}
          </div>
        </div>
      </div>
    );
  },
);
