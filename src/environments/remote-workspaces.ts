import { useMemo } from "react";

import { getWorkspaceStatus } from "@/lib/pane-status";
import type { ActivePaneStatus, WorkspaceSnapshot } from "@/tauri/types";

import { useEnvironmentsStore, type EnvironmentView } from "./store";

/** A workspace that lives on another of the account's machines, shaped so the
 *  sidebar can render it with the same card as a local one. */
export interface RemoteWorkspace {
  /** The machine's snapshot with `workspace_id` scoped as
   *  `<deviceId>/<workspace_id>`: ids are only unique per machine, and every
   *  per-workspace store in the sidebar is keyed by that id. */
  workspace: WorkspaceSnapshot;
  /** `live` is false while the machine is unreachable: the card shows its
   *  last known workspace without claiming a live agent state. */
  machine: RemoteMachine;
  status: ActivePaneStatus | null;
  repo: { name: string; path: string };
}

export interface RemoteMachine {
  id: string;
  name: string;
  live: boolean;
}

// Scoped copies keyed by the source snapshot object, so a workspace that did
// not change between two updates keeps its identity and its card's memo holds.
const scopedCache = new WeakMap<WorkspaceSnapshot, WorkspaceSnapshot>();

function scoped(machineId: string, workspace: WorkspaceSnapshot): WorkspaceSnapshot {
  let copy = scopedCache.get(workspace);
  if (!copy) {
    copy = { ...workspace, workspace_id: `${machineId}/${workspace.workspace_id}` };
    scopedCache.set(workspace, copy);
  }
  return copy;
}

const machineCache = new Map<string, RemoteMachine>();

function machineOf(view: EnvironmentView): RemoteMachine {
  const live = view.status === "connected";
  const key = `${view.device.deviceId}\n${view.device.name}\n${live}`;
  let machine = machineCache.get(key);
  if (!machine) {
    machine = { id: view.device.deviceId, name: view.device.name, live };
    machineCache.set(key, machine);
  }
  return machine;
}

// Interned for the same reason: a fresh repo object per update would also
// defeat the card's memo.
const repoCache = new Map<string, { name: string; path: string }>();

function projectOf(workspace: WorkspaceSnapshot): { name: string; path: string } {
  const path = workspace.project_root ?? workspace.cwd;
  let repo = repoCache.get(path);
  if (!repo) {
    repo = { name: path.split("/").filter(Boolean).pop() ?? workspace.title, path };
    repoCache.set(path, repo);
  }
  return repo;
}

/** Every project workspace of the other machines, as one flat list. Within a
 *  machine the newest workspace comes first (the backend appends), matching
 *  the local inbox order. A machine without a snapshot yet (connecting, or
 *  waiting for approval) contributes nothing. */
export function remoteWorkspaces(views: EnvironmentView[]): RemoteWorkspace[] {
  const rows: RemoteWorkspace[] = [];
  const sorted = [...views].sort((a, b) => a.device.name.localeCompare(b.device.name));
  for (const view of sorted) {
    const state = view.appState;
    if (!state) continue;
    const machine = machineOf(view);
    const statuses = state.pane_statuses ?? {};
    for (let i = state.workspaces.length - 1; i >= 0; i--) {
      const workspace = state.workspaces[i];
      if (workspace.workspace_type === "home") continue;
      rows.push({
        workspace: scoped(view.device.deviceId, workspace),
        machine,
        status: machine.live ? getWorkspaceStatus(workspace.surfaces, statuses) : null,
        repo: projectOf(workspace),
      });
    }
  }
  return rows;
}

export function useRemoteWorkspaces(): RemoteWorkspace[] {
  const environments = useEnvironmentsStore((s) => s.environments);
  return useMemo(() => remoteWorkspaces(Object.values(environments)), [environments]);
}
