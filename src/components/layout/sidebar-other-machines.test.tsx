import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render, within } from "@testing-library/react";

import { useEnvironmentsStore, type EnvironmentStatus } from "@/environments/store";
import type { AppStateSnapshot, PaneStatus, WorkspaceSnapshot } from "@/tauri/types";

import { SidebarOtherMachines } from "./sidebar-other-machines";

function workspace(id: string, title: string, type: "standard" | "home" = "standard"): WorkspaceSnapshot {
  return {
    workspace_id: id,
    title,
    workspace_type: type,
    cwd: `/home/user/Projects/${title}`,
    project_root: "/home/user/Projects/codemux",
    surfaces: [{ surface_id: `${id}-s`, root: { kind: "terminal", pane_id: `${id}-pane` } }],
  } as unknown as WorkspaceSnapshot;
}

function appState(workspaces: WorkspaceSnapshot[], statuses: Record<string, PaneStatus>): AppStateSnapshot {
  return { workspaces, pane_statuses: statuses } as unknown as AppStateSnapshot;
}

function machine(name: string, status: EnvironmentStatus, state: AppStateSnapshot | null) {
  const store = useEnvironmentsStore.getState();
  store.setCatalog([
    ...Object.values(store.environments).map((view) => view.device),
    { id: name, deviceId: name, nodeId: `node-${name}`, name, platform: "linux", lastSeenAt: null },
  ]);
  store.update(name, { status, appState: state });
}

beforeEach(() => useEnvironmentsStore.getState().reset());
afterEach(cleanup);

describe("SidebarOtherMachines", () => {
  it("renders nothing when the account has no other machines", () => {
    const view = render(<SidebarOtherMachines />);
    expect(view.container).toBeEmptyDOMElement();
  });

  it("lists each machine's project workspaces, most demanding first", () => {
    machine(
      "ai-node",
      "connected",
      appState(
        [workspace("home", "Home", "home"), workspace("a", "auth-refactor"), workspace("b", "new-chat"), workspace("c", "bench")],
        { "b-pane": "permission", "c-pane": "working" },
      ),
    );
    const view = render(<SidebarOtherMachines />);
    const section = view.getByRole("region", { name: "ai-node" });
    const rows = within(section).getAllByRole("listitem").map((row) => row.textContent);
    expect(rows).toEqual(["new-chatNeeds you", "benchWorking", "auth-refactorcodemux"]);
  });

  it("keeps an offline machine's last workspaces and says why it is dimmed", () => {
    machine("ai-node", "offline", appState([workspace("a", "auth-refactor")], {}));
    machine("laptop", "pending-approval", null);
    const view = render(<SidebarOtherMachines />);
    const offline = view.getByRole("region", { name: "ai-node" });
    expect(within(offline).getByText("Offline")).toBeInTheDocument();
    expect(within(offline).getByText("auth-refactor")).toBeInTheDocument();
    expect(view.getByText("Waiting for approval")).toHaveAttribute("title", "Approve this device in Codemux on laptop");
  });

  it("collapses a machine", () => {
    machine("ai-node", "connected", appState([workspace("a", "auth-refactor")], {}));
    const view = render(<SidebarOtherMachines />);
    fireEvent.click(view.getByRole("button", { expanded: true }));
    expect(view.queryByText("auth-refactor")).toBeNull();
  });
});
