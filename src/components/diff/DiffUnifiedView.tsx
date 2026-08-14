import { useRef, useCallback, useImperativeHandle, forwardRef } from "react";
import type { DiffLine } from "@/lib/diff-parser";

interface Props {
  lines: DiffLine[];
}

export interface DiffViewHandle {
  scrollToHunk: (direction: 1 | -1) => void;
}

export const DiffUnifiedView = forwardRef<DiffViewHandle, Props>(
  function DiffUnifiedView({ lines }, ref) {
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
        className="code-surface select-text relative flex-1 overflow-auto bg-card"
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

            // Conflict marker detection
            const isOursMarker = line.content.startsWith("<<<<<<<");
            const isSeparator = line.content.startsWith("=======") && !line.content.startsWith("========");
            const isTheirsMarker = line.content.startsWith(">>>>>>>");
            const isConflictMarker = isOursMarker || isSeparator || isTheirsMarker;

            const bgClass = isOursMarker
              ? "bg-primary/15 border-l-2 border-primary"
              : isTheirsMarker
                ? "bg-accent-violet/15 border-l-2 border-accent-violet"
                : isSeparator
                  ? "bg-muted/40 border-l-2 border-muted-foreground"
                  : line.type === "add"
                    ? "bg-success/10"
                    : line.type === "del"
                      ? "bg-danger/10"
                      : "";

            const prefixChar =
              line.type === "add" ? "+" : line.type === "del" ? "-" : " ";

            const prefixColor = isConflictMarker
              ? "text-muted-foreground"
              : line.type === "add"
                ? "text-success"
                : line.type === "del"
                  ? "text-danger"
                  : "text-muted-foreground";

            return (
              <div
                key={i}
                className={`flex min-h-[18px] whitespace-pre ${bgClass}`}
              >
                <span className="flex w-[72px] shrink-0 select-none">
                  <span className="inline-block w-[36px] text-right pr-2 text-[11px] text-muted-foreground tabular-nums">
                    {line.oldLine ?? ""}
                  </span>
                  <span className="inline-block w-[36px] text-right pr-2 text-[11px] text-muted-foreground tabular-nums">
                    {line.newLine ?? ""}
                  </span>
                </span>
                <span
                  className={`inline-block w-4 shrink-0 text-center select-none ${prefixColor}`}
                >
                  {prefixChar}
                </span>
                <span className="flex-1 min-w-0 pr-4">
                  {isOursMarker && <span className="text-[9px] font-bold text-primary mr-2">OURS</span>}
                  {isTheirsMarker && <span className="text-[9px] font-bold text-accent-violet mr-2">THEIRS</span>}
                  {line.content}
                </span>
              </div>
            );
          })}
        </div>
      </div>
    );
  },
);
