import { useRef, useCallback, useImperativeHandle, forwardRef, Fragment } from "react";
import type { DiffLine } from "@/lib/diff-parser";
import { cn } from "@/lib/utils";
import {
  diffRowStyle,
  lineNumberOn,
  sideOf,
  SELECTED_NUMBER_CLASS,
  SELECTED_ROW_CLASS,
  type DiffSelection,
} from "./diff-row";

interface Props {
  lines: DiffLine[];
  /** Line review. Absent on the Changes pane, which only reads. */
  selection?: DiffSelection;
  /** Render at natural height inside someone else's scroll container —
   *  what a per-file review section needs, since a PR is one scroll
   *  through many files rather than one scrollbar per file. */
  flow?: boolean;
}

export interface DiffViewHandle {
  scrollToHunk: (direction: 1 | -1) => void;
}

export const DiffUnifiedView = forwardRef<DiffViewHandle, Props>(
  function DiffUnifiedView({ lines, selection, flow = false }, ref) {
    const containerRef = useRef<HTMLDivElement>(null);
    const scrollToHunk = useCallback(
      (direction: 1 | -1) => {
        const container = containerRef.current;
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
        if (target) container.scrollTo({ top: target.offsetTop, behavior: "smooth" });
      },
      [],
    );

    useImperativeHandle(ref, () => ({ scrollToHunk }), [scrollToHunk]);

    return (
      <div
        ref={containerRef}
        // `relative` is load-bearing: it makes this the offsetParent, so each
        // hunk's offsetTop is a scroll position rather than a page coordinate
        // measured from the app shell above the toolbar.
        className={
          flow
            ? "code-surface select-text bg-card"
            : "code-surface select-text relative flex-1 overflow-auto bg-card"
        }
      >
        <div className="py-0.5">
          {lines.map((line, i) => {
            if (line.type === "hunk-header") {
              return (
                <div
                  key={i}
                  data-diff-hunk
                  className="flex min-h-[18px] bg-muted/30 whitespace-pre mt-1 first:mt-0"
                >
                  <span className="w-[72px] shrink-0" />
                  <span className="text-muted-foreground/60 text-[11px] px-3">
                    {line.content}
                  </span>
                </div>
              );
            }

            const style = diffRowStyle(line);
            const side = sideOf(line);
            const lineNo = lineNumberOn(line, side);
            const selectable = !!selection && lineNo != null;
            const selected = selectable && selection.isSelected(line, side);
            const under = selection?.renderUnder?.(line, side);

            const numberClass = cn(
              "inline-block w-[36px] text-right pr-2 text-[11px] tabular-nums",
              selected ? SELECTED_NUMBER_CLASS : "text-muted-foreground",
            );

            return (
              <Fragment key={i}>
                <div
                  data-diff-row={selectable ? `${side}:${lineNo}` : undefined}
                  data-selected={selected ? "true" : undefined}
                  className={cn(
                    "flex min-h-[18px] whitespace-pre",
                    selected ? SELECTED_ROW_CLASS : style.bgClass,
                  )}
                >
                  {/* The gutter is the click target, not the row: dragging
                      across code to copy it must stay a text selection. */}
                  <span
                    data-diff-line={selectable ? `${side}:${lineNo}` : undefined}
                    className={cn(
                      "flex w-[72px] shrink-0 select-none",
                      selectable && "cursor-pointer hover:bg-accent-ember/10",
                    )}
                    onMouseDown={
                      selectable
                        ? (e) => {
                            // Shift-click extends; without this the browser
                            // paints a text selection over the whole range.
                            if (e.shiftKey) e.preventDefault();
                          }
                        : undefined
                    }
                    onClick={
                      selectable
                        ? (e) => selection.onSelect(line, side, e.shiftKey)
                        : undefined
                    }
                  >
                    <span className={numberClass}>{line.oldLine ?? ""}</span>
                    <span className={numberClass}>{line.newLine ?? ""}</span>
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
                      <span className="text-[9px] font-bold text-accent-violet mr-2">
                        THEIRS
                      </span>
                    )}
                    {line.content}
                  </span>
                </div>
                {under}
              </Fragment>
            );
          })}
        </div>
      </div>
    );
  },
);
