/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import type { WorkspaceSnapshot } from "@/tauri/types";

const mocks = vi.hoisted(() => ({
  activateTab: vi.fn(),
  activatePane: vi.fn(),
  closeTab: vi.fn(),
  setRightPanelTab: vi.fn(),
  error: vi.fn(),
  onOpenChange: vi.fn(),
}));
vi.mock("@/tauri/commands", () => mocks);
vi.mock("@/lib/toast", () => ({ toast: { error: mocks.error } }));
vi.mock("@/stores/app-store", () => ({
  useAppStore: (selector: (state: unknown) => unknown) =>
    selector({ appState: { pane_statuses: { p2: "permission" } } }),
}));
vi.mock("@/stores/editor-store", () => ({
  useEditorStore: (selector: (state: unknown) => unknown) =>
    selector({ tabs: { editor: { isDirty: true } } }),
}));
vi.mock("@/stores/ui-store", () => ({
  useUIStore: {
    getState: () => ({ setRightPanelTab: mocks.setRightPanelTab }),
  },
}));
vi.mock("@/components/layout/PaneNode", () => ({ PaneNode: () => null }));
vi.mock("@/components/layout/agent-launcher", () => ({
  AgentLauncher: () => <button>Add session</button>,
}));

import { MobileSessionSheet } from "./mobile-session-sheet";

const workspace = {
  workspace_id: "ws",
  title: "Mobile work",
  active_tab_id: "split",
  active_surface_id: "surface",
  tabs: [
    {
      tab_id: "split",
      title: "Main",
      kind: "terminal",
      surface_id: "surface",
      browser_id: null,
      icon: null,
    },
    {
      tab_id: "editor",
      title: "app.ts",
      kind: "editor",
      surface_id: null,
      browser_id: null,
      icon: null,
    },
    {
      tab_id: "diff",
      title: "Diff",
      kind: "diff",
      surface_id: null,
      browser_id: null,
      icon: null,
    },
  ],
  surfaces: [
    {
      surface_id: "surface",
      title: "Main",
      active_pane_id: "p1",
      root: {
        kind: "split",
        pane_id: "root",
        direction: "horizontal",
        child_sizes: [0.5, 0.5],
        children: [
          {
            kind: "terminal",
            pane_id: "p1",
            session_id: "s1",
            title: "Terminal",
          },
          {
            kind: "browser",
            pane_id: "p2",
            browser_id: "b1",
            title: "Preview",
            url: "about:blank",
          },
        ],
      },
    },
  ],
} as WorkspaceSnapshot;

beforeEach(() => {
  vi.clearAllMocks();
  mocks.activateTab.mockResolvedValue(undefined);
  mocks.activatePane.mockResolvedValue(undefined);
  mocks.closeTab.mockResolvedValue(undefined);
});
afterEach(cleanup);

function show() {
  render(
    <MobileSessionSheet
      workspace={workspace}
      open
      onOpenChange={mocks.onOpenChange}
    />,
  );
}

describe("MobileSessionSheet", () => {
  it("returns focus to the supplied title button when the sheet closes", async () => {
    const trigger = document.createElement("button");
    document.body.append(trigger);
    trigger.focus();
    const returnFocusRef = { current: trigger };
    const view = render(
      <MobileSessionSheet
        workspace={workspace}
        open
        onOpenChange={mocks.onOpenChange}
        returnFocusRef={returnFocusRef}
      />,
    );
    view.rerender(
      <MobileSessionSheet
        workspace={workspace}
        open={false}
        onOpenChange={mocks.onOpenChange}
        returnFocusRef={returnFocusRef}
      />,
    );
    await waitFor(() => expect(trigger).toHaveFocus());
    trigger.remove();
  });

  it("dismisses when the launcher activates a different tab", () => {
    const view = render(
      <MobileSessionSheet
        workspace={workspace}
        open
        onOpenChange={mocks.onOpenChange}
      />,
    );
    view.rerender(
      <MobileSessionSheet
        workspace={{ ...workspace, active_tab_id: "new-chat" }}
        open
        onOpenChange={mocks.onOpenChange}
      />,
    );
    expect(mocks.onOpenChange).toHaveBeenCalledWith(false);
  });

  it("does not dismiss midway through its own pending pane selection", async () => {
    let resolvePane!: () => void;
    mocks.activatePane.mockReturnValueOnce(
      new Promise<void>((resolve) => {
        resolvePane = resolve;
      }),
    );
    const view = render(
      <MobileSessionSheet
        workspace={{ ...workspace, active_tab_id: "editor" }}
        open
        onOpenChange={mocks.onOpenChange}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Open Preview" }));
    await waitFor(() => expect(mocks.activatePane).toHaveBeenCalledWith("p2"));
    view.rerender(
      <MobileSessionSheet
        workspace={workspace}
        open
        onOpenChange={mocks.onOpenChange}
      />,
    );
    expect(mocks.onOpenChange).not.toHaveBeenCalled();
    await act(async () => resolvePane());
    expect(mocks.onOpenChange).toHaveBeenCalledWith(false);
  });

  it("shows all tab kinds, split leaves, selected state and unsaved/status text", () => {
    show();
    expect(
      screen.getByRole("button", { name: "Open Terminal" }),
    ).toHaveAttribute("aria-current", "true");
    expect(
      screen.getByRole("button", { name: "Open Preview" }),
    ).not.toHaveAttribute("aria-current");
    expect(
      screen.getByRole("button", { name: "Open app.ts" }),
    ).toHaveTextContent("Unsaved changes");
    expect(screen.getByRole("button", { name: "Open Diff" })).toBeVisible();
    expect(
      screen.getByRole("button", { name: "Open Preview" }),
    ).toHaveTextContent("Needs you");
  });

  it("waits for tab activation before selecting a split leaf and closing the panel", async () => {
    let resolveTab!: () => void;
    mocks.activateTab.mockReturnValueOnce(
      new Promise<void>((resolve) => {
        resolveTab = resolve;
      }),
    );
    show();
    fireEvent.click(screen.getByRole("button", { name: "Open Preview" }));
    expect(mocks.activateTab).toHaveBeenCalledWith("ws", "split");
    expect(mocks.activatePane).not.toHaveBeenCalled();
    expect(mocks.setRightPanelTab).not.toHaveBeenCalled();
    await act(async () => resolveTab());
    expect(mocks.activatePane).toHaveBeenCalledWith("p2");
    expect(mocks.setRightPanelTab).toHaveBeenCalledWith("ws", null);
    expect(mocks.onOpenChange).toHaveBeenCalledWith(false);
  });

  it("opens an editor without attempting to select a nonexistent pane", async () => {
    show();
    fireEvent.click(screen.getByRole("button", { name: "Open app.ts" }));
    await waitFor(() => expect(mocks.onOpenChange).toHaveBeenCalledWith(false));
    expect(mocks.activateTab).toHaveBeenCalledWith("ws", "editor");
    expect(mocks.activatePane).not.toHaveBeenCalled();
  });

  it("keeps the sheet and panel open when pane activation fails", async () => {
    mocks.activatePane.mockRejectedValueOnce(new Error("Disconnected"));
    show();
    fireEvent.click(screen.getByRole("button", { name: "Open Preview" }));
    await waitFor(() =>
      expect(mocks.error).toHaveBeenCalledWith("Could not open session", {
        description: "Error: Disconnected",
      }),
    );
    expect(mocks.setRightPanelTab).not.toHaveBeenCalled();
    expect(mocks.onOpenChange).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Open Preview" })).toBeEnabled();
  });

  it("reports failed close without dismissing or activating another session", async () => {
    mocks.closeTab.mockRejectedValueOnce(new Error("Close failed"));
    show();
    fireEvent.click(screen.getByRole("button", { name: "Close Diff" }));
    await waitFor(() =>
      expect(mocks.error).toHaveBeenCalledWith("Could not close session", {
        description: "Error: Close failed",
      }),
    );
    expect(mocks.closeTab).toHaveBeenCalledWith("ws", "diff");
    expect(mocks.activateTab).not.toHaveBeenCalled();
    expect(mocks.onOpenChange).not.toHaveBeenCalled();
  });
});
