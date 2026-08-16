import { useEffect, useRef, useState } from "react";
import { btnCard, btnCardStrong } from "./review-ui";

interface Props {
  /** "line 43" or "lines 43–44". */
  label: string;
  /** Editing an existing note rather than writing a new one. */
  initialBody?: string;
  busy?: boolean;
  /** Absent when the host can't post a single comment outside a review
   *  — the control is then not drawn at all rather than drawn and
   *  refused. */
  onCommentNow?: (body: string) => void;
  onAddToReview: (body: string) => void;
  onCancel: () => void;
}

/**
 * The card under a selection.
 *
 * "Add to review" is the emphasized one deliberately: the default act
 * here is building a review nobody has seen yet, not publishing a
 * remark mid-read. "Comment now" is the exception, and it says so by
 * being the quieter of the two.
 *
 * The canvas also shows "Suggest a fix". It is not drawn — there is no
 * suggestion pipeline behind it in this ship, and a button that opens
 * nothing is worse than a button that isn't there.
 */
export function ReviewLineComposer({
  label,
  initialBody = "",
  busy = false,
  onCommentNow,
  onAddToReview,
  onCancel,
}: Props) {
  const [body, setBody] = useState(initialBody);
  const ref = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    ref.current?.focus();
  }, []);

  const submit = () => {
    const text = body.trim();
    if (!text) return;
    onAddToReview(text);
  };

  return (
    <div
      data-testid="line-composer"
      className="my-1 ml-[72px] mr-3 overflow-hidden rounded-lg bg-muted/40 ring-1 ring-accent-ember/30"
    >
      <div className="flex items-center gap-2 border-b border-border/40 px-2.5 py-1.5">
        <span className="font-mono text-[10px] text-accent-ember">{label}</span>
        <span className="flex-1" />
        <span className="text-[10px] text-muted-foreground">esc to cancel</span>
      </div>
      <div className="flex flex-col gap-2 px-2.5 py-2">
        <textarea
          ref={ref}
          rows={3}
          value={body}
          placeholder="Leave a note on these lines…"
          onChange={(e) => setBody(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              e.preventDefault();
              onCancel();
              return;
            }
            if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
              e.preventDefault();
              submit();
            }
          }}
          data-testid="line-composer-body"
          className="w-full resize-y rounded-md border-0 bg-background px-2 py-1.5 font-sans text-[11.5px] leading-relaxed text-foreground outline-none placeholder:text-muted-foreground focus-visible:ring-[1.5px] focus-visible:ring-ring/60"
        />
        <div className="flex items-center gap-1.5">
          <button
            type="button"
            className={btnCardStrong}
            data-testid="add-to-review"
            disabled={busy || !body.trim()}
            onClick={submit}
          >
            Add to review
          </button>
          {onCommentNow && (
            <button
              type="button"
              className={btnCard}
              data-testid="comment-now"
              disabled={busy || !body.trim()}
              onClick={() => onCommentNow(body.trim())}
            >
              {busy ? "Posting" : "Comment now"}
            </button>
          )}
          <span className="flex-1" />
          <span className="font-mono text-[10px] text-muted-foreground">⌘↵</span>
        </div>
      </div>
    </div>
  );
}
