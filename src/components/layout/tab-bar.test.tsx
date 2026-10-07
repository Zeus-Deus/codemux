/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

import type { WorkspaceSnapshot } from "@/tauri/types";

const mocks = vi.hoisted(() => ({
  closeTab: vi.fn().mockResolvedValue(undefined),
  renameTab: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@/tauri/commands", () => ({
  activateTab: vi.fn().mockResolvedValue(undefined),
  closeTab: (...a: unknown[]) => mocks.closeTab(...a),
  createTab: vi.fn().mockResolvedValue(undefined),
  createBrowserPane: vi.fn().mockResolvedValue(undefined),
  reorderTabs: vi.fn().mockResolvedValue(undefined),
  renameTab: (...a: unknown[]) => mocks.renameTab(...a),
  splitPane: vi.fn().mockResolvedValue(undefined),
}));

import { TabBar } from "./tab-bar";

function makeWorkspace(): WorkspaceSnapshot {
  return {
    workspace_id: "ws-1",
    title: "demo",
    workspace_type: "standard",
    cwd: "/p",
    git_branch: "main",
    git_ahead: 0,
    git_behind: 0,
    git_additions: 0,
    git_deletions: 0,
    git_changed_files: 0,
    notification_count: 0,
    latest_agent_state: null,
    worktree_path: null,
    project_root: "/p",
    pr_number: null,
    pr_state: null,
    pr_url: null,
    linked_issue: null,
    notifications_muted: false,
    tabs: [
      { tab_id: "tab-a", kind: "terminal", title: "term-a", surface_id: null, browser_id: null, icon: null },
      { tab_id: "tab-b", kind: "terminal", title: "term-b", surface_id: null, browser_id: null, icon: null },
    ],
    active_tab_id: "tab-a",
    active_surface_id: "surface-none",
    surfaces: [],
  };
}

beforeEach(() => {
  mocks.closeTab.mockClear();
  mocks.renameTab.mockClear();
});
afterEach(cleanup);

describe("TabBar", () => {
  it("renames inline from the context menu instead of a window prompt", async () => {
    const prompt = vi.spyOn(window, "prompt");
    render(<TabBar workspace={makeWorkspace()} hideActions />);
    fireEvent.contextMenu(screen.getByText("term-b"));
    fireEvent.click(await screen.findByText("Rename tab"));

    const input = await screen.findByLabelText("Tab name");
    fireEvent.change(input, { target: { value: "logs" } });
    fireEvent.keyDown(input, { key: "Enter" });

    expect(prompt).not.toHaveBeenCalled();
    expect(mocks.renameTab).toHaveBeenCalledWith("ws-1", "tab-b", "logs");
    prompt.mockRestore();
  });

  it("closes a tab on middle-click", () => {
    render(<TabBar workspace={makeWorkspace()} hideActions />);
    const tab = screen.getByText("term-b").closest("[data-tab-id]") as HTMLElement;
    fireEvent(tab, new MouseEvent("auxclick", { button: 1, bubbles: true }));
    expect(mocks.closeTab).toHaveBeenCalledWith("ws-1", "tab-b");
  });
});
