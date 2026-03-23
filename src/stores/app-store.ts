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
