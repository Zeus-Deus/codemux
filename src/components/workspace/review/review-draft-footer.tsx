import { useState } from "react";
import { btnCard, btnEmberSolid } from "./review-ui";
import { draftCounts, type LineDraft } from "./pr-drafts";

interface Props {
  drafts: LineDraft[];
  onDiscard: () => void;
  onSubmit: () => void;
}

/**
 * The bar that exists only while you have unsent notes.
 *
 * It lives on the detail surface rather than inside the Code tab, so
 * walking back to Summary — or to another PR and back — never loses
 * sight of a review nobody has seen. "not visible to anyone yet" is the
 * sentence doing the real work: the whole point of a pending review is
 * that it isn't one yet.
 */
export function ReviewDraftFooter({ drafts, onDiscard, onSubmit }: Props) {
  const [confirming, setConfirming] = useState(false);
  const { notes, files, unanchored } = draftCounts(drafts);

  return (
    <div
      data-testid="draft-footer"
      className="flex items-center gap-2 border-t border-border/40 bg-card px-3 py-2"
    >
      <span aria-hidden className="size-1.5 shrink-0 rounded-full bg-accent-ember" />
      <span className="min-w-0 flex-1 text-[11px]">
        <span className="font-semibold text-foreground" data-testid="draft-count">
          {notes === 1 ? "1 pending" : `${notes} pending`}
        </span>{" "}
        <span className="text-muted-foreground">
          on {files === 1 ? "1 file" : `${files} files`} · not visible to anyone yet
          {unanchored > 0 && ` · ${unanchored} unanchored`}
        </span>
      </span>
      {confirming ? (
        <>
          <span className="shrink-0 text-[11px] text-muted-foreground">
            Discard {notes === 1 ? "it" : "them"}?
          </span>
          <button type="button" className={btnCard} onClick={() => setConfirming(false)}>
            Keep
          </button>
          <button
            type="button"
            className={btnCard}
            data-testid="discard-confirm"
            onClick={() => {
              setConfirming(false);
              onDiscard();
            }}
          >
            Discard
          </button>
        </>
      ) : (
        <button
          type="button"
          className={btnCard}
          data-testid="discard-drafts"
          onClick={() => setConfirming(true)}
        >
          Discard
        </button>
      )}
      <button
        type="button"
        className={btnEmberSolid}
        data-testid="open-submit-sheet"
        onClick={onSubmit}
      >
        Submit review <span className="text-[9px] opacity-70">▾</span>
      </button>
    </div>
  );
}
