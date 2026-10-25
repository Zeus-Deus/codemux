/**
 * Every project Codemux knows, the way plugin services see them: one entry
 * per project id across open and archived workspaces, the same registry the
 * host checks grants and tree references against. Derived from app state the
 * client already holds, so listing them costs no request and is never cut to
 * a "recent" few.
 */
import { useMemo } from "react";
import { projectDisplayName, useAppStore, useHomeDir } from "@/stores/app-store";
import type { ArchivedWorkspaceSnapshot, WorkspaceSnapshot } from "@/tauri/types";
import { pluginProjectIdOf } from "./workspace-context";

export interface PluginKnownProject {
  /** The plugin project id (`project_uid`, else the project root, else the folder). */
  readonly id: string;
  readonly title: string;
  /** The project root the sidebar groups by. */
  readonly root: string;
  /** No open workspace holds it; only archived ones do. */
  readonly archivedOnly: boolean;
}

type ProjectFields = Pick<WorkspaceSnapshot, "project_uid" | "project_root" | "cwd">;

export function pluginKnownProjects(input: {
  readonly workspaces: ReadonlyArray<ProjectFields>;
  readonly archived: ReadonlyArray<Pick<ArchivedWorkspaceSnapshot, "project_uid" | "project_root" | "cwd">>;
  readonly homeDir: string | null;
}): ReadonlyArray<PluginKnownProject> {
  const byId = new Map<string, PluginKnownProject>();
  const add = (workspace: ProjectFields, archivedOnly: boolean) => {
    const id = pluginProjectIdOf(workspace);
    if (id === "" || byId.has(id)) return;
    // The sidebar's grouping root (`resolveProjectRoot`).
    const root = workspace.project_root || workspace.cwd;
    byId.set(id, { id, title: projectDisplayName(root, input.homeDir), root, archivedOnly });
  };
  for (const workspace of input.workspaces) add(workspace, false);
  for (const workspace of input.archived) add(workspace, true);
  return [...byId.values()].sort((a, b) => a.title.localeCompare(b.title) || a.root.localeCompare(b.root));
}

/** Known projects, recomputed only when the workspace lists change. */
export function usePluginKnownProjects(): ReadonlyArray<PluginKnownProject> {
  const workspaces = useAppStore((state) => state.appState?.workspaces);
  const archived = useAppStore((state) => state.appState?.archived_workspaces);
  const homeDir = useHomeDir();
  return useMemo(
    () => pluginKnownProjects({ workspaces: workspaces ?? [], archived: archived ?? [], homeDir }),
    [workspaces, archived, homeDir],
  );
}
