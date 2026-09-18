/**
 * Which workspaces THIS client brought into view.
 *
 * The desktop and remote clients share workspace data, but each remote
 * client owns its selection. A view seeded on connect can still encounter a
 * workspace another client is filling. Only explicit local navigation may
 * authorize auto-repair of an empty workspace after boot.
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

/**
 * Set when this client issues a command whose BACKEND fallback moves the
 * active workspace on its own — closing, closing-with-worktree, or
 * archiving a workspace all land on whatever workspace is left, and no
 * client calls `activateWorkspace` for it. Without this marker that
 * fallback reads as somebody else's navigation and the workspace we
 * ourselves emptied the sidebar into never gets filled.
 *
 * Recorded before the invoke, so it does not depend on the app-state
 * event losing the race to the command's own promise.
 */
let pendingFallbackActivation = false;

/** Record that a command this client just issued may make the backend
 *  pick the next active workspace. */
export function noteLocalActivationFallback(): void {
  pendingFallbackActivation = true;
}

/** Adopt `workspaceId` as ours when we are waiting on such a fallback.
 *  Always consumes the marker, even when the workspace we landed on was
 *  already ours: the marker describes one command's fallback, and leaving
 *  it armed would hand the next activation to arrive — possibly another
 *  client's — to this client as well. Returns whether anything new was
 *  recorded. */
export function adoptLocalFallbackActivation(workspaceId: string): boolean {
  if (!pendingFallbackActivation) return false;
  pendingFallbackActivation = false;
  if (wasActivatedLocally(workspaceId)) return false;
  noteLocalWorkspaceActivation(workspaceId);
  return true;
}

/** Test-only: drop every recorded activation. */
export function resetLocalWorkspaceActivations(): void {
  locallyActivated.clear();
  pendingFallbackActivation = false;
}
