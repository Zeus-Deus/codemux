import { useCallback, useMemo, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { toast } from "@/lib/toast";
import {
  checkoutDefaultBranchInWorkspace,
  closePullRequest,
  gitPullChanges,
  gitStashPush,
  mergePullRequest,
  setPrReady,
  submitPrReview,
} from "@/tauri/commands";
import type {
  CheckInfo,
  InlineReviewComment,
  PullRequestInfo,
  ReviewComment,
} from "@/tauri/types";
import type { ProviderPresentation } from "@/lib/source-control";
import { ReviewHeader } from "./review-header";
import { ReviewTabStrip, type ReviewTab } from "./review-tab-strip";
import { ReviewChecks } from "./review-checks";
import { ReviewReviewers } from "./review-reviewers";
import { ReviewDescription } from "./review-description";
import { ReviewThreads } from "./review-threads";
import { ReviewActionBar, type ActionBarState } from "./review-action-bar";
import { MergeSheet, type MergeRequestPayload } from "./merge-sheet";
import {
  ReviewDriftNotice,
  mostSevere,
  type DriftNotice,
} from "./review-drift-notice";
import {
  checkState,
  plural,
  relativeAge,
  shortAge,
} from "./review-ui";
import {
  clearReviewDraft,
  draftKey as makeDraftKey,
  getLastVerdict,
  getMergeStrategy,
  getReviewDraft,
  setMergeStrategy,
} from "./pr-drafts";

/**
 * Head-SHA history, so a force-push can be noticed at all.
 *
 * Module-level rather than component state because the panel unmounts
 * whenever the pane deck switches tabs, and a force-push that happened
 * while you were reading the Files pane is exactly the one you need to
 * be told about when you come back.
 */
const headOidSeen = new Map<string, { oid: string; changedAt: number | null }>();

/** Reset force-push tracking — for tests only. */
export function _resetHeadOidTracking(): void {
  headOidSeen.clear();
}

function noteHeadOid(key: string, oid: string | null, merged: boolean): number | null {
  if (!oid) return null;
  const prev = headOidSeen.get(key);
  if (!prev) {
    headOidSeen.set(key, { oid, changedAt: null });
    return null;
  }
  if (prev.oid === oid) return prev.changedAt;
  // A merge rewrites the head too; that is not a force-push and has its
  // own, louder notice.
  const changedAt = merged ? null : Date.now();
  headOidSeen.set(key, { oid, changedAt });
  return changedAt;
}

export interface ReviewDetailProps {
  pr: PullRequestInfo;
  checks: CheckInfo[];
  reviews: ReviewComment[];
  inlineComments: InlineReviewComment[];
  checksLoading: boolean;
  commentsLoading: boolean;
  cwd: string;
  workspaceId: string;
  provider: ProviderPresentation;
  repoSlug: string | null;
  /** Authenticated account, for author-vs-reviewer. Null ⇒ reviewer. */
  viewerLogin: string | null;
  checkedOutHere: boolean;
  /** Commits on the remote this worktree doesn't have. */
  gitBehind: number;
  /** Uncommitted files here — decides whether Pull may be offered. */
  gitDirtyFiles: number;
  /** Age of the newest good data when polls are failing; null when the
   *  polls are healthy. Content is never blanked either way. */
  staleAgeMs: number | null;
  onRefresh: () => void;
  onOpenChanges: () => void;
}

/**
 * The PR detail surface.
 *
 * This is the whole component the Pull Requests page will render in its
 * detail column at a wider measure — it takes no panel-specific props
 * and lays itself out in a column, so widening it is a container
 * decision rather than a fork.
 */
export function ReviewDetail(props: ReviewDetailProps) {
  const {
    pr,
    checks,
    reviews,
    inlineComments,
    checksLoading,
    commentsLoading,
    cwd,
    workspaceId,
    provider,
    repoSlug,
    viewerLogin,
    checkedOutHere,
    gitBehind,
    gitDirtyFiles,
    staleAgeMs,
    onRefresh,
    onOpenChanges,
  } = props;

  const key = makeDraftKey(workspaceId, pr.number);
  const [activeTab, setActiveTab] = useState("summary");
  const [mergeSheetOpen, setMergeSheetOpen] = useState(false);
  const [merging, setMerging] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);
  const [strategy, setStrategy] = useState(() => getMergeStrategy());

  const merged = pr.state.toUpperCase() === "MERGED";
  const closed = pr.state.toUpperCase() === "CLOSED";
  const readOnly = merged || closed;

  const isAuthor =
    !!viewerLogin &&
    !!pr.author &&
    viewerLogin.toLowerCase() === pr.author.toLowerCase();

  const forcePushedAt = noteHeadOid(key, pr.head_ref_oid, merged);

  // ── Check arithmetic, shared by the bar sentence and the rail ──
  const { passedCount, failedCount, runningCount } = useMemo(() => {
    let passed = 0;
    let failed = 0;
    let running = 0;
    for (const check of checks) {
      const state = checkState(check.conclusion, check.status);
      if (state === "pass") passed++;
      else if (state === "fail") failed++;
      else if (state === "running") running++;
    }
    return { passedCount: passed, failedCount: failed, runningCount: running };
  }, [checks]);

  const approvals = pr.latest_reviews.filter((r) => r.state === "APPROVED").length;
  const baseBranch = pr.base_branch ?? "main";
  const conflicted =
    pr.mergeable === "CONFLICTING" || pr.merge_state_status === "DIRTY";

  /** The blocking condition, in words. Null when nothing blocks. */
  const blockedReason = useMemo((): string | null => {
    if (conflicted) return `Conflicts with ${baseBranch}`;
    if (failedCount > 0) return `${plural(failedCount, "check")} failed`;
    if (runningCount > 0) return `${plural(runningCount, "check")} still running`;
    if (pr.review_decision === "CHANGES_REQUESTED") return "Changes requested";
    if (pr.review_decision === "REVIEW_REQUIRED" || pr.merge_state_status === "BLOCKED")
      return "Needs one approval";
    return null;
  }, [
    conflicted,
    baseBranch,
    failedCount,
    runningCount,
    pr.review_decision,
    pr.merge_state_status,
  ]);

  const barState: ActionBarState = readOnly
    ? "record"
    : !isAuthor
      ? "reviewer"
      : pr.is_draft
        ? "author-draft"
        : blockedReason
          ? "author-blocked"
          : "author-green";

  const greenSentence = [
    passedCount > 0 ? `${plural(passedCount, "check")} passed` : null,
    approvals > 0 ? plural(approvals, "approval") : null,
    "no conflicts",
  ]
    .filter(Boolean)
    .join(" · ");

  const barSentence = readOnly
    ? merged
      ? `Merged${pr.merged_by ? ` by ${pr.merged_by}` : ""} — this ${provider.noun} is a record now.`
      : `Closed without merging.`
    : blockedReason ?? greenSentence;

  // ── Mutations ──

  const runSubmit = useCallback(
    async (event: string, body: string) => {
      setSubmitting(true);
      setSubmitError(null);
      try {
        await submitPrReview(cwd, pr.number, event, body);
        // Only a successful send clears the text.
        clearReviewDraft(key);
        onRefresh();
        toast.success(
          event === "approve"
            ? "Approved"
            : event === "request-changes"
              ? "Changes requested"
              : "Comment posted",
        );
      } catch (err) {
        // The text stays exactly where it was; the notice offers Retry.
        setSubmitError(String(err));
      } finally {
        setSubmitting(false);
      }
    },
    [cwd, pr.number, key, onRefresh],
  );

  const confirmMerge = useCallback(
    async (payload: MergeRequestPayload) => {
      setMerging(true);
      try {
        await mergePullRequest(
          cwd,
          pr.number,
          payload.method,
          payload.deleteBranch,
          payload.commitTitle,
          payload.commitBody,
        );
        setMergeSheetOpen(false);
        onRefresh();
        toast.success(`Merged #${pr.number}`);
      } catch (err) {
        toast.error(String(err));
      } finally {
        setMerging(false);
      }
    },
    [cwd, pr.number, onRefresh],
  );

  const markReady = useCallback(async () => {
    try {
      await setPrReady(cwd, pr.number, true);
      onRefresh();
      toast.success("Marked ready for review");
    } catch (err) {
      toast.error(String(err));
    }
  }, [cwd, pr.number, onRefresh]);

  const closePr = useCallback(async () => {
    try {
      await closePullRequest(cwd, pr.number);
      onRefresh();
      toast.success(`Closed #${pr.number}`);
    } catch (err) {
      toast.error(String(err));
    }
  }, [cwd, pr.number, onRefresh]);

  const switchToDefaultBranch = useCallback(async () => {
    try {
      const branch = await checkoutDefaultBranchInWorkspace(workspaceId);
      toast.success(`Switched to ${branch}`);
    } catch (err) {
      toast.error(String(err));
    }
  }, [workspaceId]);

  const pull = useCallback(async () => {
    try {
      await gitPullChanges(cwd);
      onRefresh();
      toast.success("Pulled");
    } catch (err) {
      toast.error(String(err));
    }
  }, [cwd, onRefresh]);

  const stashAndPull = useCallback(async () => {
    try {
      await gitStashPush(cwd, false);
      await gitPullChanges(cwd);
      onRefresh();
      toast.success("Stashed and pulled");
    } catch (err) {
      toast.error(String(err));
    }
  }, [cwd, onRefresh]);

  const copyDraft = useCallback(
    (label: string) => {
      const text = getReviewDraft(key);
      if (!text) {
        toast.info("Nothing typed to copy");
        return;
      }
      navigator.clipboard
        .writeText(text)
        .then(() => toast.success(label))
        .catch(() => toast.error("Couldn't copy"));
    },
    [key],
  );

  const openPrPath = useCallback(
    (suffix: string) => {
      openUrl(`${pr.url}${suffix}`).catch((err) => toast.error(String(err)));
    },
    [pr.url],
  );

  // ── Drift ──

  const notice = useMemo((): DriftNotice | null => {
    const pending = getReviewDraft(key);
    const candidates: DriftNotice[] = [];

    if (merged) {
      candidates.push({
        kind: "merged",
        tone: "violet",
        message: (
          <>
            Merged{pr.merged_by ? ` by ${pr.merged_by}` : ""}
            {relativeAge(pr.merged_at) ? ` ${relativeAge(pr.merged_at)}` : ""}
            {pending ? " · your pending notes were kept" : ""}
          </>
        ),
        actions: [
          ...(pending
            ? [{ label: "Copy notes", onClick: () => copyDraft("Notes copied") }]
            : []),
          {
            label: `Switch to ${baseBranch}`,
            onClick: () => void switchToDefaultBranch(),
            emphasis: "strong" as const,
          },
        ],
      });
    }

    if (closed) {
      candidates.push({
        kind: "closed",
        tone: "warn",
        message: (
          <>
            Closed without merging
            {relativeAge(pr.updated_at) ? ` ${relativeAge(pr.updated_at)}` : ""}
            {pending ? " · your pending notes were kept" : ""}
          </>
        ),
        actions: [
          ...(pending
            ? [{ label: "Copy notes", onClick: () => copyDraft("Notes copied") }]
            : []),
          {
            label: `Switch to ${baseBranch}`,
            onClick: () => void switchToDefaultBranch(),
            emphasis: "strong" as const,
          },
        ],
      });
    }

    if (forcePushedAt) {
      candidates.push({
        kind: "force-pushed",
        tone: "warn",
        // Re-anchoring notes onto the new diff is ship 4's problem; for
        // now this is informational and offers a fresh read.
        message: <>Force-pushed {relativeAge(new Date(forcePushedAt).toISOString())}</>,
        actions: [{ label: "Refresh", onClick: onRefresh, emphasis: "strong" }],
      });
    }

    // Deliberately no conflict notice. `conflict` sits above the
    // transient notices in the severity order, so raising one here would
    // outrank — and hide — the submit-failure notice for a review the
    // user just tried to send. The author's bar already names the
    // conflict in words, and a reviewer can't resolve someone else's
    // conflict from this panel anyway, so the notice would cost the one
    // message that is actionable to buy one that isn't.

    if (gitBehind > 0 && !readOnly) {
      if (gitDirtyFiles > 0) {
        candidates.push({
          kind: "remote-ahead-dirty",
          tone: "warn",
          message: (
            <>
              {plural(gitBehind, "commit")} behind, and {plural(gitDirtyFiles, "file")}{" "}
              modified here
            </>
          ),
          // Pull is never offered blind over uncommitted work.
          actions: [
            { label: "Review changes", onClick: onOpenChanges },
            { label: "Stash and pull", onClick: () => void stashAndPull() },
            { label: "Commit first", onClick: onOpenChanges, emphasis: "strong" },
          ],
        });
      } else {
        candidates.push({
          kind: "remote-ahead-clean",
          tone: "sky",
          message: (
            <>
              {plural(gitBehind, "commit")} on the remote you don't have · your worktree is
              clean
            </>
          ),
          actions: [
            { label: "What changed", onClick: () => openPrPath("/commits") },
            { label: "Pull", onClick: () => void pull(), emphasis: "strong" },
          ],
        });
      }
    }

    if (submitError) {
      candidates.push({
        kind: "submit-failed",
        tone: "fail",
        message: (
          <>
            Review didn't send — {submitError}
            {pending ? " · your notes are still here" : ""}
          </>
        ),
        actions: [
          { label: "Copy as markdown", onClick: () => copyDraft("Copied as markdown") },
          {
            label: "Retry",
            onClick: () => void runSubmit(getLastVerdict(key), getReviewDraft(key)),
            emphasis: "primary",
          },
        ],
      });
    }

    if (staleAgeMs != null) {
      candidates.push({
        kind: "stale-data",
        tone: "muted",
        // Stale and labelled beats blank: the content above stays put.
        message: <>Showing data from {shortAge(staleAgeMs)} ago</>,
        actions: [{ label: "Retry now", onClick: onRefresh }],
      });
    }

    return mostSevere(candidates);
  }, [
    key,
    merged,
    closed,
    readOnly,
    isAuthor,
    conflicted,
    forcePushedAt,
    gitBehind,
    gitDirtyFiles,
    submitError,
    staleAgeMs,
    pr.merged_by,
    pr.merged_at,
    pr.updated_at,
    provider.noun,
    baseBranch,
    copyDraft,
    switchToDefaultBranch,
    onRefresh,
    onOpenChanges,
    openPrPath,
    pull,
    stashAndPull,
    runSubmit,
  ]);

  // Only Summary has content in this ship. The strip takes a list so
  // Code and Timeline arrive without the layout below it moving.
  const tabs: ReviewTab[] = [{ id: "summary", label: "Summary" }];

  return (
    <div className="flex min-h-full flex-col" data-testid="review-detail">
      <ReviewHeader
        pr={pr}
        provider={provider}
        repoSlug={repoSlug}
        checkedOutHere={checkedOutHere}
        onRefresh={onRefresh}
      />

      <ReviewTabStrip tabs={tabs} activeId={activeTab} onSelect={setActiveTab} />

      {/* Status before description, deliberately: what you need to know
          about this PR right now is whether it's healthy, not what its
          author wrote about it a day ago. */}
      <div className="flex flex-1 flex-col gap-3 px-3.5 py-3">
        <ReviewChecks
          checks={checks}
          isLoading={checksLoading}
          cwd={cwd}
          prNumber={pr.number}
        />

        {!readOnly && (
          <ReviewReviewers
            pr={pr}
            cwd={cwd}
            canRequestReview={provider.supported}
            onRequested={onRefresh}
          />
        )}

        <ReviewDescription
          body={pr.body}
          cwd={cwd}
          prNumber={pr.number}
          draftKey={key}
          canEdit={provider.supported && isAuthor}
          readOnly={readOnly}
          onSaved={onRefresh}
        />

        <ReviewThreads
          reviews={reviews}
          inlineComments={inlineComments}
          isLoading={commentsLoading}
        />
      </div>

      {/* One notice slot, directly above the bar. */}
      {notice && <ReviewDriftNotice notice={notice} />}

      <ReviewActionBar
        state={barState}
        draftKey={key}
        sentence={barSentence}
        blockedByConflicts={conflicted}
        merging={merging}
        submitting={submitting}
        mergeStrategy={strategy}
        canRequestChanges={provider.kind !== "gitlab"}
        onSubmitReview={(event, body) => void runSubmit(event, body)}
        onOpenMergeSheet={() => setMergeSheetOpen(true)}
        onPickStrategy={(next) => {
          setStrategy(next);
          setMergeStrategy(next);
        }}
        onReadyForReview={() => void markReady()}
        onClose={() => void closePr()}
        onRebase={() => openPrPath("/conflicts")}
      />

      {mergeSheetOpen && (
        <MergeSheet
          open
          prNumber={pr.number}
          prTitle={pr.title}
          headBranch={pr.head_branch}
          merging={merging}
          onCancel={() => setMergeSheetOpen(false)}
          onConfirm={(payload) => void confirmMerge(payload)}
        />
      )}
    </div>
  );
}
