/**
 * Which workspaces THIS client brought into view.
 *
 * Every client — the desktop window and each remote web client — talks to
 * one backend and reads one app-state snapshot, including a single shared
 * `active_workspace_id`. So "the active workspace is empty" is not a fact
 * about this client: when the desktop creates a workspace and switches to
 * it, every other client sees that switch too, and any client-local guard
 * (an in-flight ref, a draft pointer) is blind to what the others are
 * already doing. Auto-repair effects that react to the active workspace
 * therefore fan out — each client injects its own pane into a workspace
 * the first one is still populating.
 *
 * This is the missing half of that check: a client may only auto-repair a
 * workspace it itself navigated to. Recorded inside the `activateWorkspace`
 * command wrapper, which every navigation path bottoms out in — an
 * activation this client issues is local whatever surface asked for it — so
 * no caller has to remember to record anything.
 *
 * Deliberately module-local, not a store: nothing renders from it, and a
 * store subscription would only add re-render churn. Ids are never
 * removed — a workspace this client visited once stays "mine" for the
 * session, which is the conservative direction (the alternative is
 * refusing to repair a workspace the user really is looking at).
 */

const locallyActivated = new Set<string>();

/** Record that this client navigated to (or just created) `workspaceId`. */
export function noteLocalWorkspaceActivation(workspaceId: string): void {
  if (workspaceId.length === 0) return;
  locallyActivated.add(workspaceId);
}

/** Whether this client brought `workspaceId` into view itself, as opposed
 *  to seeing another client's activation arrive in the shared snapshot. */
export function wasActivatedLocally(workspaceId: string): boolean {
  return locallyActivated.has(workspaceId);
}

/** Test-only: drop every recorded activation. */
export function resetLocalWorkspaceActivations(): void {
  locallyActivated.clear();
}
