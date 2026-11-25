import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  agentChatCreatePane,
  applyPreset,
  createEmptyWorkspace,
  createTab,
  createWorktreeWorkspaceResult,
} from "./commands";
import { PartialPresetLaunchError } from "@/lib/partial-preset-launch";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke, Channel: vi.fn() }));

beforeEach(() => invoke.mockReset());
afterEach(() => { delete (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__; });

describe("background workspace creation IPC", () => {
  it("passes non-selection to native chat workspace creation", async () => {
    invoke.mockResolvedValue({ workspace_id: "ws-new", cwd: "/project" });
    const initialChat = { provider: "claude" as const, thread_id: "thread-new" };
    expect(await createEmptyWorkspace("/project", { initialChat, select: false })).toBe("ws-new");
    expect(invoke).toHaveBeenCalledWith("materialize_chat_workspace", {
      cwd: "/project", initialChat, skipSetup: null, select: false,
    });
  });

  it("passes non-selection to native worktree creation", async () => {
    invoke.mockResolvedValue({ workspace_id: "ws-new", cwd: "/worktrees/task" });
    await createWorktreeWorkspaceResult("/project", "task", true, "empty",
      null, null, null, null, null, undefined, false);
    expect(invoke).toHaveBeenCalledWith("create_worktree_workspace",
      expect.objectContaining({ select: false, repoPath: "/project", branch: "task" }));
  });

  it("returns the original terminal tab supplied by workspace creation", async () => {
    invoke.mockResolvedValue({
      workspace_id: "ws-new",
      cwd: "/worktrees/task",
      adopted: false,
      initial_tab_id: "tab-original",
    });

    const created = await createWorktreeWorkspaceResult("/project", "task", true, "single");

    expect(created.initialTabId).toBe("tab-original");
  });

  it.each([false, true])("creates a background pane without selecting or refreshing the view (remote: %s)", async (remote) => {
    (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__ = remote;
    invoke.mockResolvedValue("pane-new");
    expect(await agentChatCreatePane("ws-new", "claude", "/project", null, "thread-new", false)).toBe("pane-new");
    expect(invoke).toHaveBeenCalledExactlyOnceWith("agent_chat_create_pane", {
      workspaceId: "ws-new", provider: "claude", cwd: "/project",
      launchMode: null, threadId: "thread-new", select: false,
    });
  });
});

describe("fresh preset launch IPC", () => {
  it("keeps a remote fresh tab selected when the desktop selects another tab", async () => {
    const { useAppStore } = await import("@/stores/app-store");
    const { setRemoteViewHost } = await import("@/remote/client-view");
    (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__ = true;
    setRemoteViewHost("fresh-preset-selection");
    const previous = useAppStore.getState();
    const snapshot = {
      active_workspace_id: "ws-new",
      workspaces: [{
        workspace_id: "ws-new",
        active_tab_id: "tab-desktop",
        active_surface_id: "surface-desktop",
        tabs: ["desktop", "new"].map((name) => ({
          tab_id: `tab-${name}`, surface_id: `surface-${name}`,
          kind: "terminal", title: name, browser_id: null, icon: null,
        })),
        surfaces: ["desktop", "new"].map((name) => ({
          surface_id: `surface-${name}`, active_pane_id: `pane-${name}`,
          root: { kind: "terminal", pane_id: `pane-${name}` },
        })),
      }],
    } as import("./types").AppStateSnapshot;

    invoke.mockImplementation(async (command: string) => {
      if (command === "create_tab") return "tab-new";
      if (command === "get_app_state") return structuredClone(snapshot);
      if (command === "apply_preset") return null;
      return undefined;
    });
    try {
      useAppStore.setState({ appState: null, remoteActiveWorkspaceId: null });
      expect(await createTab("ws-new", "terminal")).toBe("tab-new");
      expect(useAppStore.getState().appState?.workspaces[0].active_tab_id).toBe("tab-new");

      await applyPreset("ws-new", "preset", "current_terminal", null, null, true, "tab-new");
      expect(useAppStore.getState().appState?.workspaces[0].active_tab_id).toBe("tab-new");
      expect(invoke).toHaveBeenCalledWith("apply_preset", expect.objectContaining({
        targetTabId: "tab-new", freshTerminal: true,
      }));
    } finally {
      useAppStore.setState(previous, true);
      localStorage.clear();
    }
  });

  it("surfaces confirmed tabs when a later command fails", async () => {
    invoke.mockResolvedValue({
      confirmed_tab_ids: ["tab-original"],
      total_commands: 2,
      error: "second terminal rejected the write",
    });

    const launch = applyPreset("ws-new", "preset", "current_terminal", null, null, true, "tab-original");
    await expect(launch).rejects.toBeInstanceOf(PartialPresetLaunchError);
    await expect(launch).rejects.toMatchObject({
      confirmedTabIds: ["tab-original"],
      totalCommands: 2,
      message: expect.stringContaining("second terminal rejected the write"),
    });
    expect(invoke).toHaveBeenCalledWith("apply_preset", expect.objectContaining({
      freshTerminal: true,
      targetTabId: "tab-original",
      initialPrompt: null,
    }));
  });

  it("preserves confirmed write count when every delivered tab has closed", async () => {
    invoke.mockResolvedValue({
      confirmed_tab_ids: [],
      confirmed_commands: 1,
      total_commands: 2,
      error: "the confirmed tab closed",
    });

    await expect(applyPreset("ws-new", "preset", "current_terminal", null, null, true, "tab-original"))
      .rejects.toMatchObject({
        confirmedTabIds: [],
        confirmedCommands: 1,
        totalCommands: 2,
      });
  });

  it("resolves after all command writes are confirmed", async () => {
    invoke.mockResolvedValue(null);
    await expect(applyPreset("ws-new", "preset", "current_terminal", null, null, true, "tab-original"))
      .resolves.toBeUndefined();
  });

  it("keeps the partial launch result when the remote view refresh fails", async () => {
    (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__ = true;
    invoke
      .mockResolvedValueOnce({
        confirmed_tab_ids: ["tab-original"],
        total_commands: 2,
        error: "second terminal rejected the write",
      })
      .mockRejectedValueOnce(new Error("remote refresh unavailable"));

    await expect(applyPreset("ws-new", "preset", "current_terminal", null, null, true, "tab-original"))
      .rejects.toMatchObject({
        confirmedTabIds: ["tab-original"],
        totalCommands: 2,
      });
    expect(invoke).toHaveBeenCalledWith("get_app_state");
  });

  it("keeps confirmed success when the remote view refresh fails", async () => {
    (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__ = true;
    invoke
      .mockResolvedValueOnce(null)
      .mockRejectedValueOnce(new Error("remote refresh unavailable"));

    await expect(applyPreset("ws-new", "preset", "current_terminal", null, null, true, "tab-original"))
      .resolves.toBeUndefined();
    expect(invoke).toHaveBeenCalledWith("get_app_state");
  });
});
