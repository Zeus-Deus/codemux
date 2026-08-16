import { useState, useMemo } from "react";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import {
  ShieldCheck,
  ShieldAlert,
  MessageSquare,
  Copy,
  Clock,
} from "lucide-react";
import type { ReviewComment, InlineReviewComment } from "@/tauri/types";
import type { ReviewThreadTask } from "@/lib/pr-agent-handoff";
import { toast } from "@/lib/toast";
import { cn } from "@/lib/utils";
import { CollapsibleSection } from "./collapsible-section";
import {
  btnEmberXs,
  tzBody,
  tzBodyLg,
  tzEyebrow,
  tzMeta,
  tzRowTitle,
} from "./review-ui";

function ReviewStateIcon({ state }: { state: string }) {
  if (state === "APPROVED")
    return <ShieldCheck className="h-3 w-3 text-success shrink-0" />;
  if (state === "CHANGES_REQUESTED")
    return <ShieldAlert className="h-3 w-3 text-warning shrink-0" />;
  if (state === "PENDING")
    return <Clock className="h-3 w-3 text-muted-foreground shrink-0" />;
  return <MessageSquare className="h-3 w-3 text-muted-foreground shrink-0" />;
}

function AuthorAvatar({ name }: { name: string }) {
  const initial = name ? name[0].toUpperCase() : "?";
  return (
    <div className="h-5 w-5 rounded-full bg-muted flex items-center justify-center shrink-0">
      <span className={cn("font-medium text-muted-foreground", tzMeta)}>
        {initial}
      </span>
    </div>
  );
}

function formatDate(dateStr: string): string {
  if (!dateStr) return "";
  const date = new Date(dateStr);
  const now = new Date();
  const diffMs = now.getTime() - date.getTime();
  const diffHours = Math.floor(diffMs / 3600000);
  if (diffHours < 1) return "just now";
  if (diffHours < 24) return `${diffHours}h ago`;
  const diffDays = Math.floor(diffHours / 24);
  if (diffDays < 30) return `${diffDays}d ago`;
  return date.toLocaleDateString();
}

function CopyButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);

  const handleCopy = () => {
    navigator.clipboard.writeText(text);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  return (
    <Button
      size="icon-xs"
      variant="ghost"
      className="h-[18px] w-[18px] opacity-0 group-hover/comment:opacity-100 transition-opacity"
      onClick={handleCopy}
      title="Copy comment"
    >
      {copied ? (
        <span className={cn("text-success", tzEyebrow)}>ok</span>
      ) : (
        <Copy className="h-3 w-3" />
      )}
    </Button>
  );
}

/**
 * The one action a thread can actually offer today.
 *
 * Reply and Resolve are in the mock, and neither exists at any layer of
 * this app — no reply-to-thread command, no resolve-thread command. They
 * are therefore not drawn: a button that answers a click with an apology
 * is worse than the reviewer's own comment sitting there unanswered.
 * Handing the comment to an agent is the leg that works, so it is the
 * leg that ships.
 */
function SendToAgentButton({ onSend }: { onSend: () => Promise<unknown> }) {
  const [sending, setSending] = useState(false);
  return (
    <button
      type="button"
      className={btnEmberXs}
      data-testid="send-thread-to-agent"
      disabled={sending}
      onClick={() => {
        if (sending) return;
        setSending(true);
        onSend()
          .catch((err) => toast.error(String(err)))
          .finally(() => setSending(false));
      }}
    >
      {sending ? (
        <>
          <span
            aria-hidden
            className="size-1.5 animate-spin rounded-full border-[1.5px] border-current border-r-transparent"
          />
          Sending
        </>
      ) : (
        "Send to agent"
      )}
    </button>
  );
}

interface Props {
  reviews: ReviewComment[];
  inlineComments: InlineReviewComment[];
  isLoading?: boolean;
  /** Hands one comment to an agent. Absent ⇒ no thread actions drawn. */
  onSendToAgent?: (task: ReviewThreadTask) => Promise<unknown>;
}

interface GroupedReview {
  review: ReviewComment;
  inlineComments: InlineReviewComment[];
}

export function ReviewThreads({
  reviews,
  inlineComments,
  isLoading = false,
  onSendToAgent,
}: Props) {
  const grouped = useMemo(() => {
    // Group inline comments by pull_request_review_id
    const inlineByReview = new Map<number, InlineReviewComment[]>();
    const orphanInline: InlineReviewComment[] = [];

    for (const c of inlineComments) {
      if (c.in_reply_to_id) continue; // skip threaded replies, show only top-level
      if (c.pull_request_review_id) {
        const existing = inlineByReview.get(c.pull_request_review_id) ?? [];
        existing.push(c);
        inlineByReview.set(c.pull_request_review_id, existing);
      } else {
        orphanInline.push(c);
      }
    }

    const result: GroupedReview[] = reviews.map((r) => ({
      review: r,
      inlineComments: inlineByReview.get(r.id) ?? [],
    }));

    // Add orphan inline comments as standalone entries
    if (orphanInline.length > 0) {
      result.push({
        review: {
          id: 0,
          author: orphanInline[0].author,
          body: "",
          state: "COMMENTED",
          created_at: orphanInline[0].created_at,
        },
        inlineComments: orphanInline,
      });
    }

    return result;
  }, [reviews, inlineComments]);

  const totalCount = reviews.length + inlineComments.filter((c) => !c.in_reply_to_id).length;

  return (
    <CollapsibleSection label="Comments" count={totalCount}>
      <div className="px-1.5 space-y-2">
        {totalCount === 0 ? (
          isLoading ? (
            <>
              <Skeleton className="h-4 w-3/4 mx-1 my-0.5" />
              <Skeleton className="h-4 w-2/3 mx-1 my-0.5" />
              <Skeleton className="h-4 w-1/2 mx-1 my-0.5" />
            </>
          ) : (
            <p className={cn("text-muted-foreground px-1 py-1", tzBody)}>
              No comments yet.
            </p>
          )
        ) : null}
        {totalCount > 0 && grouped.map((g) => (
          // Composite key (`review.id` is unique within reviews; orphan
          // group falls back to its first inline comment's id since array
          // index would lose collapse state on reorder — analysis §4).
          <div
            key={
              g.review.id !== 0
                ? `review-${g.review.id}`
                : `orphan-${g.inlineComments[0]?.id ?? "empty"}`
            }
            className="space-y-1"
          >
            {/* Review header */}
            <div className="flex items-center gap-1.5 px-1 py-0.5">
              <AuthorAvatar name={g.review.author} />
              <span className={cn("font-medium text-foreground truncate", tzRowTitle)}>
                {g.review.author}
              </span>
              <ReviewStateIcon state={g.review.state} />
              {g.review.created_at && (
                <span className={cn("text-muted-foreground", tzMeta)}>
                  {formatDate(g.review.created_at)}
                </span>
              )}
            </div>

            {/* Review body */}
            {g.review.body && (
              <div className="space-y-1 pl-7 pr-1">
                <div className="group/comment flex items-start gap-1.5">
                  <p
                    className={cn(
                      "select-text text-muted-foreground flex-1 whitespace-pre-wrap break-words",
                      tzBodyLg,
                    )}
                  >
                    {g.review.body}
                  </p>
                  <CopyButton text={g.review.body} />
                </div>
                {/* An inline comment carries its own, better-anchored
                    action; the review body only earns one when it is the
                    whole of what the reviewer said. */}
                {onSendToAgent && g.inlineComments.length === 0 && (
                  <SendToAgentButton
                    onSend={() =>
                      onSendToAgent({
                        kind: "review-thread",
                        reviewer: g.review.author,
                        body: g.review.body,
                        verdict: g.review.state,
                      })
                    }
                  />
                )}
              </div>
            )}

            {/* Inline comments */}
            {g.inlineComments.map((ic) => (
              // Composite key prevents collisions when the same inline
              // comment id is re-used across review threads (rare but
              // GitHub does it for "comment on the same line in two
              // reviews"). `g.review.id` is 0 for the orphan group;
              // that's still unique relative to keyed reviews.
              <div
                key={`${g.review.id}-${ic.id}`}
                className="group/comment ml-7 mr-1 border-l-2 border-border/50 pl-2 space-y-1"
              >
                <div className="flex items-center gap-1.5">
                  <span
                    className={cn(
                      "font-mono bg-muted px-1.5 py-px rounded text-muted-foreground truncate",
                      tzMeta,
                    )}
                  >
                    {ic.path}
                    {ic.line != null && `:${ic.line}`}
                  </span>
                  <CopyButton text={ic.body} />
                </div>
                <p
                  className={cn(
                    "select-text text-muted-foreground whitespace-pre-wrap break-words",
                    tzBodyLg,
                  )}
                >
                  {ic.body}
                </p>
                {onSendToAgent && (
                  <SendToAgentButton
                    onSend={() =>
                      onSendToAgent({
                        kind: "review-thread",
                        reviewer: ic.author,
                        body: ic.body,
                        path: ic.path,
                        line: ic.line,
                        verdict: g.review.state,
                        // The review's own body is the context this
                        // comment was written under, when there is one.
                        parent:
                          g.review.body && g.review.id !== 0
                            ? { author: g.review.author, body: g.review.body }
                            : null,
                      })
                    }
                  />
                )}
              </div>
            ))}
          </div>
        ))}
      </div>
    </CollapsibleSection>
  );
}
