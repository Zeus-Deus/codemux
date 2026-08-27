/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import userEvent from "@testing-library/user-event";
import { TooltipProvider } from "@/components/ui/tooltip";
import { SidebarProvider } from "@/components/ui/sidebar";
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

// shadcn SidebarProvider uses useIsMobile → window.matchMedia, which
// jsdom doesn't implement. Stub a minimal shape so the effect runs.
if (typeof window !== "undefined" && !window.matchMedia) {
  Object.defineProperty(window, "matchMedia", {
    writable: true,
    value: (query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    }),
  });
}

const setShowSettingsMock = vi.fn();
const toggleCommandPaletteMock = vi.fn();
const setShowAutomationsMock = vi.fn();
const setShowDevicesMock = vi.fn();

// The Devices button reads three stores; each is a plain variable so a
// test can set up "no devices", "one online", or "a diverged branch".
let hosts: HostView[] = [];
let statuses: Record<number, HostStatusView> = {};
let syncRows: WorkspaceSyncView[] = [];

vi.mock("@/stores/hosts-store", () => ({ useHosts: () => hosts }));
vi.mock("@/stores/host-status-store", () => ({
  useHostStatuses: () => statuses,
}));
vi.mock("@/stores/workspaces-sync-store", () => ({
  useWorkspacesSyncStore: vi.fn((selector: (s: unknown) => unknown) =>
    selector({ rows: syncRows, init: () => Promise.resolve() }),
  ),
}));

vi.mock("@/stores/ui-store", () => ({
  useUIStore: vi.fn((selector) => {
    const state = {
      setShowSettings: setShowSettingsMock,
      toggleCommandPalette: toggleCommandPaletteMock,
      setShowAutomations: setShowAutomationsMock,
      setShowDevices: setShowDevicesMock,
    };
    return selector(state);
  }),
}));

// The ports popover pulls in the app store + tauri commands; a stub keeps
// this suite focused on the footer chrome while preserving DOM order.
vi.mock("./sidebar-ports-popover", () => ({
  SidebarPortsPopover: () => <button type="button" aria-label="Ports" />,
}));

vi.mock("@tauri-apps/api/app", () => ({
  getVersion: () => Promise.resolve("0.14.3"),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: vi.fn(),
}));

vi.mock("@/hooks/use-resolved-keybinds", () => ({
  useResolvedKeybinds: () => ({ getKeysForAction: () => null }),
}));

import { SidebarFooterBar } from "./sidebar-footer-bar";
import {
  useUpdateStatusStore,
  __resetUpdateStatusStoreForTests,
} from "@/stores/update-status-store";

// This project runs Vitest without `globals`, so RTL never registers its own
// auto-cleanup. The dropdown renders into a portal on document.body, so
// unmounting between tests is what keeps one test's open menu out of the next
// one's `document.querySelector('[role="menu"]')`.
afterEach(cleanup);

function renderFooter(open: boolean) {
  // The Pull Requests entry carries a live count, so the footer now sits
  // inside the app's query client the same way it does in the shell.
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, refetchInterval: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <TooltipProvider>
        <SidebarProvider defaultOpen={open}>
          <SidebarFooterBar />
        </SidebarProvider>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

describe("SidebarFooterBar — expanded", () => {
  beforeEach(() => {
    hosts = [];
    statuses = {};
    syncRows = [];
    setShowAutomationsMock.mockClear();
    setShowDevicesMock.mockClear();
  });

  it("renders a labeled Automations button and no Workspaces button", () => {
    const { container } = renderFooter(true);

    const automations = container.querySelector(
      'button[aria-label="Automations"]',
    ) as HTMLElement;
    expect(automations).toHaveTextContent("Automations");
    expect(container.querySelector('button[aria-label="Workspaces"]')).toBeNull();

    fireEvent.click(automations);
    expect(setShowAutomationsMock).toHaveBeenCalledWith(true);
  });

  it("hides the Devices button until a device is configured", () => {
    const { container } = renderFooter(true);
    expect(container.querySelector('[data-testid="sidebar-devices"]')).toBeNull();
  });

  it("shows Devices with a green dot when a device is reachable, and opens the page", () => {
    hosts = [host(1, "zeus")];
    statuses = { 1: status(1, { reachable: true }) };
    const { container } = renderFooter(true);

    const devices = container.querySelector(
      '[data-testid="sidebar-devices"]',
    ) as HTMLElement;
    expect(devices).toHaveAttribute("aria-label", "Devices");
    expect(
      container.querySelector('[data-testid="sidebar-devices-dot"]'),
    ).toHaveAttribute("data-tone", "green");

    fireEvent.click(devices);
    expect(setShowDevicesMock).toHaveBeenCalledWith(true);
  });

  it("turns the Devices dot amber when a branch has diverged across devices", () => {
    hosts = [host(1, "zeus")];
    statuses = { 1: status(1, { reachable: true }) };
    syncRows = [
      syncRow({ id: 1, git_branch: "feat", git_head_sha: "aaa" }),
      syncRow({
        id: 2,
        git_branch: "feat",
        git_head_sha: "bbb",
        host_server_id: "srv-zeus",
      }),
    ];
    const { container } = renderFooter(true);

    expect(
      container.querySelector('[data-testid="sidebar-devices-dot"]'),
    ).toHaveAttribute("data-tone", "amber");
  });

  it("AppMenu dropdown no longer contains Automations/Workspaces items", async () => {
    const { container } = renderFooter(true);

    const menu = container.querySelector(
      'button[aria-label="Menu"]',
    ) as HTMLElement;
    await userEvent.click(menu);

    // The open dropdown renders into a portal with role="menu". Scope
    // assertions to it so the footer's own Automations button label
    // doesn't leak into the check.
    let menuEl: HTMLElement | null = null;
    await waitFor(() => {
      menuEl = document.querySelector('[role="menu"]');
      expect(menuEl).not.toBeNull();
      expect(menuEl).toHaveTextContent("Settings");
    });

    const menuText = menuEl!.textContent ?? "";
    expect(menuText).not.toContain("Automations");
    expect(menuText).not.toContain("Workspaces");
    // Sanity: the dropdown really is open (other items still present).
    expect(menuText).toContain("Documentation");
    expect(menuText).toContain("Sign out");
  });
});

describe("AppMenuFooter — update strip", () => {
  beforeEach(() => {
    __resetUpdateStatusStoreForTests();
  });

  /** Open the gear menu and hand back its portal content. */
  async function openMenu() {
    const { container } = renderFooter(true);
    await userEvent.click(
      container.querySelector('button[aria-label="Menu"]') as HTMLElement,
    );
    let menuEl: HTMLElement | null = null;
    await waitFor(() => {
      menuEl = document.querySelector('[role="menu"]');
      expect(menuEl).not.toBeNull();
    });
    return menuEl!;
  }

  type UpdateSnapshot = Parameters<
    ReturnType<typeof useUpdateStatusStore.getState>["publish"]
  >[0];

  function publish(overrides: Partial<UpdateSnapshot> = {}) {
    useUpdateStatusStore.getState().publish({
      state: "idle",
      updateVersion: null,
      downloadProgress: 0,
      isRemote: false,
      startDownload: null,
      installAndRestart: null,
      requestDesktopUpdate: null,
      ...overrides,
    });
  }

  it("claims no update status before a checker has published", async () => {
    const menuEl = await openMenu();

    // Dev builds and the window before the first check land here: the version
    // is known, the update state is not.
    expect(menuEl.textContent).not.toContain("Up to date");
    await waitFor(() => expect(menuEl.textContent).toContain("v0.14.3"));
  });

  it("says 'Up to date' only once a checker publishes an idle state", async () => {
    publish();
    const menuEl = await openMenu();

    expect(menuEl).toHaveTextContent("Up to date");
  });

  it("runs the desktop download when an update is available on desktop", async () => {
    const startDownload = vi.fn();
    publish({ state: "update-available", updateVersion: "9.9.9", startDownload });
    const menuEl = await openMenu();

    await userEvent.click(
      Array.from(menuEl.querySelectorAll("button")).find((b) =>
        b.textContent?.includes("Update available"),
      ) as HTMLElement,
    );
    expect(startDownload).toHaveBeenCalledTimes(1);
  });

  it("asks the desktop to update when the remote client clicks the strip", async () => {
    // The remote client has no updater plugin, so `startDownload` is a no-op
    // there — clicking must take the toast's request-the-desktop path instead.
    const startDownload = vi.fn();
    const requestDesktopUpdate = vi.fn();
    publish({
      state: "update-available",
      updateVersion: "9.9.9",
      isRemote: true,
      startDownload,
      requestDesktopUpdate,
    });
    const menuEl = await openMenu();

    await userEvent.click(
      Array.from(menuEl.querySelectorAll("button")).find((b) =>
        b.textContent?.includes("Update desktop"),
      ) as HTMLElement,
    );
    expect(requestDesktopUpdate).toHaveBeenCalledTimes(1);
    expect(startDownload).not.toHaveBeenCalled();
  });
});

describe("SidebarFooterBar — collapsed", () => {
  beforeEach(() => {
    hosts = [];
    statuses = {};
    syncRows = [];
  });

  function labels(container: HTMLElement) {
    return Array.from(container.querySelectorAll("button[aria-label]")).map(
      (el) => el.getAttribute("aria-label"),
    );
  }

  it("renders the icon rail without Workspaces or Devices when no device exists", () => {
    const { container } = renderFooter(false);
    expect(labels(container)).toEqual([
      "Automations",
      "Pull requests",
      "Ports",
      "Menu",
    ]);
  });

  it("slots Devices ahead of Pull requests once a device is configured", () => {
    hosts = [host(1, "zeus")];
    const { container } = renderFooter(false);
    expect(labels(container)).toEqual([
      "Automations",
      "Devices",
      "Pull requests",
      "Ports",
      "Menu",
    ]);
  });
});
