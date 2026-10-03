import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  agentChatCreatePane,
  createEmptyWorkspace,
  createWorktreeWorkspaceResult,
} from "./commands";

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
