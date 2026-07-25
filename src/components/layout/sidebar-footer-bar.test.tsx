/// <reference types="@testing-library/jest-dom/vitest" />
import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, waitFor } from "@testing-library/react";
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

describe("SidebarFooterBar — collapsed", () => {
  it("renders the four icon buttons in order", () => {
    const { container } = renderFooter(false);

    const labels = Array.from(
      container.querySelectorAll("button[aria-label]"),
    ).map((el) => el.getAttribute("aria-label"));

    expect(labels).toEqual(["Automations", "Workspaces", "Ports", "Menu"]);
  });
});
