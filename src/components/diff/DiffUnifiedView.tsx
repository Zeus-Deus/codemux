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
    const hunkIndices = lines.reduce<number[]>((acc, line, i) => {
      if (line.type === "hunk-header") acc.push(i);
      return acc;
    }, []);

    const scrollToHunk = useCallback(
      (direction: 1 | -1) => {
        const container = containerRef.current;
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
          container.scrollTo({ top: targetIdx * lineHeight, behavior: "smooth" });
        }
      },
      [hunkIndices],
    );

    useImperativeHandle(ref, () => ({ scrollToHunk }), [scrollToHunk]);

    return (
      <div
        ref={containerRef}
        className="flex-1 overflow-auto bg-card font-mono text-xs leading-[18px]"
      >
        <div className="py-0.5">
          {lines.map((line, i) => {
            if (line.type === "hunk-header") {
              return (
                <div
                  key={i}
                  className="flex min-h-[18px] bg-muted/30 whitespace-pre mt-1 first:mt-0"
                >
                  <span className="w-[72px] shrink-0" />
                  <span className="text-muted-foreground/60 text-[11px] px-3">
                    {line.content}
                  </span>
                </div>
              );
            }

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
                <span className="flex-1 min-w-0 pr-4">{line.content}</span>
              </div>
            );
          })}
        </div>
      </div>
    );
  },
);
