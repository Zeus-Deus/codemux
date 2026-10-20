import { describe, it, expect, vi, beforeEach } from "vitest";

// The command wrappers are thin `invoke` calls; mock the Tauri core so we can
// observe the exact command name + args each wrapper sends over the IPC/WS.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
  Channel: class {},
}));

import { invoke } from "@tauri-apps/api/core";
import {
  activateWorkspace,
  agentChatOpenSearchResult,
  archiveWorkspace,
  closeWorkspace,
  closeWorkspaceWithWorktree,
} from "./commands";
import {
  adoptLocalFallbackActivation,
  resetLocalWorkspaceActivations,
  wasActivatedLocally,
} from "@/lib/local-activation";

const mockInvoke = vi.mocked(invoke);

beforeEach(() => {
  mockInvoke.mockClear();
  resetLocalWorkspaceActivations();
});

describe("activateWorkspace", () => {
  // Every navigation surface bottoms out here, so recording the activation in
  // the wrapper is what keeps the local-activation record complete — callers
  // that use the raw command (Open folder, Clone, the new-workspace dialog,
  // onboarding) would otherwise land on a workspace nothing ever fills.
  it("records the activation as local and invokes activate_workspace", async () => {
    expect(wasActivatedLocally("ws-1")).toBe(false);

    await activateWorkspace("ws-1");

    expect(mockInvoke).toHaveBeenCalledWith("activate_workspace", {
      workspaceId: "ws-1",
    });
    expect(wasActivatedLocally("ws-1")).toBe(true);
  });

  it("does not record workspaces this client never activated", async () => {
    await activateWorkspace("ws-1");
    expect(wasActivatedLocally("ws-2")).toBe(false);
  });
});

describe("workspace removal commands", () => {
  // Removing the active workspace lets the BACKEND choose the next one, so
  // there is no activation to record — only this marker, which keeps the
  // workspace we land on counted as ours to fill.
  it.each([
    ["closeWorkspace", () => closeWorkspace("ws-1", false)],
    [
      "closeWorkspaceWithWorktree",
      () => closeWorkspaceWithWorktree("ws-1", true, false, false),
    ],
    ["archiveWorkspace", () => archiveWorkspace("ws-1")],
  ])("%s marks the backend's fallback activation as local", async (_, run) => {
    await run();

    expect(adoptLocalFallbackActivation("ws-fallback")).toBe(true);
    expect(wasActivatedLocally("ws-fallback")).toBe(true);
  });

  it("does not adopt an activation when nothing was closed", () => {
    expect(adoptLocalFallbackActivation("ws-theirs")).toBe(false);
    expect(wasActivatedLocally("ws-theirs")).toBe(false);
  });
});

describe("remote activation", () => {
  it("opens a chat in this client's pane selection after hydrating its tab", async () => {
    const { useAppStore } = await import("@/stores/app-store");
    const { setRemoteViewHost } = await import("@/remote/client-view");
    const remoteWindow = window as Window & { __CODEMUX_REMOTE__?: boolean };
    remoteWindow.__CODEMUX_REMOTE__ = true;
    setRemoteViewHost("native-chat-navigation");
    const previous = useAppStore.getState();
    const snapshot = {
      active_workspace_id: "w",
      workspaces: [{
        workspace_id: "w", active_tab_id: "receiver", active_surface_id: "receiver-surface",
        tabs: [{ tab_id: "receiver", surface_id: "receiver-surface" }],
        surfaces: [{ surface_id: "receiver-surface", active_pane_id: "receiver-pane", root: { kind: "agent_chat", pane_id: "receiver-pane" } }],
      }],
    } as import("./types").AppStateSnapshot;
    try {
      useAppStore.setState({ appState: null, remoteActiveWorkspaceId: null });
      useAppStore.getState().setAppState(snapshot);
      const hydrated = structuredClone(snapshot);
      hydrated.workspaces[0].tabs.push({ tab_id: "sender", surface_id: "sender-surface", kind: "terminal", title: "Sender", browser_id: null, icon: null });
      hydrated.workspaces[0].surfaces.push({ surface_id: "sender-surface", title: "Sender", active_pane_id: "sender-pane", root: { kind: "agent_chat", pane_id: "sender-pane", provider: "codex", thread_id: "sender-thread", cwd: "/repo", title: "Sender" } });
      mockInvoke.mockResolvedValueOnce({ workspace_id: "w", pane_id: "sender-pane" }).mockResolvedValueOnce(hydrated);
      await agentChatOpenSearchResult("sender-thread");
      expect(mockInvoke).toHaveBeenCalledWith("agent_chat_open_search_result", { threadId: "sender-thread" });
      expect(mockInvoke).toHaveBeenCalledWith("get_app_state");
      expect(useAppStore.getState().appState?.workspaces[0].active_tab_id).toBe("sender");
      // A later desktop selection must not undo the link navigation here.
      useAppStore.getState().setAppState(hydrated);
      expect(useAppStore.getState().appState?.workspaces[0].active_tab_id).toBe("sender");
    } finally {
      delete remoteWindow.__CODEMUX_REMOTE__;
      useAppStore.setState(previous, true);
      localStorage.clear();
    }
  });

  it("touches without invoking desktop activation and persists only the newest successful selection", async () => {
    const { useAppStore, selectActiveWorkspaceId } = await import("@/stores/app-store");
    const remoteWindow = window as Window & { __CODEMUX_REMOTE__?: boolean };
    remoteWindow.__CODEMUX_REMOTE__ = true;
    const previous = useAppStore.getState();
    try {
      useAppStore.setState({
        appState: {
          active_workspace_id: "desktop",
          workspaces: ["desktop", "phone", "tablet"].map((workspace_id) => ({ workspace_id })),
        } as import("./types").AppStateSnapshot,
        remoteActiveWorkspaceId: "desktop", pendingActiveWorkspaceId: null,
      });
      let resolveFirst!: () => void;
      mockInvoke.mockImplementationOnce(() => new Promise<void>((resolve) => { resolveFirst = resolve; }));
      const first = activateWorkspace("phone");
      await activateWorkspace("tablet");
      resolveFirst();
      await first;
      expect(mockInvoke).toHaveBeenCalledWith("touch_workspace", { workspaceId: "phone" });
      expect(mockInvoke).not.toHaveBeenCalledWith("activate_workspace", expect.anything());
      expect(selectActiveWorkspaceId(useAppStore.getState())).toBe("tablet");
      expect(useAppStore.getState().appState?.active_workspace_id).toBe("desktop");

      mockInvoke.mockRejectedValueOnce(new Error("disconnected"));
      await expect(activateWorkspace("phone")).rejects.toThrow("disconnected");
      expect(selectActiveWorkspaceId(useAppStore.getState())).toBe("tablet");
    } finally {
      delete remoteWindow.__CODEMUX_REMOTE__;
      useAppStore.setState(previous, true);
      localStorage.clear();
    }
  });
});
