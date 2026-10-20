import { Check, Copy, FileText } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";

import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { COPY_FAILED_MESSAGE, copyToClipboard } from "@/lib/clipboard";
import { toast } from "@/lib/toast";
import { cn } from "@/lib/utils";
import { parseDiff } from "@/lib/diff-parser";
import { resolveChatFileLink } from "@/lib/agent-chat/file-links";
import { MarkdownFileLink } from "./MarkdownFileLink";
import { useChatFileLinkContext } from "./chat-file-link-context";

type DiffViewSource =
  | { oldText: string; newText: string; patch?: undefined }
  /** One file's unified diff (from `git diff`), rendered hunk by hunk with
   *  the file's real line numbers. */
  | { patch: string; oldText?: undefined; newText?: undefined };

/** A rendered row: a diff line with its gutter, or a hunk separator. */
interface DisplayRow {
  type: DiffRowType | "hunk";
  text: string;
  gutter: string;
}

function textRows(oldText: string, newText: string): DisplayRow[] {
  let lineNo = 0;
  return computeLineDiff(oldText, newText).map((row) => {
    if (row.type === "context" || row.type === "add") lineNo += 1;
    const gutter =
      row.type === "remove" ? "−" : row.type === "add" ? "+" : String(lineNo);
    return { ...row, gutter };
  });
}

function patchRows(patch: string): DisplayRow[] {
  // A trailing newline would parse as one more (empty) context line.
  return parseDiff(patch.replace(/\r?\n$/, "")).map((line): DisplayRow => {
    switch (line.type) {
      case "hunk-header":
        return { type: "hunk", text: line.content, gutter: "" };
      case "add":
        return { type: "add", text: line.content, gutter: "+" };
      case "del":
        return { type: "remove", text: line.content, gutter: "−" };
      default:
        return { type: "context", text: line.content, gutter: String(line.newLine ?? "") };
    }
  });
}

/**
 * Diff surface (design D7). Renders a line-level diff computed from the
 * tool input (old/new text), or a file's unified `patch` — context lines
 * dimmed with their line number, removed lines red on a faint red wash with
 * a `−` gutter, added lines green on a faint green wash with a `+` gutter,
 * and a patch's hunks separated by their `@@` header. Horizontal overflow
 * scrolls inside the card. The +N/−N counts and copy target come from the
 * rows, not the model's prose.
 */
export function DiffView({
  filename,
  copyText,
  ...source
}: {
  filename: string | null;
  copyText: string;
} & DiffViewSource) {
  const { cwd } = useChatFileLinkContext();
  const fileMeta = filename ? resolveChatFileLink(filename, cwd) : null;
  const { oldText, newText, patch } = source;
  const { rows, added, removed } = useMemo(() => {
    const rows =
      patch !== undefined ? patchRows(patch) : textRows(oldText ?? "", newText ?? "");
    return {
      rows,
      added: rows.filter((r) => r.type === "add").length,
      removed: rows.filter((r) => r.type === "remove").length,
    };
  }, [oldText, newText, patch]);

  return (
    <div className="overflow-hidden rounded-lg border border-hairline bg-surface-1">
      <div className="flex items-center justify-between gap-3 border-b border-hairline px-[13px] py-[9px]">
        <span className="flex min-w-0 items-center gap-2 font-mono text-body-sm text-muted-foreground">
          {fileMeta ? (
            <MarkdownFileLink meta={fileMeta} variant="plain">
              {fileMeta.basename}
            </MarkdownFileLink>
          ) : (
            <>
              <FileText
                className="size-3 shrink-0 text-muted-foreground/70"
                aria-hidden
              />
              <span className="truncate">{filename ?? "diff"}</span>
            </>
          )}
        </span>
        <span className="flex items-center gap-2.5">
          <span className="font-mono text-label">
            <span className="text-status-open">+{added}</span>{" "}
            <span className="text-status-attention">−{removed}</span>
          </span>
          <CopyButton text={copyText} />
        </span>
      </div>
      <div className="code-surface select-text overflow-x-auto py-[9px]">
        {rows.map((row, i) => {
          if (row.type === "hunk") {
            return (
              <div
                key={i}
                className={cn(
                  "flex w-max min-w-full bg-muted/30 text-muted-foreground/60",
                  i > 0 && "mt-[9px]",
                )}
              >
                <span className="w-[42px] shrink-0" aria-hidden />
                <span className="whitespace-pre pr-4">{row.text}</span>
              </div>
            );
          }
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
                {row.gutter}
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
  const copiedTimer = useRef<number | null>(null);

  useEffect(
    () => () => {
      if (copiedTimer.current !== null) window.clearTimeout(copiedTimer.current);
    },
    [],
  );

  async function copyDiff() {
    if (copied) return;
    if (!(await copyToClipboard(text))) {
      toast.error(COPY_FAILED_MESSAGE);
      return;
    }
    setCopied(true);
    if (copiedTimer.current !== null) window.clearTimeout(copiedTimer.current);
    copiedTimer.current = window.setTimeout(() => {
      setCopied(false);
      copiedTimer.current = null;
    }, 1500);
  }

  const copyLabel = copied ? "Copied" : "Copy diff";
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          size="icon-xs"
          aria-label={copyLabel}
          onClick={() => void copyDiff()}
          // A 24px target in a header sized for 14px glyphs: the negative
          // margin keeps the header height unchanged.
          className="-my-1 text-muted-foreground/70 hover:text-foreground"
        >
          {copied ? (
            <Check className="size-3.5 text-status-open" aria-hidden />
          ) : (
            <Copy className="size-3.5" aria-hidden />
          )}
        </Button>
      </TooltipTrigger>
      <TooltipContent side="top">{copyLabel}</TooltipContent>
    </Tooltip>
  );
}

export type DiffRowType = "context" | "add" | "remove";
export interface DiffRow {
  type: DiffRowType;
  text: string;
}

// The LCS table holds (old lines + 1) × (new lines + 1) cells; past this,
// computeLineDiff only aligns the lines both sides share at the start and end.
export const MAX_LCS_CELLS = 1_000_000;

/**
 * Line-level diff via a longest-common-subsequence table. Lines present
 * in both texts (in order) become context; lines only in `oldText` are
 * removals; lines only in `newText` are additions. Deterministic:
 * removals are emitted before additions at each divergence.
 *
 * Inputs too large for the table keep their common leading and trailing
 * lines as context and diff only the middle, which shows as plain removals
 * then additions if it is still too large.
 */
export function computeLineDiff(oldText: string, newText: string): DiffRow[] {
  const a = oldText.length ? oldText.split("\n") : [];
  const b = newText.length ? newText.split("\n") : [];
  if ((a.length + 1) * (b.length + 1) <= MAX_LCS_CELLS) return lcsDiff(a, b);

  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start += 1;
  let endA = a.length;
  let endB = b.length;
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) {
    endA -= 1;
    endB -= 1;
  }
  const midA = a.slice(start, endA);
  const midB = b.slice(start, endB);
  const middle =
    (midA.length + 1) * (midB.length + 1) <= MAX_LCS_CELLS
      ? lcsDiff(midA, midB)
      : [
          ...midA.map((text): DiffRow => ({ type: "remove", text })),
          ...midB.map((text): DiffRow => ({ type: "add", text })),
        ];
  return [
    ...a.slice(0, start).map((text): DiffRow => ({ type: "context", text })),
    ...middle,
    ...a.slice(endA).map((text): DiffRow => ({ type: "context", text })),
  ];
}

function lcsDiff(a: string[], b: string[]): DiffRow[] {
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
