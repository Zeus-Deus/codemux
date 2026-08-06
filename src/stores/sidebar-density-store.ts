import { create } from "zustand";
import type { ActivePaneStatus, WorkspaceSnapshot } from "@/tauri/types";

/** How long a freshly-settled row keeps its 2-line "done" treatment and its
 *  fading green ✓ before decaying back to a calm one-liner. */
export const SETTLED_FADE_MS = 60 * 60 * 1000; // ~1h

/** Fallback blocker summary for a "needs you" surface. No backend-provided
 *  permission-question / merge-conflict text is reachable from the sidebar
 *  yet, so both the permission row's line 2 and the pinned "Needs you" strip
 *  fall back to this generic prompt. */
export const PERMISSION_BLOCKER_FALLBACK = "Waiting for your input";

/** The one-line blocker summary for a workspace that is waiting on the user.
 *  Extracted so the permission row and the pinned strip render identical text
 *  and so a real, backend-derived summary can later be threaded through this
 *  single place. */
export function permissionBlockerText(_workspace: WorkspaceSnapshot): string {
  return PERMISSION_BLOCKER_FALLBACK;
}

interface StatusMark {
  status: ActivePaneStatus;
  /** Client-side timestamp of when the workspace entered this status. The
   *  backend does not stamp a status-changed-at, so the sidebar derives
   *  "elapsed" from this locally-observed transition. */
  at: number;
}

/** One merged PR shipped from a workspace, as surfaced by the idle row's
 *  "n shipped" tally + its hover popover. `issueNumber` / `title` come from
 *  the linked issue the PR closed; when a merge is observed with no linked
 *  issue, only `prNumber` is known and the popover falls back to it. */
export interface ShippedRecord {
  prNumber: number;
  issueNumber?: number;
  title?: string;
}

/** Client-observed work history for a single workspace. Non-persisted (v1):
 *  rebuilt from live `(linked_issue.number, pr_number, pr_state)` transitions
 *  each session. */
interface WorkHistory {
  /** Signature of the last observed `(issue#, pr#, prState)` tuple — lets the
   *  observer bail without a state write when nothing changed. */
  signature: string;
  /** Last observed linked-issue number (null when none). A switch to a
   *  *different* non-null issue means new work started. */
  lastIssueNumber: number | null;
  /** The current merged PR + the issue it shipped, held as a candidate until
   *  a new different issue is linked — at which point it's promoted into
   *  {@link shipped} and its PR retires from the leading icon. This is the
   *  "baseline" the README describes: a merge visible at app start (or newly
   *  observed) is a candidate, promoted only once the workspace moves on. */
  baseline: ShippedRecord | null;
  /** Merged PRs that have retired (promoted after new work started), oldest
   *  first. Drives the tally count and the popover list. */
  shipped: ShippedRecord[];
}

/** Inputs the row feeds the store each render to observe the living-row
 *  lifecycle. */
export interface WorkObservation {
  issueNumber: number | null;
  issueTitle: string | null;
  prNumber: number | null;
  prState: string | null;
}

interface SidebarDensityStore {
  /** When each workspace entered its current active agent status. Drives the
   *  elapsed timer on working / permission rows; cleared on return to idle. */
  statusSince: Record<string, StatusMark>;
  /** When each workspace most recently entered `review` (finished). Retained
   *  after the status clears so the fading green ✓ can decay over ~1h. */
  settledAt: Record<string, number>;
  /** When each workspace was last activated (opened / seen). Collapses a done
   *  (review) row back to a one-liner once the user has looked at it. */
  lastSeenAt: Record<string, number>;
  /** Client-observed work history per workspace id — merged-PR retirement +
   *  the "n shipped" tally. Non-persisted. */
  workHistory: Record<string, WorkHistory>;
  /** Record a workspace's derived agent status. No-op when unchanged, so it
   *  is safe to call from an effect on every render. */
  observeStatus: (workspaceId: string, status: ActivePaneStatus | null) => void;
  /** Observe the living-row lifecycle `(linked issue, PR, PR state)`. No-op
   *  when the tuple is unchanged, so it is safe to call from an effect on
   *  every render. Promotes a retired merge into `shipped` and tracks the
   *  merged-PR baseline. */
  observeWork: (workspaceId: string, obs: WorkObservation) => void;
  /** Mark a workspace as seen (activated). */
  markSeen: (workspaceId: string) => void;
}

export const useSidebarDensityStore = create<SidebarDensityStore>(
  (set, get) => ({
    statusSince: {},
    settledAt: {},
    lastSeenAt: {},
    workHistory: {},
    observeStatus: (workspaceId, status) => {
      const prev = get().statusSince[workspaceId]?.status ?? null;
      if (prev === status) return;
      const now = Date.now();
      set((s) => {
        const statusSince = { ...s.statusSince };
        if (status === null) {
          delete statusSince[workspaceId];
        } else {
          statusSince[workspaceId] = { status, at: now };
        }
        // A review transition stamps the settle time; other transitions
        // leave any prior settle time in place to keep the ✓ fading.
        const settledAt =
          status === "review"
            ? { ...s.settledAt, [workspaceId]: now }
            : s.settledAt;
        return { statusSince, settledAt };
      });
    },
    observeWork: (workspaceId, obs) => {
      const signature = `${obs.issueNumber ?? ""}|${obs.prNumber ?? ""}|${obs.prState ?? ""}`;
      const existing = get().workHistory[workspaceId];
      // Nothing observable changed → no state write (safe in a render effect).
      if (existing && existing.signature === signature) return;

      set((s) => {
        const prev = s.workHistory[workspaceId];
        const lastIssueNumber = prev?.lastIssueNumber ?? null;
        let baseline = prev?.baseline ?? null;
        let shipped = prev?.shipped ?? [];
        const merged = (obs.prState ?? "").toLowerCase() === "merged";

        // New work started = the linked issue switches to a *different*
        // non-null issue. If a merged PR was pending (the baseline), it now
        // retires: promote it into the shipped history (dedup by PR number,
        // so a lingering merged pr_state can't double-count).
        const issueMovedOn =
          lastIssueNumber != null &&
          obs.issueNumber != null &&
          obs.issueNumber !== lastIssueNumber;
        if (issueMovedOn && baseline) {
          if (!shipped.some((r) => r.prNumber === baseline!.prNumber)) {
            shipped = [...shipped, baseline];
          }
          baseline = null;
        }

        // Track the current merged PR as the baseline candidate (the merge
        // that will retire when the next different issue is linked). Skip PRs
        // already shipped, and don't rebuild a baseline that already names
        // this PR — the first observation captures the richest issue context.
        if (
          merged &&
          obs.prNumber != null &&
          baseline?.prNumber !== obs.prNumber &&
          !shipped.some((r) => r.prNumber === obs.prNumber)
        ) {
          baseline = {
            prNumber: obs.prNumber,
            ...(obs.issueNumber != null ? { issueNumber: obs.issueNumber } : {}),
            ...(obs.issueTitle != null ? { title: obs.issueTitle } : {}),
          };
        }

        return {
          workHistory: {
            ...s.workHistory,
            [workspaceId]: {
              signature,
              // Preserve the prior issue number when the current one is null
              // (issue temporarily unlinked) so an A → null → B move still
              // reads as new work started.
              lastIssueNumber: obs.issueNumber ?? lastIssueNumber,
              baseline,
              shipped,
            },
          },
        };
      });
    },
    markSeen: (workspaceId) =>
      set((s) => ({
        lastSeenAt: { ...s.lastSeenAt, [workspaceId]: Date.now() },
      })),
  }),
);

/** Whether a finished (`review`) workspace still shows its expanded "done"
 *  treatment: it has not been seen since it settled and it is younger than
 *  {@link SETTLED_FADE_MS}. Shared by the row's density branch and the
 *  "gather on top" live-membership predicate so the two never drift. */
export function isReviewExpanded(
  settledAt: number | undefined,
  lastSeenAt: number | undefined,
  now: number = Date.now(),
): boolean {
  if (settledAt == null) return false;
  const seenSinceReview = lastSeenAt != null && lastSeenAt >= settledAt;
  return !seenSinceReview && now - settledAt < SETTLED_FADE_MS;
}

/** Whether a workspace counts as "live" for the "gather on top" grouping:
 *  a working or permission agent, or a review row that is still expanded
 *  (unseen and fresh). A settled/seen review row is NOT live.
 *
 *  Neither is a `monitoring` one. "Gather on top" is for work the user should
 *  be looking at right now; a watch loop is explicitly the opposite of that,
 *  so it keeps its dot and its place in the list rather than being promoted. */
export function isWorkspaceLive(
  status: ActivePaneStatus | null,
  settledAt: number | undefined,
  lastSeenAt: number | undefined,
  now: number = Date.now(),
): boolean {
  if (status === "working" || status === "permission") return true;
  if (status === "review") return isReviewExpanded(settledAt, lastSeenAt, now);
  return false;
}

/** Whether a workspace's current PR has retired — its merged signal was
 *  promoted into `shipped` history when newer work was linked. The row uses
 *  this to suppress the PR icon so a superseded merge falls back to the plain
 *  gray branch icon (the work title carries the current signal instead). */
export function isRetiredPr(
  shipped: ShippedRecord[] | undefined,
  prNumber: number | null | undefined,
): boolean {
  if (prNumber == null || !shipped) return false;
  return shipped.some((r) => r.prNumber === prNumber);
}

/** Human-friendly elapsed label: `45s` / `6m` / `1h12m` / `4d3h`. Settled rows
 *  can sit for days, so anything past 24h reads in days rather than a
 *  three-digit hour count. */
export function formatElapsed(ms: number): string {
  const totalSec = Math.max(0, Math.floor(ms / 1000));
  if (totalSec < 60) return `${totalSec}s`;
  const totalMin = Math.floor(totalSec / 60);
  if (totalMin < 60) return `${totalMin}m`;
  const totalHours = Math.floor(totalMin / 60);
  if (totalHours < 24) {
    const mins = totalMin % 60;
    return mins > 0 ? `${totalHours}h${mins}m` : `${totalHours}h`;
  }
  const days = Math.floor(totalHours / 24);
  const hours = totalHours % 24;
  return hours > 0 ? `${days}d${hours}h` : `${days}d`;
}
