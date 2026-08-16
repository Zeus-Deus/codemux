import { useEffect, useMemo, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import {
  ShieldCheck,
  ShieldAlert,
  MessageSquare,
  Copy,
  Clock,
  ChevronRight,
} from "lucide-react";
import type {
  InlineReviewComment,
  PrReviewThread,
  PrThreadComment,
  ReviewComment,
} from "@/tauri/types";
import type { ReviewThreadTask } from "@/lib/pr-agent-handoff";
import { toast } from "@/lib/toast";
import { cn } from "@/lib/utils";
import { CollapsibleSection } from "./collapsible-section";
import {
  btnCard,
  btnCardXs,
  btnEmberXs,
  tzBody,
  tzEyebrow,
  tzMeta,
  tzMetaNum,
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

/** `src/a.ts:84` — the mono pill a thread is anchored by. */
function AnchorPill({ path, line }: { path: string | null; line: number | null }) {
  if (!path) return null;
  return (
    <span
      className={cn(
        "font-mono bg-muted px-1.5 py-px rounded text-muted-foreground truncate",
        tzMeta,
      )}
    >
      {path}
      {line != null && `:${line}`}
    </span>
  );
}

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

/**
 * The reply box on one thread.
 *
 * The whole of binding rule 4 lives in the `.then` below: the text is
 * cleared *after* the host has accepted it and at no other moment. A
 * failed reply keeps every word and says so, with Retry sending exactly
 * what failed — nobody retypes a paragraph because a laptop was on a
 * train.
 *
 * Escape blurs rather than discards, for the same reason.
 */
function ThreadReplyBox({
  threadId,
  onReply,
}: {
  threadId: string;
  onReply: (body: string) => Promise<unknown>;
}) {
  const [body, setBody] = useState("");
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const ref = useRef<HTMLTextAreaElement>(null);

  /**
   * One line at rest, as tall as what you have written after that.
   *
   * A fixed two-row box is both too big in a list of threads and too
   * small for a real reply — at three sentences it hides its own first
   * line, which is the one thing a composer may never do. Capped so a
   * long reply scrolls instead of pushing the thread off screen.
   */
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.style.height = "0px";
    el.style.height = `${Math.min(el.scrollHeight, 180)}px`;
  }, [body]);

  const send = () => {
    const text = body.trim();
    if (!text || sending) return;
    setSending(true);
    setError(null);
    onReply(text)
      .then(() => setBody(""))
      .catch((err) => setError(String(err)))
      .finally(() => setSending(false));
  };

  return (
    <div className="space-y-1.5" data-testid="thread-reply">
      <textarea
        ref={ref}
        rows={1}
        value={body}
        placeholder="Reply…"
        // Never disabled while in flight: a request in progress is no
        // reason to stop someone typing the next sentence.
        onChange={(e) => setBody(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.preventDefault();
            ref.current?.blur();
            return;
          }
          if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
            e.preventDefault();
            send();
          }
        }}
        data-testid={`thread-reply-input-${threadId}`}
        aria-label="Reply to this thread"
        className={cn(
          "w-full resize-none overflow-y-auto rounded-md border-0 bg-muted/40 px-2 py-1.5 leading-relaxed text-foreground outline-none placeholder:text-muted-foreground focus-visible:ring-[1.5px] focus-visible:ring-ring/60",
          tzBody,
        )}
      />
      <div className="flex items-center gap-1.5">
        <button
          type="button"
          className={cn(btnCardXs, "px-2.5")}
          data-testid={`thread-reply-send-${threadId}`}
          disabled={sending || !body.trim()}
          onClick={send}
        >
          {sending ? "Sending" : "Reply"}
        </button>
        <span className="flex-1" />
        <span className={cn("font-mono text-muted-foreground", tzEyebrow)}>⌘↵</span>
      </div>
      {error && (
        // The drift notice's language, at comment scale: a dot, the
        // reason in words, and the one action worth offering.
        <div
          role="status"
          data-testid={`thread-reply-error-${threadId}`}
          className="flex flex-wrap items-center gap-2 rounded-md bg-muted/30 px-2 py-1.5"
        >
          <span aria-hidden className="size-2 shrink-0 rounded-full bg-destructive" />
          <span className={cn("min-w-[8rem] flex-1 leading-snug text-foreground/80", tzMeta)}>
            Reply didn't send — {error} · your words are still here
          </span>
          <button
            type="button"
            className={btnEmberXs}
            data-testid={`thread-reply-retry-${threadId}`}
            onClick={send}
          >
            Retry
          </button>
        </div>
      )}
    </div>
  );
}

function ResolveButton({
  thread,
  resolved,
  onToggle,
}: {
  thread: PrReviewThread;
  resolved: boolean;
  onToggle: () => void;
}) {
  return (
    <button
      type="button"
      className={cn(btnCard, "h-6 rounded px-2", tzMeta)}
      data-testid={`thread-resolve-${thread.id}`}
      onClick={onToggle}
    >
      {resolved ? "Unresolve" : "Resolve"}
    </button>
  );
}

function ThreadComment({ comment }: { comment: PrThreadComment }) {
  return (
    <div className="group/comment space-y-0.5">
      <div className="flex items-center gap-1.5">
        <AuthorAvatar name={comment.author} />
        <span className={cn("font-medium text-foreground truncate", tzRowTitle)}>
          {comment.author}
        </span>
        {comment.created_at && (
          <span className={cn("text-muted-foreground", tzMeta)}>
            {formatDate(comment.created_at)}
          </span>
        )}
        <span className="flex-1" />
        <CopyButton text={comment.body} />
      </div>
      <p className="select-text pr-reading whitespace-pre-wrap break-words pl-7">
        {comment.body}
      </p>
    </div>
  );
}

interface ThreadProps {
  thread: PrReviewThread;
  resolved: boolean;
  onSendToAgent?: (task: ReviewThreadTask) => Promise<unknown>;
  onReply?: (thread: PrReviewThread, body: string) => Promise<unknown>;
  onSetResolved?: (thread: PrReviewThread, resolved: boolean) => void;
}

/**
 * One thread, open.
 *
 * Resolved threads render this too — behind a header you have to click,
 * because a settled conversation is a record and an open one is work.
 */
function ThreadBody({
  thread,
  resolved,
  onSendToAgent,
  onReply,
  onSetResolved,
}: ThreadProps) {
  const root = thread.comments[0];
  const last = thread.comments[thread.comments.length - 1] ?? root;

  return (
    <div className="space-y-2">
      {thread.comments.map((comment) => (
        <ThreadComment key={comment.id} comment={comment} />
      ))}

      <div className="flex flex-wrap items-center gap-1.5 pl-7">
        {/* An agent can only act on something still open; a resolved
            thread is a record, and handing one over would be work with
            nowhere to land. */}
        {onSendToAgent && !resolved && (
          <SendToAgentButton
            onSend={() =>
              onSendToAgent({
                kind: "review-thread",
                reviewer: last.author,
                body: last.body,
                path: thread.path,
                line: thread.line,
                verdict: null,
                // The thread's opening comment is the context the latest
                // one was written under, when they are not the same.
                parent:
                  last.id !== root.id ? { author: root.author, body: root.body } : null,
              })
            }
          />
        )}
        {onSetResolved && thread.is_resolvable && (
          <ResolveButton
            thread={thread}
            resolved={resolved}
            onToggle={() => onSetResolved(thread, !resolved)}
          />
        )}
      </div>

      {onReply && (
        <div className="pl-7">
          <ThreadReplyBox
            threadId={thread.id}
            onReply={(body) => onReply(thread, body)}
          />
        </div>
      )}
    </div>
  );
}

/** An open thread: anchor, state labels, conversation, actions. */
function UnresolvedThread(props: ThreadProps) {
  const { thread } = props;
  return (
    <div
      data-testid="review-thread"
      data-thread-id={thread.id}
      data-resolved="false"
      className="space-y-1.5 rounded-md px-1 py-1"
    >
      <div className="flex items-center gap-1.5">
        <AnchorPill path={thread.path} line={thread.line} />
        {thread.is_outdated && (
          // Muted, not hidden, and never a reason to drop the thread:
          // the lines moved, the objection did not.
          <span
            data-testid="thread-outdated"
            className={cn("text-muted-foreground", tzMeta)}
          >
            Outdated
          </span>
        )}
      </div>
      <ThreadBody {...props} />
    </div>
  );
}

/**
 * A settled thread, folded to one line.
 *
 * "Resolved · 3 comments" is the whole header on purpose: what a reader
 * needs from a closed conversation is that it is closed and how much of
 * it there is. The words themselves are one click away and nothing is
 * deleted.
 */
function ResolvedThread(props: ThreadProps) {
  const { thread } = props;
  const [open, setOpen] = useState(false);

  return (
    <div
      data-testid="review-thread"
      data-thread-id={thread.id}
      data-resolved="true"
      className="rounded-md px-1 py-0.5"
    >
      <button
        type="button"
        data-testid={`thread-resolved-header-${thread.id}`}
        className="flex w-full items-center gap-1.5 rounded-sm px-0.5 py-1 text-left transition-colors hover:bg-accent/30"
        onClick={() => setOpen((v) => !v)}
      >
        <ChevronRight
          className={cn(
            "h-3 w-3 shrink-0 text-muted-foreground transition-transform",
            open && "rotate-90",
          )}
        />
        <span className={cn("text-muted-foreground", tzMeta)}>
          Resolved · {thread.comments.length}{" "}
          {thread.comments.length === 1 ? "comment" : "comments"}
        </span>
        <AnchorPill path={thread.path} line={thread.line} />
        {thread.is_outdated && (
          <span
            data-testid="thread-outdated"
            className={cn("text-muted-foreground", tzMeta)}
          >
            Outdated
          </span>
        )}
      </button>
      {open && (
        <div className="pt-1">
          <ThreadBody {...props} />
        </div>
      )}
    </div>
  );
}

interface Props {
  reviews: ReviewComment[];
  inlineComments: InlineReviewComment[];
  /**
   * Real threads, with resolution state. Empty on a host that serves
   * none, in which case this surface is exactly what it was before them.
   */
  threads?: PrReviewThread[];
  isLoading?: boolean;
  /** Hands one comment to an agent. Absent ⇒ no handoff drawn. */
  onSendToAgent?: (task: ReviewThreadTask) => Promise<unknown>;
  /** Absent ⇒ the host has not declared replies; no composer is drawn. */
  onReply?: (thread: PrReviewThread, body: string) => Promise<unknown>;
  /** Absent ⇒ no Resolve button. Rejection rolls the flip back. */
  onSetResolved?: (thread: PrReviewThread, resolved: boolean) => Promise<unknown>;
}

interface GroupedReview {
  review: ReviewComment;
  inlineComments: InlineReviewComment[];
}

export function ReviewThreads({
  reviews,
  inlineComments,
  threads = [],
  isLoading = false,
  onSendToAgent,
  onReply,
  onSetResolved,
}: Props) {
  /**
   * Optimistic resolution, per thread.
   *
   * Resolving is one click with one visible consequence, and waiting a
   * round trip to see it makes the button feel broken. So the flip is
   * immediate and the entry is dropped once the host's own answer agrees
   * with it — which also means a thread someone else resolved in another
   * window still lands here on the next poll.
   */
  const [pendingResolve, setPendingResolve] = useState<Record<string, boolean>>({});

  useEffect(() => {
    setPendingResolve((prev) => {
      if (Object.keys(prev).length === 0) return prev;
      let changed = false;
      const next: Record<string, boolean> = {};
      for (const [id, value] of Object.entries(prev)) {
        const thread = threads.find((t) => t.id === id);
        // Gone, or agreed with: the override has done its job.
        if (!thread || thread.is_resolved === value) {
          changed = true;
          continue;
        }
        next[id] = value;
      }
      return changed ? next : prev;
    });
  }, [threads]);

  const resolvedOf = (thread: PrReviewThread) =>
    pendingResolve[thread.id] ?? thread.is_resolved;

  const toggleResolved = (thread: PrReviewThread, resolved: boolean) => {
    if (!onSetResolved) return;
    setPendingResolve((prev) => ({ ...prev, [thread.id]: resolved }));
    onSetResolved(thread, resolved).catch((err) => {
      // Rolled back to what the host actually says, and said out loud —
      // a flip that quietly undoes itself is worse than one that fails.
      setPendingResolve((prev) => {
        const next = { ...prev };
        delete next[thread.id];
        return next;
      });
      toast.error(String(err));
    });
  };

  /**
   * What the flat lists must no longer draw.
   *
   * A comment that lives in a thread is rendered by the thread, which
   * knows things the flat list does not (is it resolved, is it
   * outdated, what was replied). The join is the REST id the GraphQL
   * payload carries alongside its node id.
   *
   * Review *summaries* are a different resource and survive: "Left two
   * notes on the draft store" is the verdict, not a thread comment. The
   * body match is what keeps that true on GitLab, where a review summary
   * and a thread comment are the same note and share an id space.
   */
  const bodyByCommentId = useMemo(() => {
    const map = new Map<number, string>();
    for (const thread of threads) {
      for (const comment of thread.comments) {
        if (comment.database_id != null) map.set(comment.database_id, comment.body);
      }
    }
    return map;
  }, [threads]);

  const visibleInline = useMemo(
    () => inlineComments.filter((c) => !bodyByCommentId.has(c.id)),
    [inlineComments, bodyByCommentId],
  );

  const visibleReviews = useMemo(
    () => reviews.filter((r) => bodyByCommentId.get(r.id) !== r.body),
    [reviews, bodyByCommentId],
  );

  const grouped = useMemo(() => {
    // Group inline comments by pull_request_review_id
    const inlineByReview = new Map<number, InlineReviewComment[]>();
    const orphanInline: InlineReviewComment[] = [];

    for (const c of visibleInline) {
      if (c.in_reply_to_id) continue; // skip threaded replies, show only top-level
      if (c.pull_request_review_id) {
        const existing = inlineByReview.get(c.pull_request_review_id) ?? [];
        existing.push(c);
        inlineByReview.set(c.pull_request_review_id, existing);
      } else {
        orphanInline.push(c);
      }
    }

    const result: GroupedReview[] = visibleReviews.map((r) => ({
      review: r,
      inlineComments: inlineByReview.get(r.id) ?? [],
    }));

    // A comment whose review is not in the list has to land somewhere.
    // Before threads that could not happen; now it can — a review
    // summary may be deduped away (GitLab, where the summary and a
    // thread comment are one note) while a comment filed under it is
    // not. Dropping those would be the dedupe deleting evidence.
    const shown = new Set(visibleReviews.map((r) => r.id));
    for (const [reviewId, list] of inlineByReview) {
      if (!shown.has(reviewId)) orphanInline.push(...list);
    }

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
  }, [visibleReviews, visibleInline]);

  // Unresolved first, and expanded: the open questions are the reason
  // anyone opens this section.
  const unresolvedThreads = threads.filter((t) => !resolvedOf(t));
  const resolvedThreads = threads.filter((t) => resolvedOf(t));

  const legacyCount =
    visibleReviews.length + visibleInline.filter((c) => !c.in_reply_to_id).length;
  const totalCount = threads.length + legacyCount;

  const threadProps = (thread: PrReviewThread): ThreadProps => ({
    thread,
    resolved: resolvedOf(thread),
    onSendToAgent,
    onReply,
    onSetResolved: onSetResolved ? toggleResolved : undefined,
  });

  return (
    <CollapsibleSection
      label="Comments"
      count={totalCount}
      rightSlot={
        threads.length > 0 ? (
          <span
            data-testid="unresolved-count"
            className={cn("tabular-nums text-muted-foreground", tzMetaNum)}
          >
            {unresolvedThreads.length} unresolved
          </span>
        ) : undefined
      }
    >
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

        {unresolvedThreads.map((thread) => (
          <UnresolvedThread key={thread.id} {...threadProps(thread)} />
        ))}

        {/* Review verdicts sit between the two: they are context for the
            open threads above, and they outrank a settled conversation. */}
        {grouped.map((g) => (
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
                      "select-text pr-reading flex-1 whitespace-pre-wrap break-words",
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
                  <AnchorPill path={ic.path} line={ic.line} />
                  <CopyButton text={ic.body} />
                </div>
                <p className="select-text pr-reading whitespace-pre-wrap break-words">
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

        {resolvedThreads.map((thread) => (
          <ResolvedThread key={thread.id} {...threadProps(thread)} />
        ))}
      </div>
    </CollapsibleSection>
  );
}
