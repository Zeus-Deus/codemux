import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const toast = vi.hoisted(() => ({ info: vi.fn(), success: vi.fn(), error: vi.fn(), warning: vi.fn() }));
const createBrowserPane = vi.hoisted(() => vi.fn(async (_paneId: string, _url?: string) => "pane-browser"));
const openExternalUrl = vi.hoisted(() => vi.fn(async (_url: string) => "browser" as const));
const phone = vi.hoisted(() => ({ value: false }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => undefined) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock("@/lib/toast", () => ({ toast }));
vi.mock("@/tauri/commands", () => ({ createBrowserPane }));
vi.mock("@/lib/open-url", () => ({ openExternalUrl }));
vi.mock("@/lib/open-notification-target", () => ({ openNotificationTarget: vi.fn(async () => undefined) }));
vi.mock("@/hooks/use-mobile-layout", () => ({ mobileLayoutAvailable: () => phone.value }));

import { useAppStore } from "@/stores/app-store";
import type { PaneNodeSnapshot, WorkspaceSnapshot } from "@/tauri/types";
import { applyPluginIntents, capturePluginTarget, parsePlatformPageId, platformPageId, pluginPreviewUrl } from "./plugin-actions";
import { installation } from "./test-fixtures";

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
});
afterEach(() => vi.clearAllMocks());

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
