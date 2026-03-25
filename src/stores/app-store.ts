import { useMemo } from "react";
import { create } from "zustand";
import type {
  AppStateSnapshot,
  WorkspaceSnapshot,
  WorkspaceSectionSnapshot,
  SurfaceSnapshot,
} from "@/tauri/types";

interface AppStore {
  appState: AppStateSnapshot | null;
  setAppState: (snapshot: AppStateSnapshot) => void;
}

export const useAppStore = create<AppStore>((set) => ({
  appState: null,
  setAppState: (snapshot) => set({ appState: snapshot }),
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

export interface SectionGroup {
  section: WorkspaceSectionSnapshot;
  workspaces: WorkspaceSnapshot[];
}

export function useSectionedWorkspaces() {
  const appState = useAppStore((s) => s.appState);

  return useMemo(() => {
    if (!appState)
      return { sectionGroups: [] as SectionGroup[], unsorted: [] as WorkspaceSnapshot[] };

    const sections = [...appState.sections].sort((a, b) => a.position - b.position);
    const assignedIds = new Set(sections.flatMap((sec) => sec.workspace_ids));
    const unsorted = appState.workspaces.filter(
      (w) => !assignedIds.has(w.workspace_id),
    );
    const sectionGroups: SectionGroup[] = sections.map((section) => ({
      section,
      workspaces: section.workspace_ids
        .map((id) => appState.workspaces.find((w) => w.workspace_id === id))
        .filter((w): w is WorkspaceSnapshot => w != null),
    }));

    return { sectionGroups, unsorted };
  }, [appState]);
}

// Project grouping — groups unsorted workspaces by their project root directory

export interface ProjectGroup {
  projectName: string;
  projectPath: string;
  workspaces: WorkspaceSnapshot[];
}

export function resolveProjectRoot(ws: WorkspaceSnapshot): string {
  // Prefer worktree_path field (canonical source) over cwd string parsing
  const pathToCheck = ws.worktree_path || ws.cwd;
  const worktreeMarker = "/.codemux/worktrees/";
  const wtIdx = pathToCheck.indexOf(worktreeMarker);
  if (wtIdx >= 0) {
    const afterMarker = pathToCheck.substring(wtIdx + worktreeMarker.length);
    const repoName = afterMarker.split("/")[0];
    if (repoName) {
      return pathToCheck.substring(0, wtIdx + worktreeMarker.length + repoName.length);
    }
  }
  return ws.cwd;
}

export function useProjectGroupedWorkspaces(workspaces: WorkspaceSnapshot[]): ProjectGroup[] {
  return useMemo(() => {
    const groups = new Map<string, { name: string; path: string; workspaces: WorkspaceSnapshot[] }>();

    for (const ws of workspaces) {
      const projectPath = resolveProjectRoot(ws);
      const projectName = projectPath.split("/").filter(Boolean).pop() || projectPath;

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

    // Disambiguate duplicate project names by adding parent path
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
  }, [workspaces]);
}
