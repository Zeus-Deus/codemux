/// <reference types="@testing-library/jest-dom/vitest" />
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import userEvent from "@testing-library/user-event";
import { TooltipProvider } from "@/components/ui/tooltip";
import { registry } from "@/contrib/registry";
import type { PeekRenderProps } from "@/contrib/peek";
import "@/contrib/peek";
import { PEEK_AREA, RIGHT_PANEL_AREA } from "@/sdk/areas";
import { DEFAULT_PEEK_SIZE, useBrowserPeekStore } from "@/stores/browser-peek-store";
import { useSettingsStore } from "@/stores/settings-store";
import { PluginPeekOverlay } from "./PluginPeekOverlay";
import { PeekChip } from "./PeekChip";

const state = vi.hoisted(() => ({ workspaceId: "ws-1", dock: vi.fn((..._args: unknown[]) => true) }));
vi.mock("@/stores/app-store", () => ({
  useActiveWorkspaceId: () => state.workspaceId,
  selectActiveWorkspaceId: () => state.workspaceId,
  useAppStore: { getState: () => ({}) },
}));
vi.mock("@/sdk/host", () => ({
  host: {
    openRightPane: (...args: unknown[]) => state.dock(...args),
    peek: { toggle: (id: string) => {
      const store = useBrowserPeekStore.getState();
      if (store.pluginPeek?.id === id) store.closePlugin(id);
      else store.openPlugin(id, state.workspaceId);
    } },
  },
}));

const disposers: (() => void)[] = [];
let latest: PeekRenderProps;
function register(dockPaneId?: string, icon?: () => never) {
  const off = registry.register({
    id: "demo:live", source: "plugin:demo", area: PEEK_AREA, title: "Unity",
    data: { dockPaneId, icon },
    render: (props: PeekRenderProps) => {
      latest = props;
      return <div data-testid="plugin-body">{props.workspaceId}</div>;
    },
  });
  disposers.push(off);
  return off;
}
function show() {
  useBrowserPeekStore.getState().openPlugin("demo:live", "ws-1");
  return render(<TooltipProvider><PluginPeekOverlay /></TooltipProvider>);
}
beforeEach(() => {
  state.workspaceId = "ws-1";
  state.dock.mockClear();
  useBrowserPeekStore.setState({ pluginPeek: null, openWorkspaceId: null, size: DEFAULT_PEEK_SIZE });
  useSettingsStore.setState({ settings: {} });
});
afterEach(() => {
  cleanup();
  for (const off of disposers.splice(0)) off();
  vi.unstubAllGlobals();
});

it("owns header, dimensions and live status while the plugin supplies only the body", () => {
  register();
  show();
  expect(screen.getByRole("dialog", { name: "Unity preview" })).toBeInTheDocument();
  expect(screen.getByText("Unity")).toBeInTheDocument();
  expect(screen.getByText("Unity")).not.toHaveClass("border", "font-mono");
  expect(screen.getByText("Unity")).toHaveClass("font-medium");
  expect(latest.workspaceId).toBe("ws-1");
  expect(latest.size).toEqual(DEFAULT_PEEK_SIZE);
  act(() => latest.onStatusChange("live", "Editor connected"));
  expect(screen.getByRole("img", { name: "Editor connected" })).toBeInTheDocument();
  fireEvent.pointerDown(screen.getByTestId("plugin-body"));
  expect(useBrowserPeekStore.getState().pluginPeek).not.toBeNull();
  fireEvent.pointerDown(document.body);
  expect(useBrowserPeekStore.getState().pluginPeek).toBeNull();
});

it("isolates a failing plugin icon while keeping shell controls available", () => {
  const error = vi.spyOn(console, "error").mockImplementation(() => {});
  try {
    register(undefined, () => { throw new Error("icon failed"); });
    show();
    expect(screen.getByRole("alert")).toHaveTextContent("Plugin");
    expect(screen.getByRole("button", { name: "Close preview" })).toBeInTheDocument();
    expect(screen.getByTestId("plugin-body")).toBeInTheDocument();
  } finally { error.mockRestore(); }
});

it("docks only into a registered pane belonging to the same plugin", () => {
  register("demo:pane");
  const view = show();
  expect(screen.queryByRole("button", { name: "Open in side panel" })).toBeNull();
  act(() => {
    disposers.push(registry.register({ id: "demo:pane", area: RIGHT_PANEL_AREA, source: "plugin:other" }));
  });
  expect(screen.queryByRole("button", { name: "Open in side panel" })).toBeNull();
  act(() => {
    disposers.push(registry.register({ id: "demo:pane", area: RIGHT_PANEL_AREA, source: "plugin:demo" }));
  });
  fireEvent.click(view.getByRole("button", { name: "Open in side panel" }));
  expect(state.dock).toHaveBeenCalledWith("demo:pane");
  expect(useBrowserPeekStore.getState().pluginPeek).toBeNull();
});

it("drops unloaded bodies immediately instead of retaining them for exit animation", () => {
  const unload = register();
  show();
  act(unload);
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(screen.queryByTestId("plugin-body")).toBeNull();
  expect(useBrowserPeekStore.getState().pluginPeek).toBeNull();
});

it("does not close a replacement opened while docking the previous view", () => {
  register("demo:pane");
  disposers.push(registry.register({ id: "demo:pane", area: RIGHT_PANEL_AREA, source: "plugin:demo" }));
  show();
  const original = useBrowserPeekStore.getState().pluginPeek;
  state.dock.mockImplementationOnce(() => {
    useBrowserPeekStore.getState().openPlugin("demo:live", "ws-1");
    return true;
  });
  fireEvent.click(screen.getByRole("button", { name: "Open in side panel" }));
  expect(state.dock).toHaveBeenCalledWith("demo:pane");
  expect(useBrowserPeekStore.getState().pluginPeek).not.toBeNull();
  expect(useBrowserPeekStore.getState().pluginPeek).not.toBe(original);
});

it("never displays the old plugin alongside a replacement browser peek", () => {
  register();
  show();
  act(() => useBrowserPeekStore.getState().open("ws-1"));
  expect(screen.queryByRole("dialog")).toBeNull();
});

it("closes on workspace switch and ignores callbacks from an earlier open", () => {
  register();
  const view = show();
  const oldStatus = latest.onStatusChange;
  act(() => {
    useBrowserPeekStore.getState().closePlugin("demo:live");
    useBrowserPeekStore.getState().openPlugin("demo:live", "ws-1");
  });
  act(() => oldStatus("error"));
  expect(screen.getByRole("img", { name: "Connecting" })).toBeInTheDocument();
  state.workspaceId = "ws-2";
  view.rerender(<TooltipProvider><PluginPeekOverlay /></TooltipProvider>);
  expect(useBrowserPeekStore.getState().pluginPeek).toBeNull();
  expect(screen.queryByRole("dialog")).toBeNull();
});

it.each(["reopen", "workspace switch", "plugin reload"])("ignores a retained close callback after %s", (kind) => {
  const unload = register();
  const view = show();
  const oldClose = latest.close;
  act(() => useBrowserPeekStore.getState().closePlugin("demo:live"));
  if (kind === "plugin reload") act(() => { unload(); register(); });
  if (kind === "workspace switch") state.workspaceId = "ws-2";
  act(() => useBrowserPeekStore.getState().openPlugin("demo:live", state.workspaceId));
  view.rerender(<TooltipProvider><PluginPeekOverlay /></TooltipProvider>);
  const replacement = useBrowserPeekStore.getState().pluginPeek;
  act(oldClose);
  expect(useBrowserPeekStore.getState().pluginPeek).toBe(replacement);
  expect(screen.getByRole("dialog", { name: "Unity preview" })).toBeInTheDocument();
  act(() => latest.close());
  expect(useBrowserPeekStore.getState().pluginPeek).toBeNull();
});

it.each(["app", "OS"])("skips exit motion with %s reduced motion", (kind) => {
  if (kind === "app") useSettingsStore.setState({ settings: { "appearance.reduce_motion": "true" } });
  else vi.stubGlobal("matchMedia", () => ({ matches: true }));
  register();
  show();
  fireEvent.click(screen.getByRole("button", { name: "Close preview" }));
  expect(screen.queryByRole("dialog")).toBeNull();
});

it("persists drag size and removes drag listeners when closed", () => {
  vi.stubGlobal("PointerEvent", MouseEvent);
  register();
  show();
  const handle = document.querySelector("[data-peek-resize]")!;
  fireEvent.pointerDown(handle, { button: 0, clientX: 500, clientY: 100 });
  fireEvent.pointerMove(handle, { clientX: 450, clientY: 130 });
  fireEvent.pointerUp(handle);
  expect(useBrowserPeekStore.getState().size).toEqual({ width: 490, height: 330 });
  fireEvent.pointerDown(handle, { button: 0, clientX: 500, clientY: 100 });
  act(() => latest.close());
  fireEvent.pointerMove(handle, { clientX: 0, clientY: 700 });
  fireEvent.pointerUp(handle);
  expect(useBrowserPeekStore.getState().size).toEqual({ width: 490, height: 330 });
});

it("shows the compact chip only for a registered peek and toggles it with the overlay open", async () => {
  const view = render(<TooltipProvider>
    <PeekChip peekId="demo:live" label="Unity" status="live" />
    <PluginPeekOverlay />
  </TooltipProvider>);
  expect(screen.queryByRole("button")).toBeNull();
  act(() => { register(); });
  const chip = screen.getByRole("button", { name: "View Unity preview" });
  expect(chip).toHaveAttribute("aria-pressed", "false");
  await userEvent.click(chip);
  expect(chip).toHaveAttribute("aria-pressed", "true");
  await userEvent.click(chip);
  expect(chip).toHaveAttribute("aria-pressed", "false");
  view.unmount();
});
