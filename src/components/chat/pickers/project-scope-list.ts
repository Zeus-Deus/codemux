import type { ProjectGroup } from "@/stores/app-store";
import type { SettledEntry, SnoozeEntry } from "@/stores/sidebar-inbox-store";

/**
 * Ordering + sectioning for the Thread Scope location picker ("Run in").
 *
 * The picker lists every project that has a live workspace, which is the
 * same set the sidebar draws from — but the sidebar splits that set into
 * "active" and parked tiers (settled and snoozed), and the picker used to
 * render it as one
 * flat, unsorted, uncapped list. On a machine with a dozen-plus adopted
 * projects that reads as a wall of equals: the project you touched a
 * minute ago sits below one you settled months ago, because the only
 * ordering was workspace creation order.
 *
 * So the picker mirrors the sidebar's own vocabulary:
 *
 *  - **Active** — at least one workspace is neither settled nor snoozed,
 *    i.e. it still has a card in the sidebar's active list. Ordered
 *    most-recently-active first.
 *  - **Settled** — every workspace is parked (settled or snoozed). Still
 *    fully open (neither state closes anything), just parked, so it
 *    belongs below the fold rather than hidden. Ordered
 *    most-recently-parked first, matching the sidebar's settled section.
 *
 * All signals come from the sidebar inbox store, so the picker's order
 * can never disagree with what the sidebar shows.
 */

/** How many settled projects render before the "Show N more" tail. Keeps a
 *  long-parked backlog from burying the "Open another project…" action. */
export const SETTLED_COLLAPSED_COUNT = 4;

export interface ScopeProjectSections {
  /** Projects with at least one unparked workspace, most-recently-active first. */
  active: ProjectGroup[];
  /** Projects whose every workspace is parked (settled or snoozed),
   *  most-recently-parked first. */
  settled: ProjectGroup[];
}

/**
 * Split grouped projects into the picker's Active / Settled sections and
 * order each by recency.
 *
 * `activity` is the inbox store's client-side last-activity stamp map
 * (workspace id → ms epoch). It is only stamped while the sidebar inbox is
 * mounted, so a workspace can legitimately have no stamp; those rank last
 * and keep their app-state order, because `Array.prototype.sort` is stable.
 * That makes the ordering strictly better than insertion order rather than
 * dependent on a signal we can't guarantee.
 *
 * "Parked" folds BOTH of the sidebar's parked lifecycles — settled and
 * snoozed — because either one removes the workspace's card from the
 * active list: a project whose workspaces are all snoozed must not read
 * as Active here any more than a fully-settled one. If another parked
 * state ever ships, it MUST be folded into this partition the same way —
 * otherwise a project whose workspaces are all parked in the new state
 * would read as Active.
 */
export function partitionProjectScopes(
  groups: ProjectGroup[],
  settled: SettledEntry[],
  snoozed: SnoozeEntry[],
  activity: Record<string, number>,
): ScopeProjectSections {
  // Workspace id → when it was parked. A snooze records when the deferral
  // was made (`at`), which is the moment the card left the active list —
  // the parked-recency analogue of a settle sweep's `at`.
  const parkedAt = new Map<string, number>();
  for (const entry of settled) parkedAt.set(entry.id, entry.at);
  for (const entry of snoozed) {
    // Settled supersedes snoozed in the store (mutually exclusive shelves),
    // but a stale blob could carry both — keep the settle stamp.
    if (!parkedAt.has(entry.id)) parkedAt.set(entry.id, entry.at);
  }

  const activeRanked: Array<{ group: ProjectGroup; rank: number }> = [];
  const settledRanked: Array<{ group: ProjectGroup; rank: number }> = [];

  for (const group of groups) {
    const workspaces = group.workspaces;
    // A project leaves the sidebar's active list only when EVERY one of its
    // workspaces is parked — one unparked worktree still draws a card, so
    // the project is still active work. A group with no workspaces has
    // nothing to park and stays active.
    const allParked =
      workspaces.length > 0 &&
      workspaces.every((ws) => parkedAt.has(ws.workspace_id));

    if (allParked) {
      const rank = Math.max(
        ...workspaces.map((ws) => parkedAt.get(ws.workspace_id) ?? 0),
      );
      settledRanked.push({ group, rank });
    } else {
      // Seeded with 0 so an empty group (or one with no stamps) is still a
      // finite rank rather than -Infinity.
      const rank = Math.max(
        0,
        ...workspaces.map((ws) => activity[ws.workspace_id] ?? 0),
      );
      activeRanked.push({ group, rank });
    }
  }

  const byRecencyDesc = (
    a: { rank: number },
    b: { rank: number },
  ): number => b.rank - a.rank;

  return {
    active: activeRanked.sort(byRecencyDesc).map((entry) => entry.group),
    settled: settledRanked.sort(byRecencyDesc).map((entry) => entry.group),
  };
}

/**
 * The slice of the settled section to render.
 *
 * Collapsed to {@link SETTLED_COLLAPSED_COUNT} by default; a search or an
 * explicit expand reveals the rest. The currently-targeted project is always
 * included even when it falls in the hidden tail — a checkmark the user
 * cannot see reads as "my selection was lost".
 */
export function visibleSettledProjects(
  settled: ProjectGroup[],
  opts: {
    expanded: boolean;
    searching: boolean;
    activeProjectPath: string | null;
  },
): ProjectGroup[] {
  if (opts.expanded || opts.searching) return settled;

  const head = settled.slice(0, SETTLED_COLLAPSED_COUNT);
  const { activeProjectPath } = opts;
  if (!activeProjectPath) return head;
  if (head.some((group) => group.projectPath === activeProjectPath)) return head;

  const current = settled.find(
    (group) => group.projectPath === activeProjectPath,
  );
  return current ? [...head, current] : head;
}
