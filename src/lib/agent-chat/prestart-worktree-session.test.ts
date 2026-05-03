/// <reference types="@testing-library/jest-dom/vitest" />
import { beforeEach, describe, expect, it, vi } from "vitest";

// ── Mocks ────────────────────────────────────────────────────────────

const createPaneMock = vi.fn();
const startSessionMock = vi.fn();
vi.mock("@/tauri/commands", () => ({
  agentChatCreatePane: (...args: unknown[]) => createPaneMock(...args),
  agentChatStartSession: (...args: unknown[]) => startSessionMock(...args),
}));

let workspacesSnapshot: Array<{ workspace_id: string; cwd: string }> = [];
vi.mock("@/stores/app-store", () => ({
  useAppStore: {
    getState: () => ({
      appState: { workspaces: workspacesSnapshot },
    }),
  },
}));

const ensureThreadMock = vi.fn();
const setPermissionModeMock = vi.fn();
const setSessionLaunchModeMock = vi.fn();
vi.mock("@/stores/agent-chat-store", () => ({
  DEFAULT_THREAD_PERMISSION_MODE: "bypassPermissions",
  useAgentChatStore: {
    getState: () => ({
      ensureThread: ensureThreadMock,
      setPermissionMode: setPermissionModeMock,
      setSessionLaunchMode: setSessionLaunchModeMock,
    }),
  },
}));

import { prestartWorktreeSession } from "./prestart-worktree-session";

describe("prestartWorktreeSession", () => {
  beforeEach(() => {
    workspacesSnapshot = [];
    createPaneMock.mockReset();
    startSessionMock.mockReset();
    ensureThreadMock.mockReset();
    setPermissionModeMock.mockReset();
    setSessionLaunchModeMock.mockReset();
    createPaneMock.mockResolvedValue("pane-1");
    startSessionMock.mockResolvedValue("thread-echo");
  });

  it("passes the workspace's cwd to agent_chat_create_pane (NOT null)", async () => {
    // Regression: passing null leaves `pane.cwd` null and makes the
    // mount-effect's `if (!cwd) return` guard fire, which is the
    // original race that produced session_not_found.
    workspacesSnapshot = [{ workspace_id: "ws-new", cwd: "/tmp/worktree-feat" }];
    await prestartWorktreeSession("ws-new");
    expect(createPaneMock).toHaveBeenCalledTimes(1);
    expect(createPaneMock).toHaveBeenCalledWith(
      "ws-new",
      "claude",
      "/tmp/worktree-feat",
    );
  });

  it("awaits agent_chat_start_session with a pre-minted thread_id BEFORE returning", async () => {
    // The contract: by the time this helper's promise resolves, the
    // adapter's HashMap holds the thread_id. Callers activate the
    // workspace afterward so AgentChatPane mounts with a live
    // session already in place.
    workspacesSnapshot = [{ workspace_id: "ws-new", cwd: "/tmp/worktree-feat" }];
    await prestartWorktreeSession("ws-new");
    expect(startSessionMock).toHaveBeenCalledTimes(1);
    const [paneId, provider, input] = startSessionMock.mock.calls[0];
    expect(paneId).toBe("pane-1");
    expect(provider).toBe("claude");
    expect(typeof input.thread_id).toBe("string");
    expect(input.thread_id.length).toBeGreaterThan(0);
    expect(input.cwd).toBe("/tmp/worktree-feat");
    expect(input.permission_mode).toBe("bypassPermissions");
    expect(input.resume_cursor).toBeNull();
    expect(input.additional_directories).toEqual([]);
  });

  it("calls create_pane BEFORE start_session (ordering is load-bearing)", async () => {
    // start_session references the pane_id returned by create_pane.
    // Reversing the order would fail the Tauri invoke.
    const events: string[] = [];
    createPaneMock.mockImplementation(async () => {
      events.push("create_pane");
      return "pane-1";
    });
    startSessionMock.mockImplementation(async () => {
      events.push("start_session");
      return "thread-echo";
    });
    workspacesSnapshot = [{ workspace_id: "ws-new", cwd: "/tmp/w" }];
    await prestartWorktreeSession("ws-new");
    expect(events).toEqual(["create_pane", "start_session"]);
  });

  it("seeds the agent-chat slice with the minted thread_id + permission mode", async () => {
    // AgentChatPane's mount-effect adopts pane.thread_id and calls
    // ensureThread; pickers read from the slice. Seeding here means
    // the live pane mounts against a populated slice, not a bare one.
    workspacesSnapshot = [{ workspace_id: "ws-new", cwd: "/tmp/w" }];
    await prestartWorktreeSession("ws-new");
    expect(ensureThreadMock).toHaveBeenCalledTimes(1);
    expect(setPermissionModeMock).toHaveBeenCalledTimes(1);
    expect(setSessionLaunchModeMock).toHaveBeenCalledTimes(1);
    const seededThreadId = ensureThreadMock.mock.calls[0][0];
    expect(setPermissionModeMock).toHaveBeenCalledWith(
      seededThreadId,
      "bypassPermissions",
    );
    expect(setSessionLaunchModeMock).toHaveBeenCalledWith(
      seededThreadId,
      "bypassPermissions",
    );
  });

  it("reuses the SAME thread_id across start_session and the slice seed", async () => {
    workspacesSnapshot = [{ workspace_id: "ws-new", cwd: "/tmp/w" }];
    await prestartWorktreeSession("ws-new");
    const startInput = startSessionMock.mock.calls[0][2];
    const seededThreadId = ensureThreadMock.mock.calls[0][0];
    expect(startInput.thread_id).toBe(seededThreadId);
  });

  it("defers to the mount-effect path when the workspace isn't in the store yet (fallback)", async () => {
    // Edge case: Tauri event delivery hasn't landed the new
    // workspace by the time we run. Rather than fabricating a bad
    // cwd (sidecar would spawn in the wrong directory), we skip and
    // let AgentChatPane's mount-effect handle it. No worse than the
    // pre-fix behavior.
    workspacesSnapshot = [];
    await prestartWorktreeSession("ws-missing");
    expect(createPaneMock).not.toHaveBeenCalled();
    expect(startSessionMock).not.toHaveBeenCalled();
    expect(ensureThreadMock).not.toHaveBeenCalled();
  });

  it("propagates backend errors from agent_chat_start_session (so the caller can log)", async () => {
    workspacesSnapshot = [{ workspace_id: "ws-new", cwd: "/tmp/w" }];
    startSessionMock.mockRejectedValueOnce(new Error("sidecar spawn failed"));
    await expect(prestartWorktreeSession("ws-new")).rejects.toThrow(
      "sidecar spawn failed",
    );
    // create_pane still ran; the caller's catch block logs and then
    // the caller continues with activateWorkspace — failure is
    // non-blocking for the rest of the flow.
    expect(createPaneMock).toHaveBeenCalledTimes(1);
  });
});
