/**
 * Agent runs against a pull request — the local half of the timeline.
 *
 * This is the one thing in the Timeline tab that no host can render:
 * Codemux knows which agent run was started from which review comment or
 * failing check, so the run belongs in the pull request's history next to
 * the thing that prompted it.
 *
 * Two properties matter, and they pull in opposite directions:
 *
 * - **Local.** A run is never pushed to the host. It is a record of what
 *   *you* did in *this* installation, and a teammate reading the same PR
 *   on the web must not see it. That is also why the Timeline's filter
 *   can drop to "Host only": it shows what they would see.
 * - **Durable.** A run that vanishes on restart is worse than no record
 *   at all — the history would silently disagree with itself depending on
 *   when you last quit. So it persists, the same way the adjacent
 *   `lastSelectedAgentId` does: zustand `persist` over localStorage.
 *
 * Bounded on write *and* on persist. Run history is append-only and
 * nothing ever prunes it by hand, so an unbounded map is a localStorage
 * blob that grows for the life of the install.
 */

import { create } from "zustand";
import { persist, createJSONStorage } from "zustand/middleware";

const STORAGE_KEY = "codemux:pr-agent-runs:v1";

/** How many runs are kept, newest first. Roughly a year of ordinary use;
 *  far enough back that no one reaches the edge, small enough that the
 *  persisted blob stays trivial. */
export const MAX_RUNS = 200;

export type AgentRunKind = "failing-check" | "review-thread" | "conflicts";

export interface AgentRunRecord {
  /** Unique per run — the React key, and the identity for de-duplication
   *  if the same handoff is somehow recorded twice. */
  id: string;
  /**
   * `owner/repo#285`, matching what both review surfaces compute for the
   * PR they are showing. The primary identity.
   */
  prRef: string;
  /** Secondary identity, for a PR whose slug was unknown when recorded —
   *  the repo root is always known, so a run can never become orphaned. */
  projectRoot: string;
  prNumber: number;
  kind: AgentRunKind;
  /** What the agent was pointed at: a check name, `path:line`, or the
   *  base branch for a conflict. Rendered as the card's first line. */
  summary: string;
  /** Where the thread landed. */
  workspaceId: string;
  workspaceTitle: string;
  /** The tab the thread opened in, when the handoff created one — so
   *  "Open thread" lands on the conversation rather than the workspace.
   *  Null when the worktree route carried the prompt at creation time and
   *  no tab was created here. */
  threadTabId: string | null;
  /** Epoch ms. Sorted against host events by this. */
  createdAt: number;
  /**
   * Diff shape, only when it was already known.
   *
   * Deliberately optional and deliberately never computed: at the moment
   * a handoff is recorded the agent has not written anything yet, so any
   * number here would be a number about the wrong commit. The card omits
   * the line rather than showing a stale or invented one.
   */
  files?: number;
  additions?: number;
  deletions?: number;
}

interface AgentRunsState {
  /** Newest first. A flat list rather than a map keyed by PR: a run has
   *  two identities (host ref and repo path) and one list matches both
   *  without either key having to be canonical. */
  runs: AgentRunRecord[];
  record: (run: AgentRunRecord) => void;
  /** Attach diff stats to a run once they are genuinely known. */
  annotate: (
    id: string,
    stats: Pick<AgentRunRecord, "files" | "additions" | "deletions">,
  ) => void;
  /** Test seam, and the reset behind a "clear local history" action. */
  clear: () => void;
}

export const usePrAgentRunsStore = create<AgentRunsState>()(
  persist(
    (set) => ({
      runs: [],
      record: (run) =>
        set((s) => ({
          runs: [run, ...s.runs.filter((r) => r.id !== run.id)].slice(0, MAX_RUNS),
        })),
      annotate: (id, stats) =>
        set((s) => ({
          runs: s.runs.map((r) => (r.id === id ? { ...r, ...stats } : r)),
        })),
      clear: () => set({ runs: [] }),
    }),
    {
      name: STORAGE_KEY,
      storage: createJSONStorage(() => localStorage),
      // Capped again on the way out: `record` bounds the common path, but
      // a shape change or a hand-edited blob must not be able to persist
      // something unbounded.
      partialize: (s) => ({ runs: s.runs.slice(0, MAX_RUNS) }),
    },
  ),
);

/** The identity both review surfaces compute for the PR they render. */
export function prRefKey(repoSlug: string | null, prNumber: number): string {
  return `${repoSlug ?? ""}#${prNumber}`;
}

/**
 * Runs belonging to one pull request, oldest first.
 *
 * Matches on either identity: the host ref when the slug resolved on both
 * the writing and the reading surface, or the repo path and number when
 * it did not. A run recorded from the panel is therefore still found by
 * the page, which is the whole point of recording the pair.
 */
export function selectRunsForPr(
  runs: AgentRunRecord[],
  pr: { prRef: string; projectRoot: string; prNumber: number },
): AgentRunRecord[] {
  return runs
    .filter(
      (r) =>
        r.prRef === pr.prRef ||
        (r.projectRoot === pr.projectRoot && r.prNumber === pr.prNumber),
    )
    .sort((a, b) => a.createdAt - b.createdAt);
}
