import { useMemo } from "react";
import { create } from "zustand";
import type {
  AppStateSnapshot,
  PaneNodeSnapshot,
  WorkspaceSnapshot,
  SurfaceSnapshot,
} from "@/tauri/types";

interface AppStore {
  appState: AppStateSnapshot | null;
  /** Cached `$HOME` string from the `get_home_dir` Tauri call. Null
   *  until hydrated at App mount. Consumers (project-group selector,
   *  lazy-draft home detection) should treat null as "not yet known"
   *  and fall back to today's path-basename grouping. */
  homeDir: string | null;
  setAppState: (snapshot: AppStateSnapshot) => void;
  setHomeDir: (homeDir: string) => void;
}

export const useAppStore = create<AppStore>((set) => ({
  appState: null,
  homeDir: null,
  setAppState: (snapshot) => set({ appState: snapshot }),
  setHomeDir: (homeDir) => set({ homeDir }),
}));

// Derived selectors
//
// IMPORTANT — re-render economics:
//
// The Rust backend rebuilds the full `AppStateSnapshot` (workspaces,
// surfaces, panes, tabs) on every `emit_app_state` and ships it to the
// renderer over Tauri IPC. Every workspace object — and every nested
// surfaces/tabs array — therefore has a *fresh reference* on every
// backend tick. Agent token streaming, the 5-second git poll, hook
// events, and PR/port refreshes all fire emit_app_state, so under
// normal use these ticks happen many times per second.
//
// `useActiveWorkspace()` returns the matching workspace OBJECT from a
// fresh snapshot. Even when nothing about the active workspace
// semantically changed, the returned reference differs every tick, so
// every consumer subscribed via this hook re-renders on every tick.
// That cascaded into MarkdownRendered (full markdown re-parse), the
// pane container, the tab bar, etc.
//
// Mitigation:
//
// 1. Primitive-slice selectors (`useActiveWorkspaceId`,
//    `useActiveWorkspaceCwd`, etc.) below return primitive strings
//    that compare with `===` and are stable across backend ticks
//    unless the slice itself changed. Lightweight consumers (title-bar
//    SearchTrigger, run-button, file-search dialog, content-search
//    dialog) use these instead of the full-object selector.
//
// 2. `useActiveWorkspace()` is kept for consumers that legitimately
//    need the full workspace object (WorkspaceMain). The heaviest cost
//    downstream of WorkspaceMain — react-markdown re-parsing — is
//    mitigated separately by `React.memo` on `MarkdownRendered`.
//
// `useShallow` would NOT help here. It does shallow compare, but the
// Rust snapshot rebuild gives every nested array (surfaces, tabs)
// fresh refs, so shallow compare always trips.
export function useActiveWorkspace(): WorkspaceSnapshot | null {
  return useAppStore((s) => {
    if (!s.appState) return null;
    return (
      s.appState.workspaces.find(
        (w) => w.workspace_id === s.appState!.active_workspace_id,
      ) ?? null
    );
  });
}

/** Primitive: the active workspace's id, or null when no workspace is
 *  active. Stable across backend ticks unless the active workspace
 *  actually changes. Use this in components that only need the id. */
export function useActiveWorkspaceId(): string | null {
  return useAppStore((s) => s.appState?.active_workspace_id ?? null);
}

/** Primitive: the active workspace's cwd, or null. Used by search
 *  dialogs that need a path, not the whole workspace. Returns the
 *  primitive string so subscribers don't churn on full-snapshot
 *  rebuilds. */
export function useActiveWorkspaceCwd(): string | null {
  return useAppStore((s) => {
    const id = s.appState?.active_workspace_id;
    if (!id) return null;
    const ws = s.appState!.workspaces.find((w) => w.workspace_id === id);
    return ws?.cwd ?? null;
  });
}

/** Primitive: the active workspace's project_root, or null. */
export function useActiveWorkspaceProjectRoot(): string | null {
  return useAppStore((s) => {
    const id = s.appState?.active_workspace_id;
    if (!id) return null;
    const ws = s.appState!.workspaces.find((w) => w.workspace_id === id);
    return ws?.project_root ?? null;
  });
}

/** Primitive: the active workspace's git branch, or null. Used for
 *  the search-trigger label in the title bar. */
export function useActiveWorkspaceBranch(): string | null {
  return useAppStore((s) => {
    const id = s.appState?.active_workspace_id;
    if (!id) return null;
    const ws = s.appState!.workspaces.find((w) => w.workspace_id === id);
    return ws?.git_branch ?? null;
  });
}

/** Subscribe to the cached home directory. Null until the initial
 *  `getHomeDir()` Tauri call resolves at App mount. */
export function useHomeDir(): string | null {
  return useAppStore((s) => s.homeDir);
}

/** Recursively walk a pane tree checking whether the given pane id is
 *  present anywhere under the node. */
function paneTreeContains(node: PaneNodeSnapshot, paneId: string): boolean {
  if (node.pane_id === paneId) return true;
  if (node.kind === "split") {
    return node.children.some((child) => paneTreeContains(child, paneId));
  }
  return false;
}

/** Recursively collect every terminal pane's `session_id` under the given
 *  node into the supplied workspace map. Mutates `out` in place to avoid
 *  intermediate Map allocations during the walk. */
function collectTerminalSessions(
  node: PaneNodeSnapshot,
  workspaceId: string,
  out: Map<string, string>,
): void {
  if (node.kind === "terminal") {
    out.set(node.session_id, workspaceId);
    return;
  }
  if (node.kind === "split") {
    for (const child of node.children) {
      collectTerminalSessions(child, workspaceId, out);
    }
  }
  // browser / agent_chat panes have no session_id — skip.
}

/** Build a reverse index from `session_id` to the `workspace_id` that owns
 *  the terminal pane carrying that session.
 *
 *  Replaces the O(panes^2) `JSON.stringify(surf.root).includes(sid)` scan
 *  used by the terminal scrollback save / restore paths. With many
 *  workspaces, that pattern stringified every surface tree once per
 *  terminal mount and once per scrollback save — wasteful at scale. The
 *  index is built once per `AppStateSnapshot` reference (see
 *  `getSessionWorkspaceId` for caching) so repeated lookups are O(1).
 *
 *  Pure function of the snapshot — exported for unit testing.
 */
export function buildSessionWorkspaceIndex(
  appState: AppStateSnapshot | null,
): Map<string, string> {
  const index = new Map<string, string>();
  if (!appState) return index;
  for (const ws of appState.workspaces) {
    for (const surface of ws.surfaces) {
      collectTerminalSessions(surface.root, ws.workspace_id, index);
    }
  }
  return index;
}

// WeakMap caches the index per snapshot reference. The Rust-side state
// rebuilds the snapshot on every change, so a new reference always means a
// stale index — and a re-used reference means the index is still correct.
const sessionWorkspaceIndexCache = new WeakMap<
  AppStateSnapshot,
  Map<string, string>
>();

function getCachedSessionWorkspaceIndex(
  appState: AppStateSnapshot | null,
): Map<string, string> {
  if (!appState) return new Map();
  const cached = sessionWorkspaceIndexCache.get(appState);
  if (cached) return cached;
  const fresh = buildSessionWorkspaceIndex(appState);
  sessionWorkspaceIndexCache.set(appState, fresh);
  return fresh;
}

/** Imperative lookup: workspace id for a given terminal session, or null
 *  if no terminal pane currently owns it. Reads the cached index from
 *  `useAppStore.getState()` so callers inside effects / async IIFEs do not
 *  pay the build cost on every call. Returns null (not "") so call sites
 *  can apply their own fallback. */
export function getSessionWorkspaceId(sessionId: string): string | null {
  const { appState } = useAppStore.getState();
  const index = getCachedSessionWorkspaceIndex(appState);
  return index.get(sessionId) ?? null;
}

/** React hook variant — subscribes to `appState` and returns the cached
 *  index. Re-renders only when the snapshot reference changes (which is
 *  also when the index would actually differ). */
export function useSessionWorkspaceIndex(): Map<string, string> {
  return useAppStore((s) => getCachedSessionWorkspaceIndex(s.appState));
}

/** Find the workspace id that owns a given pane, if any.
 *
 *  Used by `AgentChatPane`'s draft-aware mount guard: when the pane's
 *  own `thread_id` field hasn't arrived yet (the Stage C race between
 *  `agent_chat_start_session`'s state emit and the pane mount), we
 *  need to know which workspace the pane belongs to so we can look up
 *  the promoted draft. Pure function of the snapshot — safe to call
 *  from inside a Zustand selector.
 */
export function findWorkspaceIdForPane(
  state: { appState: AppStateSnapshot | null },
  paneId: string,
): string | null {
  if (!state.appState) return null;
  for (const ws of state.appState.workspaces) {
    for (const surface of ws.surfaces) {
      if (paneTreeContains(surface.root, paneId)) {
        return ws.workspace_id;
      }
    }
  }
  return null;
}

export function useActiveSurface(): SurfaceSnapshot | null {
  return useAppStore((s) => {
    if (!s.appState) return null;
    const ws = s.appState.workspaces.find(
      (w) => w.workspace_id === s.appState!.active_workspace_id,
    );
    if (!ws) return null;
    return ws.surfaces.find((sf) => sf.surface_id === ws.active_surface_id) ?? null;
  });
}

// Project grouping — groups unsorted workspaces by their project root directory

export interface ProjectGroup {
  projectName: string;
  projectPath: string;
  workspaces: WorkspaceSnapshot[];
}

export function resolveProjectRoot(ws: WorkspaceSnapshot): string {
  return ws.project_root || ws.cwd;
}

/**
 * Pure grouping — exported so unit tests can exercise the logic
 * without rendering a hook. `useProjectGroupedWorkspaces` is a thin
 * `useMemo` wrapper.
 *
 * When `homeDir` is non-null, workspaces whose resolved project root
 * equals `homeDir` land in a dedicated group labelled "Home" (instead
 * of "zeus" or "home/zeus" from the path-basename rule). When
 * `homeDir` is null, grouping falls back to today's path-only rule.
 *
 * Under the Step 5 Home rework (Stage B+), `sidebar-workspace-list`
 * stops filtering out `workspace_type === "home"` workspaces, so this
 * label rule is what surfaces them correctly in the sidebar.
 */
export function groupWorkspacesByProject(
  workspaces: WorkspaceSnapshot[],
  homeDir: string | null,
): ProjectGroup[] {
  const groups = new Map<string, { name: string; path: string; workspaces: WorkspaceSnapshot[] }>();

  for (const ws of workspaces) {
    const projectPath = resolveProjectRoot(ws);
    const isHomeRooted = homeDir !== null && projectPath === homeDir;
    const projectName = isHomeRooted
      ? "Home"
      : projectPath.split("/").filter(Boolean).pop() || projectPath;

    if (!groups.has(projectPath)) {
      groups.set(projectPath, { name: projectName, path: projectPath, workspaces: [] });
    }
    groups.get(projectPath)!.workspaces.push(ws);
  }

  const result = Array.from(groups.values()).map((g) => ({
    projectName: g.name,
    projectPath: g.path,
    workspaces: g.workspaces,
  }));

  // Disambiguate duplicate project names by adding parent path. The
  // "Home" group is always unique (single homeDir path), so this
  // never rewrites it.
  const nameCounts = new Map<string, number>();
  for (const g of result) {
    nameCounts.set(g.projectName, (nameCounts.get(g.projectName) || 0) + 1);
  }
  for (const g of result) {
    if ((nameCounts.get(g.projectName) || 0) > 1) {
      const parts = g.projectPath.split("/").filter(Boolean);
      if (parts.length >= 2) {
        g.projectName = parts.slice(-2).join("/");
      }
    }
  }

  return result;
}

export function useProjectGroupedWorkspaces(
  workspaces: WorkspaceSnapshot[],
  homeDir: string | null,
): ProjectGroup[] {
  return useMemo(
    () => groupWorkspacesByProject(workspaces, homeDir),
    [workspaces, homeDir],
  );
}
