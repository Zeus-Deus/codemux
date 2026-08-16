import { useState } from "react";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { cn } from "@/lib/utils";
import { btnCard, btnCardStrong, btnEmberSolid } from "./review-ui";
import { rangeLabel } from "./review-code-tab";
import type { LineDraft } from "./pr-drafts";

export interface VerdictOption {
  id: string;
  label: string;
}

const VERDICTS: VerdictOption[] = [
  { id: "comment", label: "Comment" },
  { id: "approve", label: "Approve" },
  { id: "request-changes", label: "Request changes" },
];

interface Props {
  open: boolean;
  prNumber: number;
  drafts: LineDraft[];
  /** The action bar's half-written review — one draft pool, not two. */
  initialBody: string;
  initialVerdict: string;
  /** GitLab has no request-changes verdict, so it isn't offered. */
  canRequestChanges: boolean;
  submitting: boolean;
  /** Why this can't be sent, in words. Null when it can. */
  blockedReason: string | null;
  onReanchor: () => void;
  onCancel: () => void;
  onSubmit: (verdict: string, body: string) => void;
}

/**
 * What you're about to send, before you send it.
 *
 * The notes are listed in full rather than counted: a pending review is
 * written over an hour of reading, and the last chance to notice that
 * one of them was a note to yourself is here.
 */
export function ReviewSubmitSheet({
  open,
  prNumber,
  drafts,
  initialBody,
  initialVerdict,
  canRequestChanges,
  submitting,
  blockedReason,
  onReanchor,
  onCancel,
  onSubmit,
}: Props) {
  const [verdict, setVerdict] = useState(initialVerdict);
  const [body, setBody] = useState(initialBody);

  const options = canRequestChanges
    ? VERDICTS
    : VERDICTS.filter((v) => v.id !== "request-changes");

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next && !submitting) onCancel();
      }}
    >
      <DialogContent
        showCloseButton={false}
        className="gap-0 p-0 sm:max-w-md"
        data-testid="submit-sheet"
      >
        <div className="flex items-center gap-2 border-b border-border/40 px-3.5 pb-2.5 pt-3">
          <DialogTitle className="flex-1 text-[12.5px] font-semibold">
            Submit review on #{prNumber}
          </DialogTitle>
          <div className="flex gap-px rounded-lg bg-muted/60 p-0.5" role="radiogroup" aria-label="Verdict">
            {options.map((option) => (
              <button
                key={option.id}
                type="button"
                role="radio"
                aria-checked={verdict === option.id}
                data-testid={`verdict-option-${option.id}`}
                onClick={() => setVerdict(option.id)}
                className={cn(
                  "rounded-md px-2 py-1 text-[11px] transition-colors",
                  verdict === option.id
                    ? "bg-background font-semibold text-foreground"
                    : "text-muted-foreground hover:text-foreground",
                )}
              >
                {option.label}
              </button>
            ))}
          </div>
        </div>

        <div className="flex flex-col gap-2 px-3.5 py-3">
          <textarea
            rows={3}
            value={body}
            placeholder="Anything to say about the review as a whole…"
            onChange={(e) => setBody(e.target.value)}
            data-testid="submit-sheet-body"
            className="w-full resize-y rounded-md border-0 bg-background px-2 py-1.5 text-[11.5px] leading-relaxed text-foreground outline-none placeholder:text-muted-foreground focus-visible:ring-[1.5px] focus-visible:ring-ring/60"
          />

          <p className="text-[10px] font-semibold uppercase tracking-wide text-muted-foreground">
            {drafts.length === 1 ? "1 note" : `${drafts.length} notes`}
          </p>
          <ul className="flex max-h-52 flex-col gap-1.5 overflow-auto" data-testid="submit-sheet-notes">
            {drafts.map((note) => (
              <li key={note.id} className="flex gap-2 text-[11px] leading-snug">
                <span className="shrink-0 font-mono text-[10px] text-muted-foreground">
                  {note.path}:{rangeLabel(note.startLine, note.line).replace(/^lines? /, "")}
                </span>
                <span className="min-w-0 flex-1 text-foreground/85">{note.body}</span>
                {note.status === "unanchored" && (
                  <span className="shrink-0 text-[10px] font-semibold text-status-working">
                    unanchored
                  </span>
                )}
              </li>
            ))}
          </ul>
        </div>

        {blockedReason && (
          <p
            data-testid="submit-blocked-reason"
            className="border-t border-border/40 bg-status-working/10 px-3.5 py-2 text-[11px] leading-snug text-foreground/85"
          >
            {blockedReason}
          </p>
        )}

        <div className="flex items-center gap-2 border-t border-border/40 px-3.5 py-2.5">
          <span className="flex-1 text-[10.5px] text-muted-foreground">
            Sent as one request — all of it, or none of it.
          </span>
          <button type="button" className={btnCard} onClick={onCancel} disabled={submitting}>
            Cancel
          </button>
          {blockedReason ? (
            <button
              type="button"
              className={btnCardStrong}
              data-testid="submit-reanchor"
              onClick={onReanchor}
            >
              Re-anchor
            </button>
          ) : (
            <button
              type="button"
              className={btnEmberSolid}
              data-testid="submit-review-confirm"
              disabled={submitting}
              onClick={() => onSubmit(verdict, body)}
            >
              {submitting ? "Sending" : "Submit review"}
            </button>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
