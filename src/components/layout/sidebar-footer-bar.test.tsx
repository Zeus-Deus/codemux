/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { TooltipProvider } from "@/components/ui/tooltip";
import { SidebarProvider } from "@/components/ui/sidebar";

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
const setShowWorkspacesOverviewMock = vi.fn();

vi.mock("@/stores/ui-store", () => ({
  useUIStore: vi.fn((selector) => {
    const state = {
      setShowSettings: setShowSettingsMock,
      toggleCommandPalette: toggleCommandPaletteMock,
      setShowAutomations: setShowAutomationsMock,
      setShowWorkspacesOverview: setShowWorkspacesOverviewMock,
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
  return render(
    <TooltipProvider>
      <SidebarProvider defaultOpen={open}>
        <SidebarFooterBar />
      </SidebarProvider>
    </TooltipProvider>,
  );
}

describe("SidebarFooterBar — expanded", () => {
  beforeEach(() => {
    setShowAutomationsMock.mockClear();
    setShowWorkspacesOverviewMock.mockClear();
  });

  it("renders labeled Automations + Workspaces buttons that call the store setters", () => {
    const { container } = renderFooter(true);

    const automations = container.querySelector(
      'button[aria-label="Automations"]',
    ) as HTMLElement;
    const workspaces = container.querySelector(
      'button[aria-label="Workspaces"]',
    ) as HTMLElement;

    expect(automations).toHaveTextContent("Automations");
    expect(workspaces).toHaveTextContent("Workspaces");

    fireEvent.click(automations);
    expect(setShowAutomationsMock).toHaveBeenCalledWith(true);

    fireEvent.click(workspaces);
    expect(setShowWorkspacesOverviewMock).toHaveBeenCalledWith(true);
  });

  it("AppMenu dropdown no longer contains Automations/Workspaces items", async () => {
    const { container } = renderFooter(true);

    const menu = container.querySelector(
      'button[aria-label="Menu"]',
    ) as HTMLElement;
    await userEvent.click(menu);

    // The open dropdown renders into a portal with role="menu". Scope
    // assertions to it so the footer's own Automations/Workspaces button
    // labels don't leak into the check.
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
  it("renders the four icon buttons in order", () => {
    const { container } = renderFooter(false);

    const labels = Array.from(
      container.querySelectorAll("button[aria-label]"),
    ).map((el) => el.getAttribute("aria-label"));

    expect(labels).toEqual(["Automations", "Workspaces", "Ports", "Menu"]);
  });
});
