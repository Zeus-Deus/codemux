/** The only workspace field navigation needs. Keeping the selector structural
 * makes it usable with snapshots, tests, and non-React callers alike. */
export interface WorkspaceNavigationItem {
  workspace_id: string;
}

/**
 * Select a workspace by the same ordered, modular rule as Rust's
 * `AppStateStore::workspace_navigation_target`.
 *
 * The current id is explicit so renderer callers can pass the optimistically
 * selected workspace while backend/control callers can pass the snapshot id.
 */
export function selectWorkspaceNavigationTarget(
  workspaces: readonly WorkspaceNavigationItem[],
  currentWorkspaceId: string | null | undefined,
  step: number,
): string | null {
  if (workspaces.length === 0 || currentWorkspaceId == null) return null;

  const currentIndex = workspaces.findIndex(
    (workspace) => workspace.workspace_id === currentWorkspaceId,
  );
  if (currentIndex < 0) return null;

  const normalizedStep = Number.isFinite(step) ? Math.trunc(step) : 0;
  const nextIndex =
    ((currentIndex + normalizedStep) % workspaces.length + workspaces.length) %
    workspaces.length;
  return workspaces[nextIndex]?.workspace_id ?? null;
}
