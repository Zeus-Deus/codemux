import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const toast = vi.hoisted(() => ({ info: vi.fn(), success: vi.fn(), error: vi.fn(), warning: vi.fn() }));
const createBrowserPane = vi.hoisted(() => vi.fn(async (_paneId: string, _url?: string) => "pane-browser"));
const openExternalUrl = vi.hoisted(() => vi.fn(async (_url: string) => "browser" as const));
const phone = vi.hoisted(() => ({ value: false }));
const openNotificationTarget = vi.hoisted(() =>
  vi.fn(async (_target: { workspace_id: string; pane_id: string }) => true),
);
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => undefined) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock("@/lib/toast", () => ({ toast }));
vi.mock("@/tauri/commands", () => ({ createBrowserPane }));
vi.mock("@/lib/open-url", () => ({ openExternalUrl }));
vi.mock("@/lib/open-notification-target", () => ({ openNotificationTarget }));
vi.mock("@/hooks/use-mobile-layout", () => ({ mobileLayoutAvailable: () => phone.value }));

import { useAppStore } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import { markComposerFocused, registerComposer } from "@/contrib/composer-registry";
import type { PaneNodeSnapshot, WorkspaceSnapshot } from "@/tauri/types";
import { applyPluginIntents, capturePluginTarget, parsePlatformPageId, platformPageId, pluginPreviewUrl } from "./plugin-actions";
import { installation, manifest } from "./test-fixtures";

const chat = (paneId: string, threadId: string): PaneNodeSnapshot =>
  ({ kind: "agent_chat", pane_id: paneId, thread_id: threadId }) as unknown as PaneNodeSnapshot;
const workspace = (id: string, paneId: string, threadId: string) =>
  ({
    workspace_id: id,
    active_surface_id: `${id}-surface`,
    surfaces: [{ surface_id: `${id}-surface`, active_pane_id: paneId, root: chat(paneId, threadId) }],
  }) as unknown as WorkspaceSnapshot;

function showWorkspace(active: string) {
  useAppStore.setState({
    appState: {
      active_workspace_id: active,
      workspaces: [workspace("ws-1", "pane-1", "thread-1"), workspace("ws-2", "pane-2", "thread-2")],
    } as never,
    pendingActiveWorkspaceId: null,
  } as never);
}

const plugin = installation();
const target = () => capturePluginTarget({ installation: plugin, workspaceId: "ws-1", threadId: "thread-1", composer: true });
const preview = [{ type: "preview.open" as const, url: "https://example.com/report" }];

beforeEach(() => {
  phone.value = false;
  showWorkspace("ws-1");
  useUIStore.setState({ showSettings: false, showPluginPage: null, showUsage: false, rightPanelTabs: {} } as never);
});
afterEach(() => vi.clearAllMocks());

it("inserts a plugin result into the invoking chat after another composer gains focus", () => {
  const insertOrigin = vi.fn();
  const insertOther = vi.fn();
  const unregisterOrigin = registerComposer("origin", {
    workspaceId: "ws-1", threadId: "thread-1", getDraft: () => "", insertText: insertOrigin, focus: vi.fn(),
  });
  const unregisterOther = registerComposer("other", {
    workspaceId: "ws-2", threadId: "thread-2", getDraft: () => "", insertText: insertOther, focus: vi.fn(),
  });
  try {
    const captured = target();
    showWorkspace("ws-2");
    markComposerFocused("other");
    applyPluginIntents(captured, [{ type: "composer.insert", text: "Plugin result" }]);
    expect(insertOrigin).toHaveBeenCalledWith("Plugin result");
    expect(insertOther).not.toHaveBeenCalled();
    expect(toast.error).not.toHaveBeenCalled();
  } finally {
    unregisterOrigin();
    unregisterOther();
  }
});

describe("preview.open", () => {
  it("opens in a Codemux browser pane beside the invoking chat while the user is still there", async () => {
    applyPluginIntents(target(), preview);
    await vi.waitFor(() => expect(createBrowserPane).toHaveBeenCalledWith("pane-1", "https://example.com/report"));
    expect(openExternalUrl).not.toHaveBeenCalled();
  });

  it("only offers the link once the user moved, and the offer opens the system browser", async () => {
    const captured = target();
    showWorkspace("ws-2");
    applyPluginIntents(captured, preview);
    expect(createBrowserPane).not.toHaveBeenCalled();
    expect(toast.info).toHaveBeenCalledTimes(1);
    const options = toast.info.mock.calls[0]![1] as { action: { onClick: () => void } };
    // Clicked even after going back: an offer never splits a pane in a workspace.
    showWorkspace("ws-1");
    options.action.onClick();
    await vi.waitFor(() => expect(openExternalUrl).toHaveBeenCalledWith("https://example.com/report"));
    expect(createBrowserPane).not.toHaveBeenCalled();
  });

  it("uses the system browser on a phone and when the pane can't be made", async () => {
    phone.value = true;
    const original = window.matchMedia;
    window.matchMedia = ((query: string) => ({ matches: true, media: query })) as typeof window.matchMedia;
    try {
      applyPluginIntents(target(), preview);
      expect(toast.info).toHaveBeenCalledTimes(1);
      expect(createBrowserPane).not.toHaveBeenCalled();
    } finally {
      window.matchMedia = original;
      phone.value = false;
    }
    createBrowserPane.mockRejectedValueOnce(new Error("no pane"));
    applyPluginIntents(target(), preview);
    await vi.waitFor(() => expect(openExternalUrl).toHaveBeenCalledWith("https://example.com/report"));
  });

  it("only ever opens http and https", () => {
    expect(pluginPreviewUrl("https://example.com/a")).toBe("https://example.com/a");
    expect(pluginPreviewUrl("http://localhost:3000")).toBe("http://localhost:3000/");
    expect(pluginPreviewUrl("javascript:alert(1)")).toBeNull();
    expect(pluginPreviewUrl("file:///etc/passwd")).toBeNull();
    expect(pluginPreviewUrl("https://user:pw@example.com")).toBeNull();
  });
});

it("platform pages never capture a renderer plugin's page route", () => {
  expect(parsePlatformPageId("platform:activity:detail")).toBeNull();
  expect(parsePlatformPageId(platformPageId("inst-1", "home"))).toEqual({ installationId: "inst-1", viewId: "home" });
});

describe("view.open offers for a side panel", () => {
  const panelPlugin = installation({
    manifest: manifest({ views: [{ id: "panel", title: "Inspector", entry: "inspector.html", placement: "side-panel" }] }),
  });
  const openPanel = [{ type: "view.open" as const, viewId: "panel" }];
  const panelTab = `plugin:@${panelPlugin.installationId}:view.panel`;

  /** ws-1 holds chats A (pane-a) and B (pane-b); `focused` is the one it shows. */
  function showTwoChats(focused: "pane-a" | "pane-b") {
    useAppStore.setState({
      appState: {
        active_workspace_id: "ws-1",
        workspaces: [
          {
            workspace_id: "ws-1",
            active_surface_id: "s-1",
            surfaces: [
              {
                surface_id: "s-1",
                active_pane_id: focused,
                root: { kind: "split", pane_id: "split", children: [chat("pane-a", "thread-a"), chat("pane-b", "thread-b")] },
              },
            ],
          },
        ],
      } as never,
      pendingActiveWorkspaceId: null,
    } as never);
  }
  const fromChatA = () =>
    capturePluginTarget({ installation: panelPlugin, workspaceId: "ws-1", threadId: "thread-a", composer: true });
  /** The offer toast's action, as the user clicks it later. */
  function acceptOffer() {
    const options = toast.info.mock.calls[toast.info.mock.calls.length - 1]![1] as { action: { onClick: () => void } };
    options.action.onClick();
  }

  it("restores the captured chat after a same-workspace switch, and opens the panel only once it is shown", async () => {
    showTwoChats("pane-a");
    const captured = fromChatA();
    // While the action runs, the user focuses chat B in the same workspace.
    showTwoChats("pane-b");
    let finishReveal: (ok: boolean) => void = () => {};
    openNotificationTarget.mockImplementationOnce(() => new Promise((resolve) => (finishReveal = resolve)));
    applyPluginIntents(captured, openPanel);
    acceptOffer();
    expect(openNotificationTarget).toHaveBeenCalledWith({ workspace_id: "ws-1", pane_id: "pane-a" });
    expect(useUIStore.getState().rightPanelTabs["ws-1"]).toBeUndefined();
    finishReveal(true);
    await vi.waitFor(() => expect(useUIStore.getState().rightPanelTabs["ws-1"]).toBe(panelTab));
  });

  it("opens nothing when the captured chat can't be restored", async () => {
    showTwoChats("pane-a");
    const captured = fromChatA();
    showTwoChats("pane-b");
    openNotificationTarget.mockResolvedValueOnce(false);
    applyPluginIntents(captured, openPanel);
    acceptOffer();
    await vi.waitFor(() => expect(openNotificationTarget).toHaveBeenCalledTimes(1));
    await Promise.resolve();
    expect(useUIStore.getState().rightPanelTabs["ws-1"]).toBeUndefined();
  });

  it("reveals the workspace when accepted from Settings or a utility page", async () => {
    showTwoChats("pane-a");
    const captured = fromChatA();
    useUIStore.getState().setShowSettings(true);
    useUIStore.getState().setShowUsage(true);
    applyPluginIntents(captured, openPanel);
    acceptOffer();
    await vi.waitFor(() => expect(useUIStore.getState().rightPanelTabs["ws-1"]).toBe(panelTab));
    expect(useUIStore.getState().showSettings).toBe(false);
    expect(useUIStore.getState().showUsage).toBe(false);
    // Still on the captured chat: nothing to restore.
    expect(openNotificationTarget).not.toHaveBeenCalled();
  });

  it("restores the captured chat before the phone's full-screen page", async () => {
    phone.value = true;
    const original = window.matchMedia;
    window.matchMedia = ((query: string) => ({ matches: true, media: query })) as typeof window.matchMedia;
    try {
      showTwoChats("pane-a");
      const captured = fromChatA();
      showTwoChats("pane-b");
      let finishReveal: (ok: boolean) => void = () => {};
      openNotificationTarget.mockImplementationOnce(() => new Promise((resolve) => (finishReveal = resolve)));
      applyPluginIntents(captured, openPanel);
      acceptOffer();
      expect(useUIStore.getState().showPluginPage).toBeNull();
      finishReveal(true);
      await vi.waitFor(() => expect(useUIStore.getState().showPluginPage).toBe(platformPageId(panelPlugin.installationId, "panel")));
      expect(openNotificationTarget).toHaveBeenCalledWith({ workspace_id: "ws-1", pane_id: "pane-a" });
    } finally {
      window.matchMedia = original;
    }
  });
});
