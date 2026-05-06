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
