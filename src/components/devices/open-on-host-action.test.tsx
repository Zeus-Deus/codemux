/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

import type { HostView, WorkspaceSyncView } from "@/tauri/commands";

import { host, syncRow } from "./host-fixtures.test-utils";

// Spies + mutable state must be defined via vi.hoisted so the hoisted
// vi.mock factories below can reference them without a TDZ crash.
const h = vi.hoisted(() => ({
  hosts: [] as HostView[],
  rows: [] as WorkspaceSyncView[],
  setShowDevices: vi.fn(),
  workspaceOpenOnHost: vi.fn(async (_syncRowId: number) => ({
    workspace_id: "ws-new-local",
    remote_cwd: "/srv/agent/svc",
    message: "Opened on homelab — running in place on the host.",
  })),
  activateWorkspace: vi.fn(async (_id: string) => "ok"),
}));

vi.mock("@/stores/app-store", () => ({
  useAppStore: vi.fn((selector: (s: unknown) => unknown) =>
    selector({
      appState: { workspaces: [], active_workspace_id: null },
      workspacePushPullInFlight: null,
      workspacePushPullStartedAt: null,
      setWorkspacePushPullInFlight: vi.fn(),
    }),
  ),
}));
vi.mock("@/stores/hosts-store", () => ({
  useHosts: () => h.hosts,
  useHostsStore: vi.fn((selector: (s: unknown) => unknown) =>
    selector({ hosts: h.hosts, loaded: true, init: () => Promise.resolve() }),
  ),
}));
vi.mock("@/stores/host-status-store", () => ({
  useHostStatuses: () => ({}),
}));
vi.mock("@/stores/workspaces-sync-store", () => ({
  useWorkspacesSync: () => h.rows,
  useWorkspacesSyncStore: vi.fn((selector: (s: unknown) => unknown) =>
    selector({ rows: h.rows, refresh: () => Promise.resolve() }),
  ),
}));
vi.mock("@/stores/sidebar-inbox-store", () => ({
  useSidebarInboxStore: vi.fn((selector: (s: unknown) => unknown) =>
    selector({ settled: [], load: () => Promise.resolve() }),
  ),
}));
vi.mock("@/stores/ui-store", () => ({
  useUIStore: vi.fn((selector: (s: unknown) => unknown) =>
    selector({
      setShowDevices: h.setShowDevices,
      setShowSettings: vi.fn(),
    }),
  ),
}));
vi.mock("@/lib/toast", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));
vi.mock("./pull-to-device-dialog", () => ({ PullToDeviceDialog: () => null }));

// Spy on the two commands the "Open on host" path drives. Keep every
// other export real.
vi.mock("@/tauri/commands", async (importActual) => {
  const actual = await importActual<typeof import("@/tauri/commands")>();
  return {
    ...actual,
    workspaceOpenOnHost: h.workspaceOpenOnHost,
    activateWorkspace: h.activateWorkspace,
    workspacesWorktreeSizes: async () => ({}),
  };
});

const { workspaceOpenOnHost, activateWorkspace, setShowDevices } = h;

import { DevicesSection } from "./devices-section";

beforeEach(() => {
  h.hosts = [];
  h.rows = [
    syncRow({
      id: 99,
      server_id: "42",
      title: "remote-svc",
      host_server_id: "host-7",
      project_path: "/home/agent/svc",
      project_remote: null,
      git_branch: "feature/x",
      project_uid: "uid-svc",
      origin_path: "/srv/agent/svc",
    }),
  ];
  workspaceOpenOnHost.mockClear();
  activateWorkspace.mockClear();
  setShowDevices.mockClear();
});
afterEach(() => cleanup());

describe("Open on host action", () => {
  it("offers 'Open on <host>' and drives the no-pull command when the host is configured", async () => {
    // server id matches the sync row's host_server_id
    h.hosts = [host(5, "homelab", "host-7")];
    render(<DevicesSection />);

    // No status yet → the card starts folded; open it to reach the row.
    fireEvent.click(screen.getByRole("button", { name: "Expand homelab" }));
    fireEvent.click(screen.getByRole("button", { name: "Open on homelab" }));

    await waitFor(() => expect(workspaceOpenOnHost).toHaveBeenCalledWith(99));
    // It activates the freshly-created local attach-in-place workspace and
    // dismisses the page so the user lands on the running terminal.
    await waitFor(() =>
      expect(activateWorkspace).toHaveBeenCalledWith("ws-new-local"),
    );
    expect(setShowDevices).toHaveBeenCalledWith(false);
  });

  it("offers only 'Pull here' when the workspace's host is NOT configured on this device", () => {
    h.hosts = []; // host-7 unknown here → can't SSH → no in-place open
    render(<DevicesSection />);

    fireEvent.click(screen.getByRole("button", { name: "Expand Another device" }));
    expect(screen.getByRole("button", { name: "Pull here" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /open on/i })).toBeNull();
    expect(workspaceOpenOnHost).not.toHaveBeenCalled();
  });
});
