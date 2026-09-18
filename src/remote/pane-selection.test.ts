import { describe, expect, it, vi } from "vitest";
vi.mock("@/components/remote/is-remote-client", () => ({
  isRemoteClient: () => true,
}));
import { setRemoteViewHost } from "./client-view";
import {
  projectWorkspace,
  containsPane,
  projectRemotePanes,
} from "./pane-selection";
import type { WorkspaceSnapshot, AppStateSnapshot } from "@/tauri/types";
const workspace = {
  workspace_id: "w",
  active_tab_id: "desktop",
  active_surface_id: "ds",
  tabs: [
    { tab_id: "desktop", surface_id: "ds" },
    { tab_id: "phone", surface_id: "ps" },
  ],
  surfaces: [
    {
      surface_id: "ds",
      active_pane_id: "d",
      root: { kind: "terminal", pane_id: "d" },
    },
    {
      surface_id: "ps",
      active_pane_id: "a",
      root: {
        kind: "split",
        pane_id: "s",
        children: [
          { kind: "terminal", pane_id: "a" },
          { kind: "agent_chat", pane_id: "b" },
        ],
      },
    },
  ],
} as WorkspaceSnapshot;
describe("remote pane selection", () => {
  it("selects a phone tab and split pane without mutating the shared snapshot", () => {
    const next = projectWorkspace(workspace, { tab: "phone", pane: "b" });
    expect(next.active_tab_id).toBe("phone");
    expect(next.active_surface_id).toBe("ps");
    expect(next.surfaces[1].active_pane_id).toBe("b");
    expect(workspace.active_tab_id).toBe("desktop");
    expect(workspace.surfaces[1].active_pane_id).toBe("a");
  });
  it("falls back safely when a tab or pane was closed on another device", () => {
    expect(projectWorkspace(workspace, { tab: "gone" })).toBe(workspace);
    expect(
      projectWorkspace(workspace, { tab: "phone", pane: "gone" }).surfaces[1]
        .active_pane_id,
    ).toBe("a");
  });
  it("preserves object identity when the selected view already matches", () => {
    expect(projectWorkspace(workspace, { tab: "desktop" })).toBe(workspace);
  });
  it("finds leaves inside splits", () => {
    expect(containsPane(workspace.surfaces[1].root, "b")).toBe(true);
    expect(containsPane(workspace.surfaces[1].root, "x")).toBe(false);
  });
});

it("pins the initial pane against subsequent desktop selection changes", () => {
  setRemoteViewHost("selection-isolation-test");
  const initial = { workspaces: [workspace] } as AppStateSnapshot;
  projectRemotePanes(initial);
  const changed = {
    ...workspace,
    active_tab_id: "phone",
    active_surface_id: "ps",
  };
  const projected = projectRemotePanes({
    workspaces: [changed],
  } as AppStateSnapshot);
  expect(projected.workspaces[0].active_tab_id).toBe("desktop");
  expect(projected.workspaces[0].active_surface_id).toBe("ds");
});
it("ignores malformed saved selections instead of breaking startup", () => {
  setRemoteViewHost("invalid-selection-test");
  localStorage.setItem("codemux.remote.panes:invalid-selection-test", "null");
  expect(() =>
    projectRemotePanes({ workspaces: [workspace] } as AppStateSnapshot),
  ).not.toThrow();
});
