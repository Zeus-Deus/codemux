import { useMemo, useState } from "react";
import { MarkdownRendered } from "@/components/editor/MarkdownRendered";
import { DiffUnifiedView } from "@/components/diff/DiffUnifiedView";
import { Eyebrow } from "@/components/ui/eyebrow";
import type { DiffLine } from "@/lib/diff-parser";
import { splitSuggestions } from "@/lib/pr-suggestion";
import { toast } from "@/lib/toast";
import { cn } from "@/lib/utils";
import { btnCardXs } from "./review-ui";

/** Where a suggestion in this comment would land. */
export interface SuggestionTarget {
  /** First line the suggestion replaces. */
  start: number;
  /** The lines it replaces, read off the diff; null when the diff does
   *  not have them, in which case only the proposed lines are shown. */
  original: string[] | null;
  /** Absent ⇒ no Apply button: the branch is not checked out here, or
   *  the original lines are unknown. */
  onApply?: (replacement: string[]) => Promise<unknown>;
  /** Whether this replacement was already applied in this session, so a
   *  remounted block shows Applied instead of offering it again. */
  isApplied?: (replacement: string[]) => boolean;
}

/**
 * A review comment's body, rendered the way its author saw it.
 *
 * Reviewers and bots write Markdown — fences, lists, links — and GitHub's
 * ```suggestion blocks, which read as noise when shown raw. Prose goes
 * through the same renderer as the PR description; suggestions become a
 * small diff that can be applied.
 */
export function CommentBody({
  body,
  target,
  className,
}: {
  body: string;
  target?: SuggestionTarget | null;
  className?: string;
}) {
  const segments = useMemo(() => splitSuggestions(body), [body]);
  return (
    <div
      data-testid="comment-body"
      className={cn("pr-reading min-w-0 select-text break-words", className)}
    >
      {segments.map((segment, i) =>
        segment.kind === "markdown" ? (
          <MarkdownRendered key={i} content={segment.text} inline />
        ) : (
          <SuggestionBlock key={i} lines={segment.lines} target={target ?? null} />
        ),
      )}
    </div>
  );
}

function SuggestionBlock({
  lines,
  target,
}: {
  lines: string[];
  target: SuggestionTarget | null;
}) {
  const [state, setState] = useState<"idle" | "applying" | "applied">(() =>
    target?.isApplied?.(lines) ? "applied" : "idle",
  );

  const diffLines = useMemo(() => {
    const start = target?.start ?? null;
    const out: DiffLine[] = [];
    (target?.original ?? []).forEach((content, i) =>
      out.push({
        type: "del",
        content,
        oldLine: start == null ? null : start + i,
        newLine: null,
      }),
    );
    lines.forEach((content, i) =>
      out.push({
        type: "add",
        content,
        oldLine: null,
        newLine: start == null ? null : start + i,
      }),
    );
    return out;
  }, [lines, target]);

  const onApply = target?.onApply;
  const apply = () => {
    if (!onApply || state !== "idle") return;
    setState("applying");
    onApply(lines)
      .then(() => setState("applied"))
      .catch((err) => {
        setState("idle");
        toast.error(err instanceof Error ? err.message : String(err));
      });
  };

  return (
    <div
      data-testid="comment-suggestion"
      className="my-1.5 overflow-hidden rounded-md bg-surface-1"
    >
      <div className="flex items-center gap-2 px-2 py-1">
        <Eyebrow tone="strong" className="flex-1">
          Suggested change
        </Eyebrow>
        {onApply && (
          <button
            type="button"
            data-testid="apply-suggestion"
            className={btnCardXs}
            disabled={state !== "idle"}
            onClick={apply}
          >
            {state === "applied"
              ? "Applied"
              : state === "applying"
                ? "Applying"
                : "Apply suggestion"}
          </button>
        )}
      </div>
      <div className="overflow-x-auto">
        <DiffUnifiedView lines={diffLines} flow />
      </div>
    </div>
  );
}
