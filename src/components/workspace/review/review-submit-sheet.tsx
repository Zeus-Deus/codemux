import { useState } from "react";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { cn } from "@/lib/utils";
import {
  btnCard,
  btnCardStrong,
  btnEmberSolid,
  reviewBodyRequirement,
  tzBody,
  tzBodyLg,
  tzMeta,
  tzMetaNum,
  tzPanelHeader,
} from "./review-ui";
import { rangeLabel } from "./review-code-tab";
import { setLastVerdict, setReviewDraft, type DraftKey, type LineDraft } from "./pr-drafts";

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
  /** Where the body and the verdict live between visits to this sheet. */
  draftKey: DraftKey;
  drafts: LineDraft[];
  /** The action bar's half-written review — one draft pool, not two. */
  initialBody: string;
  initialVerdict: string;
  /** GitLab has no request-changes verdict, so it isn't offered. */
  canRequestChanges: boolean;
  /** Declared separately from request-changes: a host can serve one
   *  verdict and not the other. */
  canApprove: boolean;
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
  draftKey,
  drafts,
  initialBody,
  initialVerdict,
  canRequestChanges,
  canApprove,
  submitting,
  blockedReason,
  onReanchor,
  onCancel,
  onSubmit,
}: Props) {
  const [verdict, setVerdict] = useState(initialVerdict);
  const [body, setBody] = useState(initialBody);

  // ── Edits go back into the draft pool ──
  //
  // The sheet used to seed itself from the store and never write back,
  // so everything typed here existed only in this component's state:
  // clicking Re-anchor closed the sheet and lost it, reopening reseeded
  // from the older action-bar text, and a failed submit kept it alive
  // only in the retry buffer. There is meant to be one draft per pull
  // request, not one per surface that can edit it, so every keystroke
  // lands in the same place the action bar reads from and this sheet
  // reseeds from next time.
  const editBody = (value: string) => {
    setBody(value);
    setReviewDraft(draftKey, value);
  };

  const pickVerdict = (id: string) => {
    setVerdict(id);
    setLastVerdict(draftKey, id);
  };

  // Two reasons a send can be blocked, and they are not interchangeable:
  // `blockedReason` is about the notes and offers Re-anchor, this one is
  // about the body and is answered by typing. Both are said in words
  // above the footer; the notes take precedence because re-anchoring is
  // the bigger interruption.
  const bodyRequirement = reviewBodyRequirement(verdict, body);
  const blocked = blockedReason ?? bodyRequirement;

  // Rendered from the declarations, so the sheet and the action bar
  // cannot disagree about which verdicts this host has.
  const options = VERDICTS.filter(
    (v) =>
      (v.id !== "request-changes" || canRequestChanges) &&
      (v.id !== "approve" || canApprove),
  );

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
          <DialogTitle className={cn("flex-1 font-semibold", tzPanelHeader)}>
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
                onClick={() => pickVerdict(option.id)}
                className={cn(
                  "rounded-md px-2.5 py-1.5 transition-colors",
                  tzBody,
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

        <div className="flex flex-col gap-2.5 px-3.5 py-3">
          <textarea
            rows={3}
            value={body}
            placeholder="Anything to say about the review as a whole…"
            onChange={(e) => editBody(e.target.value)}
            data-testid="submit-sheet-body"
            className={cn(
              "w-full resize-y rounded-md border-0 bg-background px-2.5 py-2 leading-relaxed text-foreground outline-none placeholder:text-muted-foreground focus-visible:ring-[1.5px] focus-visible:ring-ring/60",
              tzBodyLg,
            )}
          />

          <p
            className={cn(
              "font-semibold uppercase tracking-wide text-muted-foreground",
              tzMeta,
            )}
          >
            {drafts.length === 1 ? "1 note" : `${drafts.length} notes`}
          </p>
          <ul className="flex max-h-56 flex-col gap-2 overflow-auto" data-testid="submit-sheet-notes">
            {drafts.map((note) => (
              <li key={note.id} className={cn("flex gap-2 leading-snug", tzBody)}>
                <span className={cn("shrink-0 font-mono text-muted-foreground", tzMeta)}>
                  {note.path}:{rangeLabel(note.startLine, note.line).replace(/^lines? /, "")}
                </span>
                <span className="min-w-0 flex-1 text-foreground/85">{note.body}</span>
                {note.status === "unanchored" && (
                  <span className={cn("shrink-0 font-semibold text-status-working", tzMeta)}>
                    unanchored
                  </span>
                )}
              </li>
            ))}
          </ul>
        </div>

        {blocked && (
          <p
            data-testid={
              blockedReason ? "submit-blocked-reason" : "submit-body-required"
            }
            className={cn(
              "border-t border-border/40 bg-status-working/10 px-3.5 py-2.5 leading-snug text-foreground/85",
              tzBody,
            )}
          >
            {blocked}
          </p>
        )}

        <div className="flex items-center gap-2 border-t border-border/40 px-3.5 py-2.5">
          <span className={cn("flex-1 text-muted-foreground", tzMetaNum)}>
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
              // A wordless comment or request-for-changes is refused by
              // the host, so it is refused here — with the reason above,
              // and a textarea right there to answer it.
              disabled={submitting || bodyRequirement != null}
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
