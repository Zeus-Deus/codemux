/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useAppStore } from "@/stores/app-store";
import type { AppStateSnapshot, WorkspaceSnapshot } from "@/tauri/types";

vi.mock("@/tauri/commands", () => ({
  activateTab: vi.fn().mockResolvedValue(undefined),
  closeTab: vi.fn().mockResolvedValue(undefined),
  createTab: vi.fn().mockResolvedValue(undefined),
  createBrowserPane: vi.fn().mockResolvedValue(undefined),
  reorderTabs: vi.fn().mockResolvedValue(undefined),
  renameTab: vi.fn().mockResolvedValue(undefined),
  splitPane: vi.fn().mockResolvedValue(undefined),
}));

import { activateTab, closeTab } from "@/tauri/commands";
import { TabBar } from "./tab-bar";

function workspace(): WorkspaceSnapshot {
  return {
    workspace_id: "ws-1",
    title: "ws",
    tabs: [
      { tab_id: "tab-a", kind: "terminal", title: "term-a", surface_id: null, browser_id: null, icon: null },
      { tab_id: "tab-b", kind: "terminal", title: "term-b", surface_id: null, browser_id: null, icon: null },
    ],
    active_tab_id: "tab-a",
    active_surface_id: "",
    surfaces: [],
  } as unknown as WorkspaceSnapshot;
}

const PANE_STATUSES = {};
beforeEach(() => {
  useAppStore.setState({
    appState: { pane_statuses: PANE_STATUSES } as unknown as AppStateSnapshot,
  });
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  useAppStore.setState({ appState: null });
});

describe("TabBar", () => {
  it("keeps each close button out of its tab so the tab has one name", () => {
    render(<TabBar workspace={workspace()} hideActions />);

    const tabs = screen.getAllByRole("tab");
    expect(tabs.map((tab) => tab.textContent)).toEqual(["term-a", "term-b"]);
    for (const tab of tabs) expect(tab.querySelector("button, [role=button]")).toBeNull();
    expect(screen.getByRole("tab", { name: "term-a" })).toHaveAttribute("aria-selected", "true");
  });

  it("closes an inactive tab without activating it first", () => {
    render(<TabBar workspace={workspace()} hideActions />);

    const close = screen.getByRole("button", { name: "Close term-b" });
    fireEvent.mouseDown(close);
    fireEvent.click(close);
    expect(closeTab).toHaveBeenCalledWith("ws-1", "tab-b");
    expect(activateTab).not.toHaveBeenCalled();
  });
});
