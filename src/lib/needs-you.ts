import { getWorkspaceStatus } from "@/lib/pane-status";
import { activateWorkspaceInteraction } from "@/lib/perf/instrumented-activate";
import { selectActiveWorkspaceId, useAppStore } from "@/stores/app-store";
import {
  useSidebarDensityStore,
  type StatusMark,
} from "@/stores/sidebar-density-store";
import type { PaneStatus, WorkspaceSnapshot } from "@/tauri/types";

/**
 * Order blocked items oldest-blocked first, by each one's `permission` mark in
 * the sidebar density store: the agent stuck longest is the one to unblock
 * first. An item with no mark yet ranks as newest (it just started blocking as
 * far as we can tell); ties keep the input order.
 */
export function sortOldestBlockedFirst<T>(
  items: readonly T[],
  idOf: (item: T) => string,
  statusSince: Readonly<Record<string, StatusMark>>,
): T[] {
  return items
    .map((item, index) => {
      const mark = statusSince[idOf(item)];
      return {
        item,
        index,
        blockedAt: mark?.status === "permission" ? mark.at : Number.POSITIVE_INFINITY,
      };
    })
    .sort(
      // `Infinity - Infinity` is NaN; NaN and 0 both fall through `||` to the
      // stable input-order tie-break.
      (a, b) => a.blockedAt - b.blockedAt || a.index - b.index,
    )
    .map(({ item }) => item);
}

/** Every workspace waiting on the user, oldest-blocked first. */
export function needsYouWorkspaceIds(
  workspaces: readonly WorkspaceSnapshot[],
  paneStatuses: Readonly<Record<string, PaneStatus>>,
  statusSince: Readonly<Record<string, StatusMark>>,
): string[] {
  const blocked = workspaces.filter(
    (ws) => getWorkspaceStatus(ws.surfaces, paneStatuses) === "permission",
  );
  return sortOldestBlockedFirst(blocked, (ws) => ws.workspace_id, statusSince).map(
    (ws) => ws.workspace_id,
  );
}

/**
 * Where "jump to the next workspace that needs you" lands: the longest-waiting
 * one, or, when the user is already on a blocked workspace, the one after it
 * so repeated presses walk the whole list. Null when there is nowhere else to
 * go.
 */
export function nextNeedsYouTarget(
  ids: readonly string[],
  currentId: string | null,
): string | null {
  if (ids.length === 0) return null;
  const at = currentId ? ids.indexOf(currentId) : -1;
  const target = at === -1 ? ids[0] : ids[(at + 1) % ids.length];
  return target === currentId ? null : target;
}

/** Activate the next workspace that needs the user. False when none does, or
 *  the user is already on the only one. */
export function jumpToNextNeedsYou(): boolean {
  const store = useAppStore.getState();
  const ids = needsYouWorkspaceIds(
    store.appState?.workspaces ?? [],
    store.appState?.pane_statuses ?? {},
    useSidebarDensityStore.getState().statusSince,
  );
  const target = nextNeedsYouTarget(ids, selectActiveWorkspaceId(store));
  if (!target) return false;
  activateWorkspaceInteraction(target).catch(console.error);
  return true;
}
