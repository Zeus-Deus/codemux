/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

import type {
  HostStatusView,
  HostView,
  WorkspaceSyncView,
} from "@/tauri/commands";
import type { WorkspaceSnapshot } from "@/tauri/types";

import { host, status, syncRow as row, workspace } from "./host-fixtures.test-utils";

const h = vi.hoisted(() => ({
  workspaces: [] as WorkspaceSnapshot[],
  hosts: [] as HostView[],
  statuses: {} as Record<number, HostStatusView>,
  rows: [] as WorkspaceSyncView[],
  settled: [] as { id: string; at: number }[],
  setShowDevices: vi.fn(),
  setShowSettings: vi.fn(),
  setTransferError: vi.fn(),
  openUrl: vi.fn(async () => {}),
  workspacesWorktreeSizes: vi.fn(
    async (_ids: string[]) => ({}) as Record<string, number | null>,
  ),
}));

vi.mock("@/stores/app-store", () => ({
  useAppStore: vi.fn((selector: (s: unknown) => unknown) =>
    selector({
      appState: { workspaces: h.workspaces, active_workspace_id: null },
      workspacePushPullInFlight: null,
      workspacePushPullStartedAt: null,
      setWorkspacePushPullInFlight: vi.fn(),
      setWorkspacePushPullError: h.setTransferError,
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
  useHostStatuses: () => h.statuses,
}));
vi.mock("@/stores/workspaces-sync-store", () => ({
  useWorkspacesSync: () => h.rows,
  useWorkspacesSyncStore: vi.fn((selector: (s: unknown) => unknown) =>
    selector({ rows: h.rows, refresh: () => Promise.resolve() }),
  ),
}));
vi.mock("@/stores/sidebar-inbox-store", () => ({
  useSidebarInboxStore: vi.fn((selector: (s: unknown) => unknown) =>
    selector({ settled: h.settled, load: () => Promise.resolve() }),
  ),
}));
vi.mock("@/stores/ui-store", () => ({
  useUIStore: vi.fn((selector: (s: unknown) => unknown) =>
    selector({
      setShowDevices: h.setShowDevices,
      setShowSettings: h.setShowSettings,
    }),
  ),
}));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: h.openUrl }));
vi.mock("@/lib/toast", () => ({
  toast: { success: vi.fn(), error: vi.fn(), warning: vi.fn() },
}));
vi.mock("@/tauri/commands", async (importActual) => {
  const actual = await importActual<typeof import("@/tauri/commands")>();
  return { ...actual, workspacesWorktreeSizes: h.workspacesWorktreeSizes };
});
// The pull dialog has its own tests; keep it out of the section's DOM.
vi.mock("./pull-to-device-dialog", () => ({ PullToDeviceDialog: () => null }));

import { DevicesSection } from "./devices-section";
import { __resetWorktreeSizesForTests } from "./use-sweep-candidates";

const GB = 1024 ** 3;

beforeEach(() => {
  h.workspaces = [];
  h.hosts = [];
  h.statuses = {};
  h.rows = [];
  h.settled = [];
  h.setShowDevices.mockClear();
  h.setShowSettings.mockClear();
  h.openUrl.mockClear();
  h.workspacesWorktreeSizes.mockClear();
  h.workspacesWorktreeSizes.mockResolvedValue({});
  __resetWorktreeSizesForTests();
});
afterEach(() => cleanup());

describe("DevicesSection", () => {
  it("renders an online device card with the Remote Control pill and a Connect button", () => {
    h.hosts = [host(2, "zeus")];
    h.statuses = {
      2: status(2, { reachable: true, remote_control_serving: true, disk_bytes: 4.1 * GB }),
    };
    h.rows = [
      row({ id: 1, title: "passpage", workspace_kind: "main" }),
      row({ id: 2, title: "passpage-ui-polish", git_branch: "ui-polish-v1" }),
    ];
    render(<DevicesSection />);

    expect(screen.getByText("zeus")).toBeInTheDocument();
    expect(screen.getByText("online")).toBeInTheDocument();
    expect(screen.getByText("Remote Control serving")).toBeInTheDocument();
    expect(screen.getByText("deus@zeus · 2 workspaces · 1 project · 4.10 GB")).toBeInTheDocument();
    // Online cards start expanded: rows and the kind pill are visible.
    expect(screen.getByText("passpage-ui-polish")).toBeInTheDocument();
    expect(screen.getByText("repo root")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Connect" }));
    expect(h.openUrl).toHaveBeenCalledWith("https://app.codemux.org");
  });

  it("collapses an offline card to its header and expands it on click", () => {
    h.hosts = [host(1, "pandora")];
    h.statuses = { 1: status(1, { last_seen_at: new Date(Date.now() - 2 * 86_400_000).toISOString() }) };
    h.rows = [row({ id: 5, host_server_id: "srv-pandora", title: "partpilot" })];
    render(<DevicesSection />);

    expect(screen.getByText("unreachable · last seen 2d ago")).toBeInTheDocument();
    expect(screen.queryByText("partpilot")).toBeNull();
    expect(screen.queryByRole("button", { name: "Connect" })).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Expand pandora" }));
    expect(screen.getByText("partpilot")).toBeInTheDocument();
  });

  it("flags a reachable but degraded device and shows the poller's note", () => {
    h.hosts = [host(3, "nas")];
    h.statuses = {
      3: status(3, { reachable: true, last_error: "codemux-remote is not installed on this host" }),
    };
    h.rows = [row({ id: 7, host_server_id: "srv-nas", title: "media" })];
    render(<DevicesSection />);

    expect(screen.getByText("needs attention")).toBeInTheDocument();
    expect(screen.getByText("codemux-remote is not installed on this host")).toBeInTheDocument();
    // Still reachable, so it starts expanded like an online card.
    expect(screen.getByText("media")).toBeInTheDocument();
  });

  it("marks a diverged branch and names the other device in the tooltip", () => {
    h.hosts = [host(2, "zeus")];
    h.statuses = { 2: status(2, { reachable: true }) };
    h.rows = [
      row({ id: 1, title: "remote-copy", git_branch: "feat", git_head_sha: "aaa" }),
      row({ id: 2, workspace_id: "local-1", host_server_id: null, git_branch: "feat", git_head_sha: "bbb" }),
    ];
    render(<DevicesSection />);
    const chip = screen.getByText("diverged");
    expect(chip).toHaveAttribute("title", "Same branch has different commits on this device");
  });

  it("shows an unprobed host as still checking", () => {
    h.hosts = [host(9, "fresh", null)];
    render(<DevicesSection />);
    expect(screen.getByText("fresh")).toBeInTheDocument();
    expect(screen.getByText("deus@fresh · not synced yet")).toBeInTheDocument();
    expect(screen.getByText("checking…")).toBeInTheDocument();
  });

  it("routes Add device to the hosts settings section", () => {
    render(<DevicesSection />);
    fireEvent.click(screen.getByRole("button", { name: /add device/i }));
    expect(h.setShowDevices).toHaveBeenCalledWith(false);
    expect(h.setShowSettings).toHaveBeenCalledWith(true, "hosts");
  });

  it("shows the empty state when no device is configured", () => {
    h.workspaces = [workspace({ workspace_id: "a" }), workspace({ workspace_id: "b" })];
    render(<DevicesSection />);
    expect(screen.getByText("No devices yet")).toBeInTheDocument();
    expect(screen.getByText("2 workspaces · managed in the sidebar")).toBeInTheDocument();
  });

  it("offers to sweep exactly the settled worktrees the backend qualifies", async () => {
    h.workspaces = [
      workspace({ workspace_id: "wt-1" }),
      workspace({ workspace_id: "wt-2" }),
      workspace({ workspace_id: "root" }),
    ];
    const { unmount } = render(<DevicesSection />);
    expect(screen.queryByText(/sweep/i)).toBeNull();
    expect(h.workspacesWorktreeSizes).not.toHaveBeenCalled();
    unmount();

    // Three settled ids go to the backend; it answers for the two
    // worktrees (one unmeasured) and omits the protected root.
    h.settled = [
      { id: "wt-1", at: 1 },
      { id: "wt-2", at: 2 },
      { id: "root", at: 3 },
    ];
    h.workspacesWorktreeSizes.mockResolvedValue({ "wt-1": 1.5 * GB, "wt-2": null });
    render(<DevicesSection />);
    expect(h.workspacesWorktreeSizes).toHaveBeenCalledWith(["wt-1", "wt-2", "root"]);
    const chip = await screen.findByRole("button", { name: /sweep 2 settled/i });
    expect(chip).toHaveTextContent("~1.50 GB");
  });
});
