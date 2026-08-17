import {
  useRef,
  useCallback,
  useImperativeHandle,
  useLayoutEffect,
  useState,
  forwardRef,
  Fragment,
  type ReactNode,
} from "react";
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

  return (
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
  );
}

/** What a column wants to draw beneath one row — a composer, a pending
 *  note. Hunk headers are not addressable, so they never have one. */
function underFor(
  selection: DiffSelection | undefined,
  line: DiffLine | null,
  side: DiffRowSide,
): ReactNode {
  if (!line || line.type === "hunk-header") return null;
  return selection?.renderUnder?.(line, side) ?? null;
}

/**
 * The under-row for one column, and the space it costs the other one.
 *
 * The two columns are separate scroll boxes, so anything rendered below
 * a row pushes only *that* column's remaining rows down — and from there
 * the sides no longer face each other, which is the one thing a split
 * view is for. A composer opened on a RIGHT line used to shift every
 * line below it out of alignment with its LEFT counterpart.
 *
 * Each side therefore reports the height it took, and both sides hold at
 * least the taller of the two. One side with content and one without
 * gives an empty box of exactly the same height; both sides with content
 * (a context line noted from LEFT *and* RIGHT) levels them to the taller.
 * Measured rather than assumed because the content is a live component
 * whose height changes as you type.
 */
function UnderRow({
  id,
  content,
  matchHeight,
  onMeasure,
}: {
  id: string;
  content: ReactNode;
  /** What the opposite column reported for this row. */
  matchHeight: number;
  onMeasure: (id: string, height: number) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) {
      // A spacer costs the other column nothing.
      onMeasure(id, 0);
      return;
    }
    const report = () => onMeasure(id, el.offsetHeight);
    report();
    // Absent in jsdom, where every height is zero and there is nothing
    // to keep level anyway.
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(report);
    observer.observe(el);
    return () => observer.disconnect();
  }, [content, id, onMeasure]);

  if (!content) {
    return matchHeight > 0 ? <div aria-hidden style={{ height: matchHeight }} /> : null;
  }
  return (
    <div ref={ref} style={matchHeight > 0 ? { minHeight: matchHeight } : undefined}>
      {content}
    </div>
  );
}

export const DiffSplitView = forwardRef<DiffViewHandle, Props>(
  function DiffSplitView({ lines, selection, flow = false }, ref) {
    const leftRef = useRef<HTMLDivElement>(null);
    const rightRef = useRef<HTMLDivElement>(null);
    const isSyncing = useRef(false);

    const pairs = buildSplitPairs(lines);

    // Resolved once per pair rather than inside each column, so a column
    // that is drawing nothing still knows the other one is.
    const unders = pairs.map((pair) => ({
      left: underFor(selection, pair.left, "LEFT"),
      right: underFor(selection, pair.right, "RIGHT"),
    }));

    const [underHeights, setUnderHeights] = useState<Record<string, number>>({});
    const measureUnder = useCallback((id: string, height: number) => {
      setUnderHeights((prev) => (prev[id] === height ? prev : { ...prev, [id]: height }));
    }, []);

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
              <Fragment key={i}>
                <SplitSideLine
                  line={pair.left}
                  side="left"
                  selection={selection}
                  flow={flow}
                />
                {(unders[i].left || unders[i].right) && (
                  <UnderRow
                    id={`${i}:left`}
                    content={unders[i].left}
                    matchHeight={underHeights[`${i}:right`] ?? 0}
                    onMeasure={measureUnder}
                  />
                )}
              </Fragment>
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
              <Fragment key={i}>
                <SplitSideLine
                  line={pair.right}
                  side="right"
                  selection={selection}
                  flow={flow}
                />
                {(unders[i].right || unders[i].left) && (
                  <UnderRow
                    id={`${i}:right`}
                    content={unders[i].right}
                    matchHeight={underHeights[`${i}:left`] ?? 0}
                    onMeasure={measureUnder}
                  />
                )}
              </Fragment>
            ))}
          </div>
        </div>
      </div>
    );
  },
);
