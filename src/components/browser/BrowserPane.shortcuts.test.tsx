import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { createSeedAppState } from "@/dev/mock-fixtures";
import { useKeyboardShortcuts } from "@/hooks/use-keyboard-shortcuts";
import { updateAppShortcuts } from "@/lib/app-shortcuts";
import { useAppStore } from "@/stores/app-store";
import { usePaneZoomStore } from "@/stores/pane-zoom-store";
import { useSyncedSettingsStore } from "@/stores/synced-settings-store";
import { useUIStore } from "@/stores/ui-store";
import { activatePane, activateTab, activateWorkspace, agentBrowserRun } from "@/tauri/commands";
import { BrowserPane } from "./BrowserPane";

vi.mock("@/tauri/commands", () => ({
  startBrowserStream: vi.fn().mockResolvedValue("ws://localhost:9777"),
  agentBrowserRun: vi.fn().mockResolvedValue(undefined),
  activatePane: vi.fn().mockResolvedValue(undefined),
  activateTab: vi.fn().mockResolvedValue(undefined),
  activateWorkspace: vi.fn().mockResolvedValue(undefined),
}));

class StreamSocket {
  static OPEN = 1;
  static instances: StreamSocket[] = [];
  readyState = 1;
  send = vi.fn();
  onopen = null;
  onmessage = null;
  onclose = null;
  onerror = null;
  constructor() { StreamSocket.instances.push(this); }
  close() { this.readyState = 3; }
}

function AppShortcuts() {
  useKeyboardShortcuts();
  return null;
}

function seed(lone = false) {
  const state = createSeedAppState();
  const ws = state.workspaces[0];
  ws.workspace_id = "ws-browser";
  ws.active_tab_id = "tab-1";
  ws.tabs = ["tab-1", ...(lone ? [] : ["tab-2"])].map((tab_id) => ({
    tab_id, kind: "terminal", title: "Tab", surface_id: "surface-1", browser_id: null, icon: null,
  }));
  const browser = { kind: "browser" as const, pane_id: "p-browser", browser_id: "browser-test", title: "Browser" };
  ws.active_surface_id = "surface-1";
  ws.surfaces = [{
    surface_id: "surface-1", title: "Main", active_pane_id: "p-browser",
    root: lone ? browser : {
      kind: "split", pane_id: "p-split", direction: "horizontal", child_sizes: [0.5, 0.5],
      children: [browser, { kind: "terminal", pane_id: "p-chat", session_id: "session-chat", title: "Chat" }],
    },
  }];
  const blocked = { ...ws, workspace_id: "ws-blocked", surfaces: [{
    ...ws.surfaces[0], surface_id: "surface-blocked", active_pane_id: "p-blocked",
    root: { kind: "terminal" as const, pane_id: "p-blocked", session_id: "session-blocked", title: "Blocked" },
  }] };
  state.active_workspace_id = ws.workspace_id;
  state.workspaces = [ws, blocked];
  state.agent_browser_sessions = [];
  state.pane_statuses = { "p-blocked": "permission" };
  useAppStore.setState({ appState: state, pendingActiveWorkspaceId: null });
}

async function renderBrowser() {
  const view = render(<>
    <AppShortcuts />
    <div data-pane-drop-id="p-browser"><BrowserPane browserId="browser-test" focused visible hideToolbar /></div>
    <div data-pane-drop-id="p-chat"><textarea data-composer-input /></div>
  </>);
  await act(async () => {});
  const browserShell = view.container.querySelector<HTMLElement>('[data-pane-drop-id="p-browser"]')!;
  const chatShell = view.container.querySelector<HTMLElement>('[data-pane-drop-id="p-chat"]')!;
  browserShell.getBoundingClientRect = () => new DOMRect(0, 0, 100, 100);
  chatShell.getBoundingClientRect = () => new DOMRect(101, 0, 100, 100);
  const canvas = view.container.querySelector("canvas")!;
  canvas.focus();
  expect(document.activeElement).toBe(canvas);
  return { canvas, socket: StreamSocket.instances[0], chat: chatShell.querySelector("textarea") };
}

beforeEach(() => {
  vi.clearAllMocks();
  StreamSocket.instances = [];
  vi.stubGlobal("WebSocket", StreamSocket);
  seed();
  usePaneZoomStore.setState({ zoomedPaneBySurface: {} });
  useUIStore.setState({ showSettings: false, showCommandPalette: false, showFileSearch: false, showContentSearch: false });
  const settings = useSyncedSettingsStore.getState().settings;
  useSyncedSettingsStore.setState({ settings: { ...settings, keyboard: { shortcuts: {} } } });
  updateAppShortcuts({});
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  useAppStore.setState({ appState: null, pendingActiveWorkspaceId: null });
});

describe("BrowserPane app shortcut propagation", () => {
  it("reloads the browser page before the window's interface reload guard can claim Ctrl+R", async () => {
    const { canvas, socket } = await renderBrowser();
    fireEvent.keyDown(canvas, { key: "r", code: "KeyR", ctrlKey: true });
    fireEvent.keyUp(canvas, { key: "r", code: "KeyR", ctrlKey: true });
    expect(agentBrowserRun).toHaveBeenCalledWith("browser-test", "reload", {});
    expect(socket.send).not.toHaveBeenCalled();
  });

  it("moves from the focused browser canvas into the adjacent composer", async () => {
    const { canvas, socket, chat } = await renderBrowser();
    fireEvent.keyDown(canvas, { key: "ArrowRight", code: "ArrowRight", altKey: true, shiftKey: true });
    expect(activatePane).toHaveBeenCalledWith("p-chat");
    expect(document.activeElement).toBe(chat);
    expect(socket.send).not.toHaveBeenCalled();
    fireEvent.keyUp(canvas, { key: "ArrowRight", code: "ArrowRight" });
    expect(socket.send).not.toHaveBeenCalled();
  });

  it.each([
    { key: "PageDown", code: "PageDown", ctrlKey: true },
    { key: "PageUp", code: "PageUp", ctrlKey: true },
    { key: "Enter", code: "Enter", ctrlKey: true, shiftKey: true },
    { key: "J", code: "KeyJ", ctrlKey: true, shiftKey: true },
  ])("dispatches $key through the app without sending either event to the remote page", async (event) => {
    const { canvas, socket } = await renderBrowser();
    fireEvent.keyDown(canvas, event);
    if (event.key.startsWith("Page")) expect(activateTab).toHaveBeenCalledWith("ws-browser", "tab-2");
    if (event.key === "Enter") expect(usePaneZoomStore.getState().zoomedPaneBySurface).toEqual({ "surface-1": "p-browser" });
    if (event.key === "J") expect(activateWorkspace).toHaveBeenCalledWith("ws-blocked");
    fireEvent.keyUp(canvas, { key: event.key.toLowerCase(), code: event.code });
    expect(socket.send).not.toHaveBeenCalled();
  });

  it("keeps a yielded keyup out of the remote page after the shortcut becomes inapplicable", async () => {
    const { canvas, socket } = await renderBrowser();
    fireEvent.keyDown(canvas, { key: "PageDown", code: "PageDown", ctrlKey: true });
    act(() => seed(true));
    fireEvent.keyUp(canvas, { key: "PageDown", code: "PageDown" });
    expect(socket.send).not.toHaveBeenCalled();
  });

  it.each([
    { key: "PageDown", ctrlKey: true },
    { key: "PageUp", ctrlKey: true },
    { key: "Enter", ctrlKey: true, shiftKey: true },
    { key: "Escape" },
  ])("forwards inapplicable $key shortcuts to a lone browser pane", async (event) => {
    seed(true);
    const { canvas, socket } = await renderBrowser();
    fireEvent.keyDown(canvas, event);
    fireEvent.keyUp(canvas, event);
    expect(socket.send).toHaveBeenCalledTimes(2);
    expect(JSON.parse(socket.send.mock.calls[0][0])).toMatchObject({ eventType: "rawKeyDown", key: event.key });
    expect(JSON.parse(socket.send.mock.calls[1][0])).toMatchObject({ eventType: "keyUp", key: event.key });
    expect(activateTab).not.toHaveBeenCalled();
  });

  it("uses resolved user bindings instead of hard-coded combinations", async () => {
    const settings = useSyncedSettingsStore.getState().settings;
    useSyncedSettingsStore.setState({ settings: { ...settings, keyboard: { shortcuts: { nextTab: "F6" } } } });
    const { canvas, socket } = await renderBrowser();
    fireEvent.keyDown(canvas, { key: "F6", code: "F6" });
    fireEvent.keyUp(canvas, { key: "F6", code: "F6" });
    expect(activateTab).toHaveBeenCalledWith("ws-browser", "tab-2");
    expect(socket.send).not.toHaveBeenCalled();
    fireEvent.keyDown(canvas, { key: "PageDown", ctrlKey: true });
    fireEvent.keyUp(canvas, { key: "PageDown", ctrlKey: true });
    expect(socket.send).toHaveBeenCalledTimes(2);
  });
});
