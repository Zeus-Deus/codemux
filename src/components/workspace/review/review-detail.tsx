import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { openUrl } from "@tauri-apps/plugin-opener";
import { toast } from "@/lib/toast";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  checkoutDefaultBranchInWorkspace,
  closePullRequest,
  getPrReviewDiff,
  getPrTimeline,
  gitPullChanges,
  gitStashPush,
  mergePullRequest,
  setPrReady,
  submitPrReview,
  submitPrReviewWithComments,
} from "@/tauri/commands";
import type {
  CheckInfo,
  InlineReviewComment,
  ProviderOperations,
  PullRequestInfo,
  ReviewComment,
} from "@/tauri/types";
import {
  prRefKey,
  selectRunsForPr,
  usePrAgentRunsStore,
} from "@/stores/pr-agent-runs-store";
import { buildTimeline, type TimelineFilter } from "@/lib/pr-timeline";
import { providerRef, type ProviderPresentation } from "@/lib/source-control";
import {
  findWorkspaceForBranch,
  handOffToAgent,
  type HandoffRequest,
  type ReviewThreadTask,
} from "@/lib/pr-agent-handoff";
import { checkOutPr } from "@/lib/pr-checkout";
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
import { ReviewCodeTab, type CodeTabIntent } from "./review-code-tab";
import { ReviewTimeline, TimelineFilterPicker } from "./review-timeline";
import { UnsupportedHostState } from "./review-empty-states";
import { ReviewDraftFooter } from "./review-draft-footer";
import { ReviewSubmitSheet } from "./review-submit-sheet";
import {
  clearLineDrafts,
  clearReviewDraft,
  draftKey as makeDraftKey,
  getLastVerdict,
  getMergeStrategy,
  getReviewDraft,
  putDiffSnapshot,
  reanchorLineDrafts,
  setMergeStrategy,
  submitBlockedReason,
  useLineDrafts,
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
  /**
   * The workspace this detail is being read *in*, when there is one.
   *
   * Null on the Pull Requests page: a pull request there belongs to a
   * project, not to a workspace, and everything workspace-shaped below
   * (draft scope, "switch to main", the handoff's you-are-already-here
   * route) reads that null as "no workspace to act in" rather than
   * pretending one exists.
   */
  workspaceId: string | null;
  /** Repository root — where an agent handoff would cut a worktree. */
  projectRoot: string;
  provider: ProviderPresentation;
  /**
   * What may be *done* on this checkout, declared by the backend adapter.
   *
   * Every write control below renders from this rather than from
   * `provider.kind`: a control whose operation is undeclared is never
   * drawn, so there is no disabled button anyone has to explain and no
   * request that can reach a host which would refuse it.
   */
  operations: ProviderOperations;
  repoSlug: string | null;
  /** Authenticated account, for author-vs-reviewer. Null ⇒ reviewer. */
  viewerLogin: string | null;
  checkedOutHere: boolean;
  /**
   * A *different* workspace standing on this PR's head branch, if any.
   * Resolved by the parent, which already subscribes to the workspace
   * list, so this component never scans it per render.
   */
  existingWorkspaceId?: string | null;
  /** Offer the branch controls in the header. The panel leaves this
   *  off: it is already standing in the branch. */
  showCheckout?: boolean;
  /** Commits on the remote this worktree doesn't have. */
  gitBehind: number;
  /** Uncommitted files here — decides whether Pull may be offered. */
  gitDirtyFiles: number;
  /** Age of the newest good data when polls are failing; null when the
   *  polls are healthy. Content is never blanked either way. */
  staleAgeMs: number | null;
  onRefresh: () => void;
  /** Absent on surfaces with no Changes pane to open. */
  onOpenChanges?: () => void;
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
    projectRoot,
    provider,
    operations,
    repoSlug,
    viewerLogin,
    checkedOutHere,
    existingWorkspaceId,
    showCheckout = false,
    gitBehind,
    gitDirtyFiles,
    staleAgeMs,
    onRefresh,
    onOpenChanges,
  } = props;

  // Drafts are scoped to the surface writing them. Without a workspace
  // the project root is that scope, which keeps the page's half-written
  // review separate from the panel's on the same PR — two places to
  // type, two drafts, neither overwriting the other (binding rule 4).
  const key = makeDraftKey(workspaceId ?? `page:${projectRoot}`, pr.number);
  const [activeTab, setActiveTab] = useState("summary");
  const [mergeSheetOpen, setMergeSheetOpen] = useState(false);
  const [merging, setMerging] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);
  const [strategy, setStrategy] = useState(() => getMergeStrategy());
  const [submitSheetOpen, setSubmitSheetOpen] = useState(false);
  const [codeIntent, setCodeIntent] = useState<CodeTabIntent | null>(null);
  const lineDrafts = useLineDrafts(key);

  const merged = pr.state.toUpperCase() === "MERGED";
  const closed = pr.state.toUpperCase() === "CLOSED";
  const readOnly = merged || closed;

  const isAuthor =
    !!viewerLogin &&
    !!pr.author &&
    viewerLogin.toLowerCase() === pr.author.toLowerCase();

  const forcePushedAt = noteHeadOid(key, pr.head_ref_oid, merged);

  // ── The diff ──
  //
  // Fetched for the Code tab, and also whenever notes are pending even
  // if you're reading Summary: a force-push has to be able to tell you
  // how many of your notes stopped matching without you going looking.
  // Keyed on the head sha, so a rewrite refetches by itself.
  const needsDiff = activeTab === "code" || lineDrafts.length > 0;
  const diffQuery = useQuery({
    queryKey: ["pr", "review-diff", cwd, pr.number, pr.head_ref_oid],
    queryFn: () => getPrReviewDiff(cwd, pr.number),
    enabled: needsDiff,
    staleTime: 60_000,
  });
  const diffText = diffQuery.data ?? "";

  useEffect(() => {
    if (!diffText || !pr.head_ref_oid) return;
    // Snapshot first: once the notes are re-anchored, the diff they were
    // written against is the only record of what the author saw, and a
    // force-pushed commit is not guaranteed to be fetchable later.
    putDiffSnapshot(key, pr.head_ref_oid, diffText);
    reanchorLineDrafts(key, diffText, pr.head_ref_oid);
  }, [diffText, pr.head_ref_oid, key]);

  const unanchoredCount = lineDrafts.filter((d) => d.status === "unanchored").length;
  const blockedSubmitReason = submitBlockedReason(lineDrafts, pr.head_ref_oid);

  // ── The timeline ──
  //
  // 30s, like the other conversation queries: history is not something
  // you sit and watch change. Fetched only while the tab is open — the
  // rail is the only thing that reads it, and a poll for a tab nobody is
  // looking at is a request per PR per half-minute for nothing.
  const timelineEnabled = activeTab === "timeline" && operations.timeline;
  const timelineQuery = useQuery({
    queryKey: ["pr", "timeline", cwd, pr.number] as const,
    queryFn: () => getPrTimeline(cwd, pr.number),
    enabled: timelineEnabled,
    staleTime: 30_000,
    refetchInterval: timelineEnabled ? 30_000 : false,
  });

  // Local agent runs for this PR. Subscribed to the store rather than
  // read once, so a handoff started from the Summary tab shows up on the
  // rail without a refetch — there is nothing to fetch, it is local.
  const allRuns = usePrAgentRunsStore((s) => s.runs);
  const prRef = prRefKey(repoSlug, pr.number);
  const agentRuns = useMemo(
    () => selectRunsForPr(allRuns, { prRef, projectRoot, prNumber: pr.number }),
    [allRuns, prRef, projectRoot, pr.number],
  );

  const [timelineFilter, setTimelineFilter] = useState<TimelineFilter>("everything");

  const timelineEntries = useMemo(
    () =>
      buildTimeline({
        pr,
        events: timelineQuery.data ?? [],
        runs: agentRuns,
        // The live checks query, not the timeline payload — the last row
        // is a fact about now.
        checks,
        filter: timelineFilter,
      }),
    [pr, timelineQuery.data, agentRuns, checks, timelineFilter],
  );

  const timelineStaleAgeMs =
    timelineQuery.isError && timelineQuery.dataUpdatedAt > 0
      ? Date.now() - timelineQuery.dataUpdatedAt
      : null;

  const openCodeTab = useCallback((kind: CodeTabIntent["kind"]) => {
    setActiveTab("code");
    setCodeIntent({ kind, nonce: Date.now() });
  }, []);

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

  /** What the last submit tried to send, so Retry sends the same thing. */
  const lastSubmit = useRef<{ verdict: string; body: string } | null>(null);

  /**
   * The pending review, sent whole.
   *
   * One request carrying the verdict, the overall body and every line
   * note — GitHub creates all of them or none of them, so there is no
   * state where the author sees half a review. The anchors were checked
   * against the current diff before this ran; if one still fails, the
   * drafts survive and the notice offers Retry.
   */
  const submitWithNotes = useCallback(
    async (verdict: string, body: string) => {
      lastSubmit.current = { verdict, body };
      setSubmitting(true);
      setSubmitError(null);
      try {
        await submitPrReviewWithComments(
          cwd,
          pr.number,
          verdict,
          body,
          lineDrafts.map((d) => ({
            file: d.path,
            body: d.body,
            side: d.side,
            line: d.line,
            start_line: d.startLine,
          })),
          pr.head_ref_oid ?? "",
        );
        clearLineDrafts(key);
        clearReviewDraft(key);
        setSubmitSheetOpen(false);
        onRefresh();
        toast.success(
          verdict === "approve"
            ? "Approved"
            : verdict === "request-changes"
              ? "Changes requested"
              : "Review sent",
        );
      } catch (err) {
        // Nothing was created host-side, and nothing is dropped here.
        setSubmitSheetOpen(false);
        setSubmitError(String(err));
      } finally {
        setSubmitting(false);
      }
    },
    [cwd, pr.number, pr.head_ref_oid, lineDrafts, key, onRefresh],
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
    if (!workspaceId) return;
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

  // ── Agent handoffs ──
  //
  // A failing check, a review comment and a conflict are three ways of
  // saying "someone has to go and do something on this branch". All
  // three go through one helper, which decides whether that means a
  // thread here, a thread in the workspace that already has the branch,
  // or a fresh worktree.

  /** Everything but the task itself — the same for all three buttons. */
  const handoffBase = useMemo(
    (): Omit<HandoffRequest, "task"> => ({
      pr: {
        number: pr.number,
        title: pr.title,
        url: pr.url,
        head_branch: pr.head_branch,
        base_branch: pr.base_branch,
      },
      prRef: `${repoSlug ?? ""}${providerRef(provider, pr.number)}`,
      projectRoot,
      cwd,
      // Standing in the branch is the panel's normal case, and it is
      // the one that needs no worktree at all.
      currentWorkspaceId: checkedOutHere ? workspaceId : null,
      cli: provider.cli,
      providerKind: provider.kind,
    }),
    [
      pr.number,
      pr.title,
      pr.url,
      pr.head_branch,
      pr.base_branch,
      repoSlug,
      provider,
      projectRoot,
      cwd,
      checkedOutHere,
      workspaceId,
    ],
  );

  // A merged or closed PR is a record. Sending an agent to fix its
  // checks would be work with nowhere to land.
  const canHandOff = !readOnly && operations.list_read;

  /**
   * What the button will do *here*, said in the panel's own terms.
   *
   * The canvas caption ("checks out the branch into a worktree, opens a
   * thread") is only true from the pull-requests page. In this panel you
   * are usually already standing in the branch, and promising a worktree
   * that won't be created is the kind of small lie that costs trust in
   * everything else the surface says.
   */
  const handoffCaption = checkedOutHere
    ? "opens a thread in this workspace"
    : pr.head_branch && findWorkspaceForBranch(projectRoot, pr.head_branch)
      ? "opens a thread in the workspace on this branch"
      : "checks out the branch into a worktree, opens a thread";

  const fixCheckWithAgent = useCallback(
    (check: CheckInfo, logExcerpt: string) =>
      handOffToAgent({
        ...handoffBase,
        task: {
          kind: "failing-check",
          checkName: check.name,
          logExcerpt: logExcerpt || null,
          detailUrl: check.detail_url,
        },
      }),
    [handoffBase],
  );

  const sendThreadToAgent = useCallback(
    (task: ReviewThreadTask) => handOffToAgent({ ...handoffBase, task }),
    [handoffBase],
  );

  const resolveConflictsWithAgent = useCallback(
    () => handOffToAgent({ ...handoffBase, task: { kind: "conflicts" } }),
    [handoffBase],
  );

  // ── Drift ──

  const notice = useMemo((): DriftNotice | null => {
    const pending = getReviewDraft(key) || lineDrafts.length > 0;
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
          ...(workspaceId
            ? [
                {
                  label: `Switch to ${baseBranch}`,
                  onClick: () => void switchToDefaultBranch(),
                  emphasis: "strong" as const,
                },
              ]
            : []),
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
          ...(workspaceId
            ? [
                {
                  label: `Switch to ${baseBranch}`,
                  onClick: () => void switchToDefaultBranch(),
                  emphasis: "strong" as const,
                },
              ]
            : []),
        ],
      });
    }

    if (forcePushedAt) {
      const age = relativeAge(new Date(forcePushedAt).toISOString());
      candidates.push({
        kind: "force-pushed",
        tone: "warn",
        message: (
          <>
            Force-pushed {age}
            {lineDrafts.length > 0 &&
              ` · ${unanchoredCount} of your ${plural(lineDrafts.length, "note")} no longer ${
                unanchoredCount === 1 ? "matches" : "match"
              } a line`}
          </>
        ),
        actions:
          lineDrafts.length > 0
            ? [
                // The diff they were written against — not refetched,
                // because a force-pushed commit may already be gone.
                { label: "Show on old diff", onClick: () => openCodeTab("old-diff") },
                {
                  label: "Re-anchor",
                  onClick: () => openCodeTab("repin"),
                  emphasis: "strong" as const,
                },
              ]
            : [{ label: "Refresh", onClick: onRefresh, emphasis: "strong" as const }],
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
            ...(onOpenChanges
              ? [{ label: "Review changes", onClick: onOpenChanges }]
              : []),
            { label: "Stash and pull", onClick: () => void stashAndPull() },
            ...(onOpenChanges
              ? [
                  {
                    label: "Commit first",
                    onClick: onOpenChanges,
                    emphasis: "strong" as const,
                  },
                ]
              : []),
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
            {lineDrafts.length > 0
              ? ` · your ${plural(lineDrafts.length, "note")} are still here`
              : pending
                ? " · your notes are still here"
                : ""}
          </>
        ),
        actions: [
          { label: "Copy as markdown", onClick: () => copyDraft("Copied as markdown") },
          {
            label: "Retry",
            onClick: () => {
              // Retry sends exactly what failed — the notes included.
              const last = lastSubmit.current;
              if (lineDrafts.length > 0) {
                void submitWithNotes(
                  last?.verdict ?? getLastVerdict(key),
                  last?.body ?? getReviewDraft(key),
                );
              } else {
                void runSubmit(getLastVerdict(key), getReviewDraft(key));
              }
            },
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
    workspaceId,
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
    lineDrafts,
    unanchoredCount,
    openCodeTab,
    submitWithNotes,
  ]);

  // ── Branch controls (page only) ──

  const [checkingOut, setCheckingOut] = useState(false);

  const checkOut = useCallback(() => {
    if (checkingOut) return;
    setCheckingOut(true);
    checkOutPr({
      projectRoot,
      headBranch: pr.head_branch,
      prNumber: pr.number,
      existingWorkspaceId,
    })
      .catch((err) => toast.error(String(err)))
      .finally(() => setCheckingOut(false));
  }, [checkingOut, projectRoot, pr.head_branch, pr.number, existingWorkspaceId]);

  const copyBranch = useCallback(() => {
    if (!pr.head_branch) return;
    navigator.clipboard
      .writeText(pr.head_branch)
      .then(() => toast.success("Branch name copied"))
      .catch(() => toast.error("Couldn't copy the branch name"));
  }, [pr.head_branch]);

  // The strip takes a list, so Timeline arrives without the layout below
  // it moving. A host that does not declare the timeline operation gets
  // no Timeline tab — not an empty one.
  const tabs: ReviewTab[] = [
    { id: "summary", label: "Summary" },
    ...(operations.timeline ? [{ id: "timeline", label: "Timeline" }] : []),
    { id: "code", label: "Code", count: pr.changed_files ?? null },
  ];

  // A host that declares nothing cannot even be read here, so there is
  // no detail to render — only the sentence and the browser (4b). The
  // backend refuses these operations anyway; drawing the surface first
  // and letting each control fail would be the same answer delivered
  // five times, badly.
  if (!operations.list_read) {
    return <UnsupportedHostState provider={provider} url={pr.url} />;
  }

  return (
    /**
     * The column owns the viewport height, not its content.
     *
     * Sized to content, the action bar landed wherever the description
     * happened to end — halfway up a tall window, with the rest of the
     * column empty below it. The bar is the thing you come back to; it
     * belongs at the bottom edge of what you can see. So: the column is
     * `h-full`, the tab body is the only part that scrolls, and the
     * notice slot and bar are pinned under it at any window height.
     */
    <div className="flex h-full min-h-0 flex-col" data-testid="review-detail">
      <div className="shrink-0">
      <ReviewHeader
        pr={pr}
        provider={provider}
        repoSlug={repoSlug}
        checkedOutHere={checkedOutHere}
        checkout={
          showCheckout
            ? {
                hasWorkspace: !!existingWorkspaceId,
                busy: checkingOut,
                onCheckOut: checkOut,
                onCopyBranch: copyBranch,
              }
            : undefined
        }
        onRefresh={onRefresh}
      />

      <ReviewTabStrip
        tabs={tabs}
        activeId={activeTab}
        onSelect={setActiveTab}
        trailing={
          activeTab === "timeline" ? (
            <TimelineFilterPicker value={timelineFilter} onChange={setTimelineFilter} />
          ) : undefined
        }
      />
      </div>

      {/* The one scroll on this surface: header and tab strip stay put
          above it, the notice slot and the bar stay put below it. */}
      <ScrollArea
        className="min-h-0 flex-1 [&_[data-slot=scroll-area-viewport]>div]:!block"
        data-testid="review-scroll"
      >
      {activeTab === "timeline" ? (
        <ReviewTimeline
          entries={timelineEntries}
          loading={timelineQuery.isFetching && !timelineQuery.data}
          staleAgeMs={timelineStaleAgeMs}
        />
      ) : activeTab === "code" ? (
        <ReviewCodeTab
          draftKey={key}
          cwd={cwd}
          prNumber={pr.number}
          headOid={pr.head_ref_oid}
          diffText={diffText}
          // `isFetching`, not `isPending`: a query that isn't running
          // reports "pending" forever, and a spinner that never resolves
          // is a worse lie than an empty state.
          loading={diffQuery.isFetching}
          error={diffQuery.error ? String(diffQuery.error) : null}
          // Rendered from the declaration, not from the host's name.
          canDraftLineNotes={operations.line_comments && !readOnly}
          canCommentNow={operations.line_comments && !readOnly}
          onPosted={onRefresh}
          intent={codeIntent}
        />
      ) : (
      /* Status before description, deliberately: what you need to know
         about this PR right now is whether it's healthy, not what its
         author wrote about it a day ago. */
      /* Full width, like the rest of the app's surfaces: the detail is
         a working pane, not a document page. Only the border gutters
         hold it off the edges. */
      <div className="flex flex-1 flex-col gap-3 px-4 py-3">
        <ReviewChecks
          checks={checks}
          isLoading={checksLoading}
          cwd={cwd}
          prNumber={pr.number}
          onFixWithAgent={canHandOff ? fixCheckWithAgent : undefined}
          handoffCaption={handoffCaption}
        />

        {!readOnly && (
          <ReviewReviewers
            pr={pr}
            cwd={cwd}
            // No 4d row of its own: requesting a review is a write to the
            // conversation, which is exactly what `comment` declares.
            canRequestReview={operations.comment}
            onRequested={onRefresh}
          />
        )}

        <ReviewDescription
          body={pr.body}
          cwd={cwd}
          prNumber={pr.number}
          draftKey={key}
          canEdit={operations.comment && isAuthor}
          readOnly={readOnly}
          onSaved={onRefresh}
        />

        <ReviewThreads
          reviews={reviews}
          inlineComments={inlineComments}
          isLoading={commentsLoading}
          onSendToAgent={canHandOff ? sendThreadToAgent : undefined}
        />
      </div>
      )}
      </ScrollArea>

      <div className="shrink-0">
      {/* Pending notes are named on every tab, not just the one they
          were written on — a review nobody can see is easy to forget. */}
      {lineDrafts.length > 0 && !readOnly && (
        <ReviewDraftFooter
          drafts={lineDrafts}
          onDiscard={() => clearLineDrafts(key)}
          onSubmit={() => setSubmitSheetOpen(true)}
        />
      )}

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
        canRequestChanges={operations.request_changes}
        canApprove={operations.approve}
        canComment={operations.comment}
        canMerge={operations.merge_with_strategies}
        canChangeState={operations.draft_ready_close_reopen}
        onSubmitReview={(event, body) => void runSubmit(event, body)}
        onOpenMergeSheet={() => setMergeSheetOpen(true)}
        onPickStrategy={(next) => {
          setStrategy(next);
          setMergeStrategy(next);
        }}
        onReadyForReview={() => void markReady()}
        onClose={() => void closePr()}
        onRebase={() => openPrPath("/conflicts")}
        onResolveConflicts={canHandOff ? resolveConflictsWithAgent : undefined}
      />
      </div>

      {submitSheetOpen && (
        <ReviewSubmitSheet
          open
          prNumber={pr.number}
          drafts={lineDrafts}
          // One draft pool: whatever is half-written in the action bar
          // is the same text this review's body starts from.
          initialBody={getReviewDraft(key)}
          initialVerdict={getLastVerdict(key)}
          canRequestChanges={operations.request_changes}
          canApprove={operations.approve}
          submitting={submitting}
          blockedReason={blockedSubmitReason}
          onReanchor={() => {
            setSubmitSheetOpen(false);
            openCodeTab("repin");
          }}
          onCancel={() => setSubmitSheetOpen(false)}
          onSubmit={(verdict, body) => void submitWithNotes(verdict, body)}
        />
      )}

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
