/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { TooltipProvider } from "@/components/ui/tooltip";
import {
  host,
  status,
  syncRow,
} from "@/components/devices/host-fixtures.test-utils";
import type {
  HostStatusView,
  HostView,
  WorkspaceSyncView,
} from "@/tauri/commands";

const setShowDevicesMock = vi.fn();
let hosts: HostView[] = [];
let statuses: Record<number, HostStatusView> = {};
let rows: WorkspaceSyncView[] = [];
const initSyncMock = vi.fn(() => Promise.resolve());

vi.mock("@/stores/ui-store", () => ({
  useUIStore: vi.fn((selector) =>
    selector({ setShowDevices: setShowDevicesMock }),
  ),
}));
vi.mock("@/stores/hosts-store", () => ({ useHosts: () => hosts }));
vi.mock("@/stores/host-status-store", () => ({
  useHostStatuses: () => statuses,
}));
vi.mock("@/stores/workspaces-sync-store", () => ({
  useWorkspacesSyncStore: vi.fn((selector: (s: unknown) => unknown) =>
    selector({ rows, init: initSyncMock }),
  ),
}));

import { SidebarDevicesButton } from "./sidebar-devices-button";
import { useAppStore } from "@/stores/app-store";

afterEach(cleanup);

function renderButton() {
  return render(
    <TooltipProvider>
      <SidebarDevicesButton />
    </TooltipProvider>,
  );
}

describe("SidebarDevicesButton", () => {
  beforeEach(() => {
    hosts = [];
    statuses = {};
    rows = [];
    useAppStore.setState({ workspacePushPullError: null });
    setShowDevicesMock.mockClear();
    initSyncMock.mockClear();
  });

  it("renders nothing until a device is configured or work exists elsewhere", () => {
    renderButton();
    expect(screen.queryByTestId("sidebar-devices")).toBeNull();
  });

  it("appears for work on an unconfigured device, so the page stays reachable", () => {
    rows = [syncRow({ id: 1, host_server_id: "srv-ghost" })];
    renderButton();
    expect(screen.getByTestId("sidebar-devices")).toBeInTheDocument();
  });

  it("opens the Devices page and loads the sync snapshot once on mount", () => {
    hosts = [host(1, "zeus")];
    renderButton();

    fireEvent.click(screen.getByTestId("sidebar-devices"));
    expect(setShowDevicesMock).toHaveBeenCalledWith(true);
    expect(initSyncMock).toHaveBeenCalledTimes(1);
  });

  it("shows no dot while every device is offline", () => {
    hosts = [host(1, "zeus")];
    statuses = { 1: status(1) };
    renderButton();

    expect(screen.getByTestId("sidebar-devices")).toBeInTheDocument();
    expect(screen.queryByTestId("sidebar-devices-dot")).toBeNull();
  });

  it("shows a green dot and counts online devices in the tooltip", async () => {
    hosts = [host(1, "zeus"), host(2, "pandora")];
    statuses = { 1: status(1, { reachable: true }), 2: status(2, { reachable: true }) };
    renderButton();

    expect(screen.getByTestId("sidebar-devices-dot")).toHaveAttribute(
      "data-tone",
      "green",
    );
    await userEvent.hover(screen.getByTestId("sidebar-devices"));
    expect(await screen.findByRole("tooltip")).toHaveTextContent(
      "Devices — 2 online",
    );
  });

  it("turns amber and names the branch and device when a branch diverged", async () => {
    hosts = [host(1, "zeus")];
    statuses = { 1: status(1, { reachable: true }) };
    rows = [
      syncRow({
        id: 10,
        git_branch: "bypass-share-limit-owner",
        git_head_sha: "aaa",
        host_server_id: null,
      }),
      syncRow({
        id: 11,
        git_branch: "bypass-share-limit-owner",
        git_head_sha: "bbb",
        host_server_id: "srv-zeus",
      }),
    ];
    renderButton();

    expect(screen.getByTestId("sidebar-devices-dot")).toHaveAttribute(
      "data-tone",
      "amber",
    );
    await userEvent.hover(screen.getByTestId("sidebar-devices"));
    expect(await screen.findByRole("tooltip")).toHaveTextContent(
      "Devices — bypass-share-limit-owner diverged on zeus",
    );
  });

  it("turns amber for a degraded device and repeats its note", async () => {
    hosts = [host(3, "nas")];
    statuses = {
      3: status(3, { reachable: true, last_error: "codemux-remote is not installed" }),
    };
    renderButton();

    expect(screen.getByTestId("sidebar-devices-dot")).toHaveAttribute(
      "data-tone",
      "amber",
    );
    await userEvent.hover(screen.getByTestId("sidebar-devices"));
    expect(await screen.findByRole("tooltip")).toHaveTextContent(
      "Devices — nas: codemux-remote is not installed",
    );
  });

  it("turns amber for a failed transfer recorded in the app store", async () => {
    hosts = [host(1, "zeus")];
    statuses = { 1: status(1, { reachable: true }) };
    useAppStore.getState().setWorkspacePushPullError("Pull failed: partpilot");
    renderButton();

    expect(screen.getByTestId("sidebar-devices-dot")).toHaveAttribute(
      "data-tone",
      "amber",
    );
    await userEvent.hover(screen.getByTestId("sidebar-devices"));
    expect(await screen.findByRole("tooltip")).toHaveTextContent(
      "Devices — Pull failed: partpilot",
    );
  });
});
