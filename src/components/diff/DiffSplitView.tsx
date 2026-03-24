import { useRef, useCallback, useImperativeHandle, forwardRef } from "react";
import type { DiffLine } from "@/lib/diff-parser";
import { buildSplitPairs } from "@/lib/diff-parser";

interface Props {
  lines: DiffLine[];
}

export interface DiffViewHandle {
  scrollToHunk: (direction: 1 | -1) => void;
}

function SplitSideLine({
  line,
  side,
}: {
  line: DiffLine | null;
  side: "left" | "right";
}) {
  if (!line) {
    return (
      <div className="flex min-h-[18px] whitespace-pre bg-muted/5">
        <span className="w-10 shrink-0 select-none" />
        <span className="flex-1" />
      </div>
    );
  }

  if (line.type === "hunk-header") {
    return (
      <div className="flex min-h-[18px] bg-muted/30 whitespace-pre mt-1 first:mt-0">
        <span className="w-10 shrink-0 select-none" />
        <span className="text-muted-foreground/60 text-[11px] px-2 truncate">
          {line.content}
        </span>
      </div>
    );
  }

  const lineNum = side === "left" ? line.oldLine : line.newLine;

  const bgClass =
    line.type === "add"
      ? "bg-success/10"
      : line.type === "del"
        ? "bg-danger/10"
        : "";

  const prefixChar =
    line.type === "add" ? "+" : line.type === "del" ? "-" : " ";

  const prefixColor =
    line.type === "add"
      ? "text-success"
      : line.type === "del"
        ? "text-danger"
        : "text-muted-foreground";

  return (
    <div className={`flex min-h-[18px] whitespace-pre ${bgClass}`}>
      <span className="inline-block w-10 shrink-0 text-right pr-2 text-[11px] text-muted-foreground tabular-nums select-none">
        {lineNum ?? ""}
      </span>
      <span
        className={`inline-block w-4 shrink-0 text-center select-none ${prefixColor}`}
      >
        {prefixChar}
      </span>
      <span className="flex-1 min-w-0 pr-4">{line.content}</span>
    </div>
  );
}

export const DiffSplitView = forwardRef<DiffViewHandle, Props>(
  function DiffSplitView({ lines }, ref) {
    const leftRef = useRef<HTMLDivElement>(null);
    const rightRef = useRef<HTMLDivElement>(null);
    const isSyncing = useRef(false);

    const pairs = buildSplitPairs(lines);

    const hunkIndices = pairs.reduce<number[]>((acc, pair, i) => {
      if (pair.left?.type === "hunk-header") acc.push(i);
      return acc;
    }, []);

    const scrollToHunk = useCallback(
      (direction: 1 | -1) => {
        const container = leftRef.current;
        if (!container || hunkIndices.length === 0) return;

        const scrollTop = container.scrollTop;
        const lineHeight = 18;
        const currentLine = Math.floor(scrollTop / lineHeight);

        let targetIdx: number | undefined;
        if (direction === 1) {
          targetIdx = hunkIndices.find((i) => i > currentLine + 1);
        } else {
          for (let j = hunkIndices.length - 1; j >= 0; j--) {
            if (hunkIndices[j] < currentLine) {
              targetIdx = hunkIndices[j];
              break;
            }
          }
        }

        if (targetIdx !== undefined) {
          const top = targetIdx * lineHeight;
          leftRef.current?.scrollTo({ top, behavior: "smooth" });
          rightRef.current?.scrollTo({ top, behavior: "smooth" });
        }
      },
      [hunkIndices],
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

    return (
      <div className="flex-1 grid grid-cols-[1fr_1px_1fr] min-h-0 overflow-hidden font-mono text-xs leading-[18px]">
        {/* Left — deletions / old */}
        <div
          ref={leftRef}
          className="overflow-auto bg-card"
          onScroll={handleScroll("left")}
        >
          <div className="py-0.5">
            {pairs.map((pair, i) => (
              <SplitSideLine key={i} line={pair.left} side="left" />
            ))}
          </div>
        </div>

        {/* Divider */}
        <div className="bg-border/50" />

        {/* Right — additions / new */}
        <div
          ref={rightRef}
          className="overflow-auto bg-card"
          onScroll={handleScroll("right")}
        >
          <div className="py-0.5">
            {pairs.map((pair, i) => (
              <SplitSideLine key={i} line={pair.right} side="right" />
            ))}
          </div>
        </div>
      </div>
    );
  },
);
