import { useState } from "react";
import { cn } from "@/lib/utils";
import { toast } from "@/lib/toast";
import { requestPrReview } from "@/tauri/commands";
import type { PullRequestInfo } from "@/tauri/types";
import { btnEmber } from "./review-ui";

/** Verdict → label + tone. GitLab never produces CHANGES_REQUESTED. */
const VERDICT: Record<string, { label: string; className: string }> = {
  APPROVED: { label: "approved", className: "text-status-open" },
  CHANGES_REQUESTED: { label: "changes", className: "text-status-working" },
  COMMENTED: { label: "commented", className: "text-muted-foreground" },
  DISMISSED: { label: "dismissed", className: "text-muted-foreground" },
  PENDING: { label: "pending", className: "text-muted-foreground" },
};

interface Props {
  pr: PullRequestInfo;
  cwd: string;
  /** False on hosts with no adapter — the row is then not rendered at
   *  all rather than drawn as a control that can't work. */
  canRequestReview: boolean;
  onRequested: () => void;
}

export function ReviewReviewers({ pr, cwd, canRequestReview, onRequested }: Props) {
  const [showInput, setShowInput] = useState(false);
  const [username, setUsername] = useState("");
  const [submitting, setSubmitting] = useState(false);

  const reviewed = pr.latest_reviews.filter((r) => r.state !== "PENDING");
  const pending = pr.review_requests;
  const hasAnyone = reviewed.length > 0 || pending.length > 0;

  const submit = async () => {
    const name = username.trim();
    if (!name || submitting) return;
    setSubmitting(true);
    try {
      await requestPrReview(cwd, pr.number, name);
      setUsername("");
      setShowInput(false);
      onRequested();
      toast.success(`Requested a review from ${name}`);
    } catch (err) {
      // The name stays in the field — a typo is worth correcting, not
      // retyping.
      toast.error(String(err));
    } finally {
      setSubmitting(false);
    }
  };

  if (hasAnyone) {
    return (
      <div
        className="flex flex-wrap items-center gap-x-2 gap-y-1"
        data-testid="review-reviewers"
      >
        {reviewed.map((review) => {
          const verdict = VERDICT[review.state] ?? VERDICT.COMMENTED;
          return (
            <span
              key={review.author}
              className="flex items-center gap-1.5 rounded-md bg-muted/50 py-0.5 pl-0.5 pr-2"
            >
              <Avatar name={review.author} />
              <span className="text-[10.5px] text-foreground/80">{review.author}</span>
              <span className={cn("text-[10px]", verdict.className)}>{verdict.label}</span>
            </span>
          );
        })}
        {pending.map((login) => (
          <span
            key={login}
            className="flex items-center gap-1.5 rounded-md bg-muted/50 py-0.5 pl-0.5 pr-2"
          >
            <Avatar name={login} />
            <span className="text-[10.5px] text-foreground/80">{login}</span>
            <span className="text-[10px] text-muted-foreground">pending</span>
          </span>
        ))}
      </div>
    );
  }

  // Absence is the action: an unreviewed PR's reviewers line is the one
  // place a Request review button belongs.
  return (
    <div
      className="flex flex-col gap-1.5 rounded-md bg-accent-ember/7 px-2 py-1.5"
      data-testid="review-reviewers"
    >
      <div className="flex items-center gap-2">
        <span className="size-[15px] shrink-0 rounded-full bg-card" aria-hidden />
        <span className="flex-1 text-[11px] text-foreground/80">
          Nobody is reviewing this yet
        </span>
        {canRequestReview && !showInput && (
          <button
            type="button"
            className={btnEmber}
            onClick={() => setShowInput(true)}
            data-testid="request-review"
          >
            Request review
          </button>
        )}
      </div>
      {showInput && (
        <div className="flex items-center gap-1.5">
          <input
            autoFocus
            value={username}
            placeholder="username"
            onChange={(e) => setUsername(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") void submit();
              if (e.key === "Escape") setShowInput(false);
            }}
            className="h-7 min-w-0 flex-1 rounded-md border-0 bg-background px-2 text-[11px] text-foreground outline-none placeholder:text-muted-foreground/70 focus-visible:ring-[1.5px] focus-visible:ring-ring/60"
            data-testid="request-review-input"
          />
          <button
            type="button"
            className={btnEmber}
            onClick={() => void submit()}
            data-testid="request-review-submit"
          >
            {submitting ? "Requesting" : "Request"}
          </button>
        </div>
      )}
    </div>
  );
}

function Avatar({ name }: { name: string }) {
  return (
    <span
      aria-hidden
      className="flex size-[15px] shrink-0 items-center justify-center rounded-full bg-card text-[8px] font-semibold uppercase text-muted-foreground"
    >
      {name.slice(0, 1)}
    </span>
  );
}
