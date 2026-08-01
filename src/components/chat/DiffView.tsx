import { Check, Copy, FileText } from "lucide-react";
import { useState } from "react";

import { cn } from "@/lib/utils";

/**
 * Diff surface (design D7). Computes a line-level diff from the tool
 * input (old/new text) — context lines dimmed with a running line number,
 * removed lines red on a faint red wash with a `−` gutter, added lines
 * green on a faint green wash with a `+` gutter. Horizontal overflow
 * scrolls inside the card. The +N/−N counts and copy target come from the
 * computed rows, not the model's prose.
 */
export function DiffView({
  filename,
  oldText,
  newText,
  copyText,
}: {
  filename: string | null;
  oldText: string;
  newText: string;
  copyText: string;
}) {
  const rows = computeLineDiff(oldText, newText);
  const added = rows.filter((r) => r.type === "add").length;
  const removed = rows.filter((r) => r.type === "remove").length;

  let lineNo = 0;

  return (
    <div className="overflow-hidden rounded-[11px] border border-border/60 bg-muted/40">
      <div className="flex items-center justify-between gap-3 border-b border-border/60 px-[13px] py-[9px]">
        <span className="flex min-w-0 items-center gap-2 font-mono text-[12px] text-muted-foreground">
          <FileText
            className="h-3 w-3 shrink-0 text-muted-foreground/70"
            strokeWidth={1.4}
            aria-hidden
          />
          <span className="truncate">{filename ?? "diff"}</span>
        </span>
        <span className="flex items-center gap-2.5">
          <span className="font-mono text-[11px]">
            <span className="text-status-open">+{added}</span>{" "}
            <span className="text-status-attention">−{removed}</span>
          </span>
          <CopyButton text={copyText} />
        </span>
      </div>
      <div className="overflow-x-auto py-[9px] font-mono text-[12px] leading-[1.75]">
        {rows.map((row, i) => {
          if (row.type === "context" || row.type === "add") lineNo += 1;
          const gutter =
            row.type === "remove" ? "−" : row.type === "add" ? "+" : String(lineNo);
          return (
            <div
              key={i}
              className={cn(
                "flex w-max min-w-full",
                row.type === "context" && "text-muted-foreground",
                row.type === "remove" && "bg-status-attention/[0.09] text-status-attention",
                row.type === "add" && "bg-status-open/[0.09] text-status-open",
              )}
            >
              <span
                className={cn(
                  "w-[42px] shrink-0 pr-[14px] text-right",
                  row.type === "context" ? "opacity-60" : "opacity-70",
                )}
              >
                {gutter}
              </span>
              <span className="whitespace-pre pr-4">{row.text}</span>
            </div>
          );
        })}
      </div>
    </div>
  );
}

function CopyButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      type="button"
      aria-label="Copy"
      onClick={() => {
        void navigator.clipboard?.writeText(text);
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1500);
      }}
      className="flex text-muted-foreground/70 hover:text-foreground"
    >
      {copied ? (
        <Check className="h-3.5 w-3.5 text-status-open" strokeWidth={1.5} aria-hidden />
      ) : (
        <Copy className="h-3.5 w-3.5" strokeWidth={1.5} aria-hidden />
      )}
    </button>
  );
}

export type DiffRowType = "context" | "add" | "remove";
export interface DiffRow {
  type: DiffRowType;
  text: string;
}

/**
 * Line-level diff via a longest-common-subsequence table. Lines present
 * in both texts (in order) become context; lines only in `oldText` are
 * removals; lines only in `newText` are additions. Deterministic:
 * removals are emitted before additions at each divergence.
 */
export function computeLineDiff(oldText: string, newText: string): DiffRow[] {
  const a = oldText.length ? oldText.split("\n") : [];
  const b = newText.length ? newText.split("\n") : [];
  const n = a.length;
  const m = b.length;
  const dp: number[][] = Array.from({ length: n + 1 }, () =>
    new Array<number>(m + 1).fill(0),
  );
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      dp[i][j] =
        a[i] === b[j]
          ? dp[i + 1][j + 1] + 1
          : Math.max(dp[i + 1][j], dp[i][j + 1]);
    }
  }
  const rows: DiffRow[] = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (a[i] === b[j]) {
      rows.push({ type: "context", text: a[i] });
      i += 1;
      j += 1;
    } else if (dp[i + 1][j] >= dp[i][j + 1]) {
      rows.push({ type: "remove", text: a[i] });
      i += 1;
    } else {
      rows.push({ type: "add", text: b[j] });
      j += 1;
    }
  }
  while (i < n) {
    rows.push({ type: "remove", text: a[i] });
    i += 1;
  }
  while (j < m) {
    rows.push({ type: "add", text: b[j] });
    j += 1;
  }
  return rows;
}
