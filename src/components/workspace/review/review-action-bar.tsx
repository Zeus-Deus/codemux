import { useEffect, useRef, useState } from "react";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { cn } from "@/lib/utils";
import { toast } from "@/lib/toast";
import {
  btnCard,
  btnCardStrong,
  btnEmber,
  btnGreenMuted,
  btnGreenSolid,
  btnGreenTint,
  btnQuiet,
  reviewBodyRequirement,
  tzBody,
  tzEyebrow,
  tzMeta,
  tzMetaNum,
} from "./review-ui";
import { MERGE_STRATEGIES } from "./merge-sheet";
import {
  getLastVerdict,
  getReviewDraft,
  setLastVerdict,
  setReviewDraft,
  type DraftKey,
} from "./pr-drafts";

/**
 * Which bar the PR earns. Reviewer is the fallback: if we can't tell
 * whether you wrote this PR, offering a review is harmless and offering
 * a merge is not.
 */
export type ActionBarState =
  | "record"
  | "reviewer"
  | "author-draft"
  | "author-green"
  | "author-blocked";

export interface ActionBarProps {
  state: ActionBarState;
  draftKey: DraftKey;
  /** Green: "12 checks passed · 1 approval · no conflicts".
   *  Blocked: the reason, in words. */
  sentence: string;
  /** Blocked bars carry a conflict-specific extra action. */
  blockedByConflicts?: boolean;
  merging: boolean;
  submitting: boolean;
  mergeStrategy: string;
  /** Per-operation declarations. A control whose operation is undeclared
   *  is not drawn — never drawn-and-disabled, which would need a reason
   *  in words for something that can never work here (binding rule 5). */
  canRequestChanges: boolean;
  canApprove: boolean;
  canComment: boolean;
  canMerge: boolean;
  canChangeState: boolean;
  onSubmitReview: (event: string, body: string) => void;
  onOpenMergeSheet: () => void;
  onPickStrategy: (strategy: string) => void;
  onReadyForReview: () => void;
  onClose: () => void;
  onRebase?: () => void;
  /** Conflicts only: hands the merge to an agent in a worktree thread. */
  onResolveConflicts?: () => Promise<unknown>;
}

/**
 * One bar, bottom of the detail, four shapes.
 *
 * Binding rule 1 is enforced structurally: every state ends with a
 * button carrying `data-testid="review-primary-action"`, and every
 * button style in `review-ui` shares one geometry — same height, same
 * padding, same radius. A state change repaints the primary and rewrites
 * the sentence beside it; it never moves or resizes it. That is why
 * Merge stays visible while blocked instead of disappearing: its
 * disabled state is where the reason lives.
 */
export function ReviewActionBar(props: ActionBarProps) {
  const { state, sentence, merging } = props;

  return (
    <div
      className="flex flex-col gap-2 border-t border-border/40 bg-muted/30 px-3 py-2.5"
      data-testid="review-action-bar"
      data-bar-state={state}
    >
      {state === "reviewer" ? (
        <ReviewerBar {...props} />
      ) : state === "record" ? (
        <div className="flex items-center gap-2">
          <span className={cn("flex-1 text-muted-foreground", tzBody)}>{sentence}</span>
        </div>
      ) : state === "author-draft" ? (
        <div className="flex items-center gap-2">
          <span
            className={cn(
              "flex flex-1 items-center gap-1.5 text-muted-foreground",
              tzMetaNum,
            )}
          >
            <span
              aria-hidden
              className="size-2 shrink-0 rounded-full border-[1.5px] border-dashed border-muted-foreground"
            />
            Draft · reviewers aren't notified
          </span>
          {props.canChangeState && (
            <>
              <button type="button" className={btnCard} onClick={props.onClose}>
                Close
              </button>
              <button
                type="button"
                className={btnCardStrong}
                data-testid="review-primary-action"
                onClick={props.onReadyForReview}
              >
                Ready for review
              </button>
            </>
          )}
        </div>
      ) : (
        <div className="flex items-center gap-2">
          <span
            className={cn(
              "flex min-w-0 flex-1 items-center gap-1.5 text-foreground/80",
              tzMetaNum,
            )}
          >
            <span
              aria-hidden
              className={cn(
                "size-2 shrink-0 rounded-full",
                state === "author-green" ? "bg-status-open" : "bg-status-working",
              )}
            />
            <span className="truncate" data-testid="bar-sentence">
              {sentence}
            </span>
          </span>
          {props.blockedByConflicts && props.onResolveConflicts && (
            <ResolveWithAgentButton onResolve={props.onResolveConflicts} />
          )}
          {props.blockedByConflicts && props.onRebase && (
            <button type="button" className={btnCard} onClick={props.onRebase}>
              Rebase
            </button>
          )}
          {/* No strategy while the branch conflicts: picking how to
              merge is a decision about a merge that can't run, and the
              room it takes is room the blocking reason needs to stay in
              words rather than an ellipsis (binding rule 5). It returns
              the moment the conflict does — Merge itself never moves. */}
          {!props.blockedByConflicts && props.canMerge && (
            <StrategyPicker
              value={props.mergeStrategy}
              onPick={props.onPickStrategy}
              disabled={merging}
            />
          )}
          {props.canMerge && (
          <button
            type="button"
            data-testid="review-primary-action"
            className={state === "author-green" ? btnGreenSolid : btnGreenMuted}
            onClick={props.onOpenMergeSheet}
            // Blocked Merge is still a real control: it opens the sheet,
            // where the host has the final say. What it never does is
            // pretend the block isn't there — that's the sentence's job.
            aria-describedby="review-bar-sentence"
          >
            {merging ? (
              <>
                <span
                  aria-hidden
                  className="size-2.5 animate-spin rounded-full border-[1.6px] border-current border-r-transparent"
                />
                Merging
              </>
            ) : (
              "Merge"
            )}
          </button>
          )}
        </div>
      )}
    </div>
  );
}

/**
 * Ember, and sitting to the left of Rebase and Merge.
 *
 * It never takes the primary slot: resolving conflicts with an agent
 * starts work, it doesn't complete the merge, and the button that
 * completes the merge must stay where it was (binding rule 1).
 */
function ResolveWithAgentButton({ onResolve }: { onResolve: () => Promise<unknown> }) {
  const [starting, setStarting] = useState(false);
  return (
    <button
      type="button"
      className={btnEmber}
      data-testid="resolve-conflicts-with-agent"
      disabled={starting}
      onClick={() => {
        if (starting) return;
        setStarting(true);
        onResolve()
          .catch((err) => toast.error(String(err)))
          .finally(() => setStarting(false));
      }}
    >
      {starting ? (
        <>
          <span
            aria-hidden
            className="size-1.5 animate-spin rounded-full border-[1.5px] border-current border-r-transparent"
          />
          Starting agent
        </>
      ) : (
        <>
          <span aria-hidden className="size-1.5 rounded-full bg-current" />
          Resolve with agent
        </>
      )}
    </button>
  );
}

function StrategyPicker({
  value,
  onPick,
  disabled,
}: {
  value: string;
  onPick: (id: string) => void;
  disabled: boolean;
}) {
  const label = MERGE_STRATEGIES.find((s) => s.id === value)?.label ?? "Squash";
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild disabled={disabled}>
        <button type="button" className={btnQuiet} data-testid="merge-strategy-picker">
          {label} <span className={tzEyebrow}>▾</span>
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="min-w-28">
        {MERGE_STRATEGIES.map((strategy) => (
          <DropdownMenuItem key={strategy.id} onSelect={() => onPick(strategy.id)}>
            {strategy.label}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/**
 * The reviewer bar: a field that becomes a composer, and three verdicts.
 *
 * The text lives in the module-level draft map, not in this component's
 * state — a 2.5s poll, a tab switch or a failed submit must all leave it
 * exactly where it was (binding rule 4).
 */
function ReviewerBar({
  draftKey,
  submitting,
  canRequestChanges,
  canApprove,
  canComment,
  onSubmitReview,
}: ActionBarProps) {
  const [text, setText] = useState(() => getReviewDraft(draftKey));
  const [expanded, setExpanded] = useState(() => getReviewDraft(draftKey).length > 0);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  // Switching PR swaps in that PR's own draft rather than carrying this
  // one's across.
  const lastKey = useRef(draftKey);
  useEffect(() => {
    if (lastKey.current === draftKey) return;
    lastKey.current = draftKey;
    const next = getReviewDraft(draftKey);
    setText(next);
    setExpanded(next.length > 0);
  }, [draftKey]);

  const update = (value: string) => {
    setText(value);
    setReviewDraft(draftKey, value);
  };

  // Comment and Request changes need words; Approve does not. Asked here
  // so the control is unavailable with the reason on it, rather than
  // available, clicked, and refused by the host.
  const needsWords = (event: string) => reviewBodyRequirement(event, text);

  const submit = (event: string) => {
    if (needsWords(event)) {
      setExpanded(true);
      textareaRef.current?.focus();
      return;
    }
    setLastVerdict(draftKey, event);
    onSubmitReview(event, text);
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
      e.preventDefault();
      submit(getLastVerdict(draftKey));
    }
  };

  if (!canComment && !canApprove && !canRequestChanges) return null;

  return (
    <div className="flex flex-col gap-2">
      {expanded ? (
        <textarea
          ref={textareaRef}
          autoFocus
          value={text}
          rows={3}
          placeholder="Leave a review…"
          onChange={(e) => update(e.target.value)}
          onKeyDown={onKeyDown}
          data-testid="review-composer"
          className={cn(
            "w-full resize-y rounded-md border-0 bg-background px-2.5 py-2 leading-relaxed text-foreground outline-none placeholder:text-muted-foreground focus-visible:ring-[1.5px] focus-visible:ring-ring/60",
            tzBody,
          )}
        />
      ) : (
        <button
          type="button"
          onClick={() => setExpanded(true)}
          data-testid="review-composer-collapsed"
          className={cn(
            "rounded-md bg-background px-2.5 py-2 text-left text-muted-foreground",
            tzBody,
          )}
        >
          Leave a review… <span className={cn("font-mono", tzMeta)}>⌘↵</span>
        </button>
      )}
      <div className="flex items-center gap-2">
        <span className="flex-1" />
        {canApprove && (
          <button
            type="button"
            className={btnGreenTint}
            data-testid="verdict-approve"
            onClick={() => submit("approve")}
            disabled={submitting}
          >
            {submitting ? "Sending" : "Approve"}
          </button>
        )}
        {/* GitLab has no request-changes verdict, so its adapter does not
            declare the operation and this control is not drawn there at
            all — rather than drawn and refused. */}
        {canRequestChanges && (
          <button
            type="button"
            className={btnCard}
            data-testid="verdict-request-changes"
            onClick={() => submit("request-changes")}
            disabled={submitting || !!needsWords("request-changes")}
            title={needsWords("request-changes") ?? undefined}
          >
            Request changes
          </button>
        )}
        {canComment && (
          <button
            type="button"
            className={btnCard}
            data-testid="review-primary-action"
            onClick={() => submit("comment")}
            disabled={submitting || !!needsWords("comment")}
            title={needsWords("comment") ?? undefined}
          >
            Comment
          </button>
        )}
      </div>
    </div>
  );
}
