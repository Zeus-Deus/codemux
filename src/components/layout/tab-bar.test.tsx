/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useCloseGuardStore } from "@/lib/close-guard";
import { TooltipProvider } from "@/components/ui/tooltip";
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

import { activateTab, closeTab, renameTab } from "@/tauri/commands";
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
    appState: { workspaces: [workspace()], pane_statuses: PANE_STATUSES } as unknown as AppStateSnapshot,
  });
  useCloseGuardStore.setState({ prompt: null });
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

  it("closes an inactive tab without activating it first", async () => {
    render(<TabBar workspace={workspace()} hideActions />);

    const close = screen.getByRole("button", { name: "Close term-b" });
    fireEvent.mouseDown(close);
    fireEvent.click(close);
    await waitFor(() => expect(closeTab).toHaveBeenCalledWith("ws-1", "tab-b"));
    expect(activateTab).not.toHaveBeenCalled();
  });

  it("renames inline from the context menu instead of a window prompt", async () => {
    const prompt = vi.spyOn(window, "prompt");
    render(<TabBar workspace={workspace()} hideActions />);
    fireEvent.contextMenu(screen.getByText("term-b"));
    fireEvent.click(await screen.findByText("Rename tab"));

    const input = await screen.findByLabelText("Tab name");
    fireEvent.change(input, { target: { value: "logs" } });
    fireEvent.keyDown(input, { key: "Enter" });

    expect(prompt).not.toHaveBeenCalled();
    expect(renameTab).toHaveBeenCalledWith("ws-1", "tab-b", "logs");
    prompt.mockRestore();
  });

  it("closes a tab on middle-click", async () => {
    render(<TabBar workspace={workspace()} hideActions />);
    const tab = screen.getByText("term-b").closest("[data-tab-id]") as HTMLElement;
    fireEvent(tab, new MouseEvent("auxclick", { button: 1, bubbles: true }));
    await waitFor(() => expect(closeTab).toHaveBeenCalledWith("ws-1", "tab-b"));
  });

  it("asks before a middle-click stops an agent and Cancel preserves its tab", async () => {
    const ws = workspace();
    ws.tabs[1].surface_id = "chat-surface";
    ws.surfaces = [{
      surface_id: "chat-surface", title: "", active_pane_id: "chat-pane",
      root: { kind: "agent_chat", pane_id: "chat-pane", title: "Chat", thread_id: "thread-1", provider: "codex", cwd: "/repo" },
    }];
    useAppStore.setState({ appState: {
      workspaces: [ws], pane_statuses: { "chat-pane": "working" },
    } as unknown as AppStateSnapshot });
    render(<TooltipProvider><TabBar workspace={ws} hideActions /></TooltipProvider>);
    const tab = screen.getByText("term-b").closest("[data-tab-id]")!;
    fireEvent(tab, new MouseEvent("auxclick", { button: 1, bubbles: true }));
    await waitFor(() => expect(useCloseGuardStore.getState().prompt).not.toBeNull());
    expect(closeTab).not.toHaveBeenCalled();
    useCloseGuardStore.getState().prompt!.cancel();
    expect(closeTab).not.toHaveBeenCalled();
  });
});
