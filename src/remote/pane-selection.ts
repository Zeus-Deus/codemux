import type {
  AppStateSnapshot,
  PaneNodeSnapshot,
  WorkspaceSnapshot,
} from "@/tauri/types";
import { remoteViewHost } from "./client-view";
import { isRemoteClient } from "@/components/remote/is-remote-client";
type Selection = { tab: string; pane?: string };
let selections: Record<string, Selection> = {};
let loadedHost = "";
function read() {
  const host = remoteViewHost();
  if (host !== loadedHost) {
    loadedHost = host;
    try {
      const saved: unknown = JSON.parse(
        localStorage.getItem(`codemux.remote.panes:${host}`) || "{}",
      );
      selections = Object.fromEntries(
        Object.entries(saved && typeof saved === "object" ? saved : {}).filter(
          (entry): entry is [string, Selection] => {
            const value = entry[1];
            return (
              value !== null &&
              typeof value === "object" &&
              typeof value.tab === "string" &&
              (value.pane === undefined || typeof value.pane === "string")
            );
          },
        ),
      );
    } catch {
      selections = {};
    }
  }
}
export function rememberRemotePane(
  workspace: string,
  tab: string,
  pane?: string,
) {
  read();
  selections[workspace] = { tab, pane };
  try {
    localStorage.setItem(
      `codemux.remote.panes:${remoteViewHost()}`,
      JSON.stringify(selections),
    );
  } catch {
    /* optional */
  }
}
export function containsPane(node: PaneNodeSnapshot, id: string): boolean {
  return (
    node.pane_id === id ||
    (node.kind === "split" && node.children.some((c) => containsPane(c, id)))
  );
}
export function projectWorkspace(
  workspace: WorkspaceSnapshot,
  selection?: Selection,
): WorkspaceSnapshot {
  if (!selection) return workspace;
  const tab = workspace.tabs.find((t) => t.tab_id === selection.tab);
  if (!tab) return workspace;
  const surface = workspace.surfaces.find(
    (s) => s.surface_id === tab.surface_id,
  );
  const pane =
    surface && selection.pane && containsPane(surface.root, selection.pane)
      ? selection.pane
      : surface?.active_pane_id;
  if (
    workspace.active_tab_id === tab.tab_id &&
    (!surface ||
      (workspace.active_surface_id === surface.surface_id &&
        pane === surface.active_pane_id))
  )
    return workspace;
  return {
    ...workspace,
    active_tab_id: tab.tab_id,
    active_surface_id: surface?.surface_id ?? workspace.active_surface_id,
    surfaces:
      surface && pane !== surface.active_pane_id
        ? workspace.surfaces.map((s) =>
            s === surface ? { ...s, active_pane_id: pane! } : s,
          )
        : workspace.surfaces,
  };
}
export function projectRemotePanes(
  snapshot: AppStateSnapshot,
): AppStateSnapshot {
  if (!isRemoteClient()) return snapshot;
  read();
  let changed = false;
  const workspaces = snapshot.workspaces.map((w) => {
    // Seed once so a later desktop selection cannot move this client.
    selections[w.workspace_id] ??= { tab: w.active_tab_id };
    const projected = projectWorkspace(w, selections[w.workspace_id]);
    const surface = projected.surfaces.find(
      (s) => s.surface_id === projected.active_surface_id,
    );
    selections[w.workspace_id] = {
      tab: projected.active_tab_id,
      pane: surface?.active_pane_id,
    };
    changed ||= projected !== w;
    return projected;
  });
  return changed ? { ...snapshot, workspaces } : snapshot;
}
