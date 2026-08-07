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
import { CollapsibleSection } from "./collapsible-section";

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
      <span className="text-[10px] font-medium text-muted-foreground">
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
      className="h-4 w-4 opacity-0 group-hover/comment:opacity-100 transition-opacity"
      onClick={handleCopy}
      title="Copy comment"
    >
      {copied ? (
        <span className="text-[8px] text-success">ok</span>
      ) : (
        <Copy className="h-2.5 w-2.5" />
      )}
    </Button>
  );
}

interface Props {
  reviews: ReviewComment[];
  inlineComments: InlineReviewComment[];
  isLoading?: boolean;
}

interface GroupedReview {
  review: ReviewComment;
  inlineComments: InlineReviewComment[];
}

export function ReviewThreads({ reviews, inlineComments, isLoading = false }: Props) {
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
            <p className="text-xs text-muted-foreground px-1 py-0.5">
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
            <div className="flex items-center gap-1.5 px-1">
              <AuthorAvatar name={g.review.author} />
              <span className="text-xs font-medium text-foreground truncate">
                {g.review.author}
              </span>
              <ReviewStateIcon state={g.review.state} />
              {g.review.created_at && (
                <span className="text-[10px] text-muted-foreground">
                  {formatDate(g.review.created_at)}
                </span>
              )}
            </div>

            {/* Review body */}
            {g.review.body && (
              <div className="group/comment flex items-start gap-1 pl-7 pr-1">
                <p className="select-text text-xs text-muted-foreground flex-1 whitespace-pre-wrap break-words">
                  {g.review.body}
                </p>
                <CopyButton text={g.review.body} />
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
                className="group/comment ml-7 mr-1 border-l-2 border-border/50 pl-2 space-y-0.5"
              >
                <div className="flex items-center gap-1">
                  <span className="text-[10px] font-mono bg-muted px-1 rounded text-muted-foreground truncate">
                    {ic.path}
                    {ic.line != null && `:${ic.line}`}
                  </span>
                  <CopyButton text={ic.body} />
                </div>
                <p className="select-text text-xs text-muted-foreground whitespace-pre-wrap break-words">
                  {ic.body}
                </p>
              </div>
            ))}
          </div>
        ))}
      </div>
    </CollapsibleSection>
  );
}
