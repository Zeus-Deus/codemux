/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type { HostView, WorkspaceSyncView } from "@/tauri/commands";
import type { OverviewItem } from "./use-overview-items";

// Spies + mutable state must be defined via vi.hoisted so the hoisted
// vi.mock factories below can reference them without a TDZ crash.
const h = vi.hoisted(() => ({
  hosts: [] as HostView[],
  setShowWorkspacesOverview: vi.fn(),
  workspaceOpenOnHost: vi.fn(async (_syncRowId: number) => ({
    workspace_id: "ws-new-local",
    remote_cwd: "/srv/agent/svc",
    message: "Opened on homelab — running in place on the host.",
  })),
  activateWorkspace: vi.fn(async (_id: string) => "ok"),
}));

vi.mock("@/stores/hosts-store", () => ({
  useHostsStore: vi.fn((selector: (s: unknown) => unknown) =>
    selector({ hosts: h.hosts, loaded: true, init: () => Promise.resolve() }),
  ),
  useHosts: () => h.hosts,
}));
vi.mock("@/stores/app-store", () => ({
  useAppStore: vi.fn((selector: (s: unknown) => unknown) =>
    selector({
      appState: null,
      workspacePushPullInFlight: null,
      setWorkspacePushPullInFlight: vi.fn(),
    }),
  ),
}));
vi.mock("@/stores/ui-store", () => ({
  useUIStore: vi.fn((selector: (s: unknown) => unknown) =>
    selector({ setShowWorkspacesOverview: h.setShowWorkspacesOverview }),
  ),
}));
vi.mock("@/lib/pane-status", () => ({ getWorkspaceStatus: () => null }));
vi.mock("@/lib/toast", () => ({
  toast: { success: vi.fn(), error: vi.fn() },
}));

// Spy on the two commands the "Open on host" path drives. Keep every
// other export real (the row imports a handful at module scope).
vi.mock("@/tauri/commands", async (importActual) => {
  const actual = await importActual<typeof import("@/tauri/commands")>();
  return {
    ...actual,
    workspaceOpenOnHost: h.workspaceOpenOnHost,
    activateWorkspace: h.activateWorkspace,
  };
});

const { workspaceOpenOnHost, activateWorkspace, setShowWorkspacesOverview } = h;

import { WorkspaceOverviewRow } from "./workspace-overview-row";

function makeSyncRow(overrides?: Partial<WorkspaceSyncView>): WorkspaceSyncView {
  return {
    id: 99,
    server_id: "42",
    workspace_id: null,
    title: "remote-svc",
    host_server_id: "host-7",
    project_path: "/home/agent/svc",
    project_remote: null,
    git_branch: "feature/x",
    git_head_sha: null,
    project_uid: "uid-svc",
    workspace_kind: "worktree",
    default_branch: null,
    origin_path: "/srv/agent/svc",
    created_at: "2026-06-01T00:00:00Z",
    updated_at: "2026-06-01T00:00:00Z",
    dirty: false,
    ...overrides,
  };
}

function remoteItem(overrides?: Partial<WorkspaceSyncView>): OverviewItem {
  const sync = makeSyncRow(overrides);
  return {
    kind: "remote",
    key: `remote:${sync.server_id ?? sync.id}`,
    sync,
    hostServerId: sync.host_server_id,
    projectName: "svc",
    projectPath: sync.project_path,
    projectKey: sync.project_uid ?? sync.project_path,
  };
}

function configuredHost(): HostView {
  return {
    id: 5,
    server_id: "host-7", // matches the sync row's host_server_id
    name: "homelab",
    ssh_target: "me@homelab",
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    dirty: false,
  };
}

beforeEach(() => {
  h.hosts = [];
  workspaceOpenOnHost.mockClear();
  activateWorkspace.mockClear();
  setShowWorkspacesOverview.mockClear();
});
afterEach(() => cleanup());

describe("Open on host action", () => {
  it("offers 'Open on host' and drives the no-pull command when the host is configured", async () => {
    h.hosts = [configuredHost()];
    const user = userEvent.setup();
    const onAfterOpen = vi.fn();

    render(
      <WorkspaceOverviewRow
        item={remoteItem()}
        isAttached={false}
        onAfterOpen={onAfterOpen}
      />,
    );

    await user.click(screen.getByRole("button", { name: /workspace actions/i }));
    const openItem = await screen.findByText("Open on host");
    await user.click(openItem);

    await waitFor(() =>
      expect(workspaceOpenOnHost).toHaveBeenCalledWith(99),
    );
    // It activates the freshly-created local attach-in-place workspace and
    // dismisses the overview so the user lands on the running terminal.
    await waitFor(() =>
      expect(activateWorkspace).toHaveBeenCalledWith("ws-new-local"),
    );
    expect(setShowWorkspacesOverview).toHaveBeenCalledWith(false);
  });

  it("hides 'Open on host' when the workspace's host is NOT configured on this device", async () => {
    h.hosts = []; // host-7 unknown here → can't SSH → no in-place open
    const user = userEvent.setup();

    render(
      <WorkspaceOverviewRow
        item={remoteItem()}
        isAttached={false}
        onAfterOpen={() => {}}
      />,
    );

    await user.click(screen.getByRole("button", { name: /workspace actions/i }));
    // The menu opens (Pull-to-device entry is present) but Open-on-host is not.
    await screen.findByText(/pull to this device/i);
    expect(screen.queryByText("Open on host")).toBeNull();
    expect(workspaceOpenOnHost).not.toHaveBeenCalled();
  });
});
