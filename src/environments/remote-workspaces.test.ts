import { describe, expect, it } from "vitest";

import type { AppStateSnapshot, WorkspaceSnapshot } from "@/tauri/types";

import { remoteWorkspaces } from "./remote-workspaces";
import type { EnvironmentStatus, EnvironmentView } from "./store";

function workspace(id: string, project: string, type = "standard"): WorkspaceSnapshot {
  return {
    workspace_id: id,
    title: id,
    workspace_type: type,
    cwd: `/home/dev/${project}`,
    project_root: `/home/dev/${project}`,
    surfaces: [{ surface_id: `${id}-s`, root: { kind: "agent_chat", pane_id: `${id}-p` } }],
  } as unknown as WorkspaceSnapshot;
}

function view(
  id: string,
  name: string,
  status: EnvironmentStatus,
  workspaces: WorkspaceSnapshot[] | null,
): EnvironmentView {
  return {
    device: { id, deviceId: id, nodeId: `node-${id}`, name, platform: "linux", lastSeenAt: null },
    status,
    appState: workspaces
      ? ({
          workspaces,
          pane_statuses: Object.fromEntries(workspaces.map((w) => [`${w.workspace_id}-p`, "working"])),
        } as unknown as AppStateSnapshot)
      : null,
    error: null,
  };
}

describe("remoteWorkspaces", () => {
  it("lists every machine's project workspaces, newest first, with machine-scoped ids", () => {
    const rows = remoteWorkspaces([
      view("env-b", "build-box", "connected", [workspace("w1", "api")]),
      view("env-a", "ai-node", "connected", [
        workspace("home", "home", "home"),
        workspace("w1", "codemux"),
        workspace("w2", "codemux"),
      ]),
    ]);
    expect(rows.map((r) => r.workspace.workspace_id)).toEqual([
      "env-a/w2",
      "env-a/w1",
      "env-b/w1",
    ]);
    expect(rows[0].repo).toEqual({ name: "codemux", path: "/home/dev/codemux" });
    expect(rows[0].machine).toEqual({ id: "env-a", name: "ai-node", live: true });
    expect(rows[0].status).toBe("working");
  });

  it("keeps an offline machine's last workspaces without a live status", () => {
    const [row] = remoteWorkspaces([view("env-s", "studio", "offline", [workspace("w1", "site")])]);
    expect(row.machine.live).toBe(false);
    expect(row.status).toBeNull();
  });

  it("skips machines that have not sent a snapshot", () => {
    expect(remoteWorkspaces([view("env-p", "pi", "pending-approval", null)])).toEqual([]);
  });

  it("keeps object identity for unchanged workspaces across updates", () => {
    const ws = workspace("w1", "codemux");
    const first = remoteWorkspaces([view("env-a", "ai-node", "connected", [ws])])[0];
    const second = remoteWorkspaces([view("env-a", "ai-node", "connected", [ws])])[0];
    expect(second.workspace).toBe(first.workspace);
    expect(second.repo).toBe(first.repo);
    expect(second.machine).toBe(first.machine);
  });
});
