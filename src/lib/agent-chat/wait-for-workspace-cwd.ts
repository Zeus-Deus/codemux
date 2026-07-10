import { getAppState } from "@/tauri/commands";
import { useAppStore } from "@/stores/app-store";

/** How long to wait for a freshly-created workspace's real cwd to reach
 *  the app-store before giving up. Worktree creation itself
 *  (`git worktree add` + optional setup) already resolved by the time we
 *  call this — we're only waiting for the async `app-state-changed`
 *  event to propagate the new workspace into the Zustand store, which is
 *  normally a single IPC tick. 5s is generous headroom for a cold repo /
 *  a briefly backed-up event queue while still failing fast enough that a
 *  genuinely stuck create surfaces to the user instead of hanging. */
const DEFAULT_TIMEOUT_MS = 5_000;

/** Interval between direct `get_app_state` fetches on the fallback path. */
const POLL_INTERVAL_MS = 200;

/** Read a workspace's cwd from the current app-store snapshot, or `null`
 *  when the workspace isn't present yet (or is present but has no cwd). */
function readCwd(workspaceId: string): string | null {
  const ws = useAppStore
    .getState()
    .appState?.workspaces.find((w) => w.workspace_id === workspaceId);
  return ws?.cwd || null;
}

/**
 * Resolve a just-created workspace's real cwd, waiting for it to appear
 * in the app-store.
 *
 * Motivation (root cause of the PR #142 deferred-worktree cwd
 * regression): a deferred worktree workspace only
 * reaches the Zustand store via the async `app-state-changed` Tauri
 * event, which has essentially never been processed by the time
 * `create_worktree_workspace`'s `invoke` promise resolves. A synchronous
 * `useAppStore.getState()` read therefore misses ~always, which silently
 * left the pane + agent session launching at the PARENT checkout cwd
 * (the project root) instead of the new worktree — so agents committed
 * into the user's real working copy.
 *
 * Resolution strategy — race two sources so this returns as soon as
 * either lands the workspace:
 *   1. `useAppStore.subscribe` — fires when the `app-state-changed`
 *      event updates the store (the normal, fast path).
 *   2. A polled direct `get_app_state` fetch — a fallback for the case
 *      where event delivery is reordered/dropped. Each fetch is written
 *      back into the store via `setAppState`, so it both unblocks us and
 *      lets the rest of the UI catch up; the write also wakes the
 *      subscription above, keeping the resolve logic in one place.
 *
 * @returns the workspace's cwd, or `null` if it never appears within
 *   `timeoutMs`. Callers MUST treat `null` as a hard failure and must
 *   NOT fall back to a parent/project-root cwd.
 */
export async function waitForWorkspaceCwd(
  workspaceId: string,
  timeoutMs: number = DEFAULT_TIMEOUT_MS,
): Promise<string | null> {
  // Fast path: already in the store (e.g. a synchronous emit, or a retry
  // after the event already landed).
  const immediate = readCwd(workspaceId);
  if (immediate) return immediate;

  return new Promise<string | null>((resolve) => {
    let settled = false;
    const finish = (value: string | null): void => {
      if (settled) return;
      settled = true;
      unsubscribe();
      clearTimeout(timer);
      clearInterval(poll);
      resolve(value);
    };

    // 1. Store subscription — the normal `app-state-changed` path.
    const unsubscribe = useAppStore.subscribe(() => {
      const cwd = readCwd(workspaceId);
      if (cwd) finish(cwd);
    });

    // 2. Direct-fetch fallback — poll `get_app_state` and hydrate the
    //    store so a dropped/reordered event can't strand us. Writing the
    //    fresh snapshot back wakes the subscription above, which does the
    //    actual resolve.
    const poll = setInterval(() => {
      void getAppState()
        .then((snapshot) => {
          if (!settled) useAppStore.getState().setAppState(snapshot);
        })
        .catch(() => {
          /* transient fetch failure — the next tick retries */
        });
    }, POLL_INTERVAL_MS);

    const timer = setTimeout(() => finish(null), timeoutMs);
  });
}
