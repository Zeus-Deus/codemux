/// <reference types="@testing-library/jest-dom/vitest" />
import { memo } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { PaneNodeSnapshot, WorkspaceSnapshot } from "@/tauri/types";

// xterm cannot mount in jsdom; the stub reports the `visible` prop it gets so
// the test can see which panes a zoom hides.
vi.mock("@/components/terminal/TerminalPane", () => ({
  TerminalPane: memo(({ sessionId, visible }: { sessionId: string; visible: boolean }) => (
    <div data-testid={`term-${sessionId}`} data-visible={String(visible)} />
  )),
}));
vi.mock("@/tauri/commands", () => ({
  splitPane: vi.fn(),
  closePane: vi.fn(),
  activatePane: vi.fn(),
  resizeSplit: vi.fn(),
  swapPanes: vi.fn(),
}));

import { PaneContainer } from "./pane-container";
import { usePaneZoomStore } from "@/stores/pane-zoom-store";

const term = (id: string): PaneNodeSnapshot => ({
  kind: "terminal",
  pane_id: id,
  session_id: `s-${id}`,
  title: id,
});

function workspace(activePaneId: string): WorkspaceSnapshot {
  // left | (top-right / bottom-right)
  const root: PaneNodeSnapshot = {
    kind: "split",
    pane_id: "outer",
    direction: "horizontal",
    child_sizes: [0.5, 0.5],
    children: [
      term("left"),
      {
        kind: "split",
        pane_id: "inner",
        direction: "vertical",
        child_sizes: [0.5, 0.5],
        children: [term("top"), term("bottom")],
      },
    ],
  };
  return {
    workspace_id: "ws-1",
    active_surface_id: "sf-1",
    surfaces: [{ surface_id: "sf-1", title: "", root, active_pane_id: activePaneId }],
  } as unknown as WorkspaceSnapshot;
}

const visibility = () =>
  Object.fromEntries(
    ["left", "top", "bottom"].map((id) => [
      id,
      document.querySelector(`[data-testid="term-s-${id}"]`)?.getAttribute("data-visible"),
    ]),
  );

beforeEach(() => usePaneZoomStore.setState({ zoomedPaneBySurface: {} }));
afterEach(() => cleanup());

describe("PaneContainer zoom", () => {
  it("renders every pane of the split when nothing is zoomed", async () => {
    render(<PaneContainer workspace={workspace("top")} />);
    await screen.findByTestId("term-s-left");
    expect(visibility()).toEqual({ left: "true", top: "true", bottom: "true" });
    expect(document.querySelector("[data-pane-zoom-chip]")).toBeNull();
  });

  it("fills the surface with the zoomed pane and keeps the rest mounted but hidden", async () => {
    usePaneZoomStore.setState({ zoomedPaneBySurface: { "sf-1": "top" } });
    render(<PaneContainer workspace={workspace("top")} />);
    await screen.findByTestId("term-s-left");

    expect(visibility()).toEqual({ left: "false", top: "true", bottom: "false" });
    const outer = document.querySelector<HTMLElement>('[data-split-pane-id="outer"]')!;
    expect(outer.style.gridTemplateColumns).toBe("1fr");
    expect(document.querySelector("[data-pane-zoom-chip]")).toBeInTheDocument();
  });

  it("restores the split from the Zoomed chip", async () => {
    usePaneZoomStore.setState({ zoomedPaneBySurface: { "sf-1": "top" } });
    render(<PaneContainer workspace={workspace("top")} />);
    await screen.findByTestId("term-s-left");

    fireEvent.click(document.querySelector("[data-pane-zoom-chip]")!);
    expect(visibility()).toEqual({ left: "true", top: "true", bottom: "true" });
  });

  it("ends the zoom once focus moves to another pane", async () => {
    usePaneZoomStore.setState({ zoomedPaneBySurface: { "sf-1": "top" } });
    const { rerender } = render(<PaneContainer workspace={workspace("top")} />);
    await screen.findByTestId("term-s-left");

    act(() => rerender(<PaneContainer workspace={workspace("left")} />));
    expect(visibility()).toEqual({ left: "true", top: "true", bottom: "true" });
    expect(usePaneZoomStore.getState().zoomedPaneBySurface).toEqual({});
  });
});
