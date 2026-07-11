/// <reference types="@testing-library/jest-dom/vitest" />
import { beforeEach, describe, expect, it, vi } from "vitest";

// ── Mocks ────────────────────────────────────────────────────────────

const createPaneMock = vi.fn();
const startSessionMock = vi.fn();
vi.mock("@/tauri/commands", () => ({
  agentChatCreatePane: (...args: unknown[]) => createPaneMock(...args),
  agentChatStartSession: (...args: unknown[]) => startSessionMock(...args),
}));

// The cwd-resolution race lives in `./wait-for-workspace-cwd` and has
// its own dedicated tests (`wait-for-workspace-cwd.test.ts`). Here we
// mock it so these tests exercise prestart's own logic in isolation:
// a resolved cwd → full prestart; a `null` (genuine timeout) → the
// mount-effect fallback contract.
const waitForWorkspaceCwdMock = vi.fn();
vi.mock("./wait-for-workspace-cwd", () => ({
  waitForWorkspaceCwd: (...args: unknown[]) => waitForWorkspaceCwdMock(...args),
}));

const ensureThreadMock = vi.fn();
const setPermissionModeMock = vi.fn();
const setSessionLaunchModeMock = vi.fn();
const setModelMock = vi.fn();
const setEffortMock = vi.fn();
const setContextWindowMock = vi.fn();
const setModeMock = vi.fn();
vi.mock("@/stores/agent-chat-store", () => ({
  DEFAULT_THREAD_PERMISSION_MODE: "bypassPermissions",
  useAgentChatStore: {
    getState: () => ({
      ensureThread: ensureThreadMock,
      setPermissionMode: setPermissionModeMock,
      setSessionLaunchMode: setSessionLaunchModeMock,
      setModel: setModelMock,
      setEffort: setEffortMock,
      setContextWindow: setContextWindowMock,
      setMode: setModeMock,
    }),
  },
}));

import { prestartWorktreeSession } from "./prestart-worktree-session";

describe("prestartWorktreeSession", () => {
  beforeEach(() => {
    waitForWorkspaceCwdMock.mockReset();
    // Default: the worktree cwd resolves. Individual tests override the
    // resolved value or set it to `null` to exercise the timeout path.
    waitForWorkspaceCwdMock.mockResolvedValue("/tmp/worktree-feat");
    createPaneMock.mockReset();
    startSessionMock.mockReset();
    ensureThreadMock.mockReset();
    setPermissionModeMock.mockReset();
    setSessionLaunchModeMock.mockReset();
    setModelMock.mockReset();
    setEffortMock.mockReset();
    setContextWindowMock.mockReset();
    setModeMock.mockReset();
    createPaneMock.mockResolvedValue("pane-1");
    startSessionMock.mockResolvedValue("thread-echo");
  });

  it("passes the workspace's cwd to agent_chat_create_pane (NOT null)", async () => {
    // Regression: passing null leaves `pane.cwd` null and makes the
    // mount-effect's `if (!cwd) return` guard fire, which is the
    // original race that produced session_not_found.
    waitForWorkspaceCwdMock.mockResolvedValue("/tmp/worktree-feat");
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
    waitForWorkspaceCwdMock.mockResolvedValue("/tmp/worktree-feat");
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
    waitForWorkspaceCwdMock.mockResolvedValue("/tmp/w");
    await prestartWorktreeSession("ws-new");
    expect(events).toEqual(["create_pane", "start_session"]);
  });

  it("seeds the agent-chat slice with the minted thread_id + permission mode", async () => {
    // AgentChatPane's mount-effect adopts pane.thread_id and calls
    // ensureThread; pickers read from the slice. Seeding here means
    // the live pane mounts against a populated slice, not a bare one.
    waitForWorkspaceCwdMock.mockResolvedValue("/tmp/w");
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
    waitForWorkspaceCwdMock.mockResolvedValue("/tmp/w");
    await prestartWorktreeSession("ws-new");
    const startInput = startSessionMock.mock.calls[0][2];
    const seededThreadId = ensureThreadMock.mock.calls[0][0];
    expect(startInput.thread_id).toBe(seededThreadId);
  });

  it("defers to the mount-effect path when the workspace isn't in the store yet (fallback)", async () => {
    // Edge case: the cwd never reached the store within the timeout
    // (`waitForWorkspaceCwd` resolves null). Rather than fabricating a
    // bad cwd (sidecar would spawn in the wrong directory), we skip and
    // let AgentChatPane's mount-effect handle it. No worse than the
    // pre-fix behavior.
    waitForWorkspaceCwdMock.mockResolvedValue(null);
    await prestartWorktreeSession("ws-missing");
    expect(createPaneMock).not.toHaveBeenCalled();
    expect(startSessionMock).not.toHaveBeenCalled();
    expect(ensureThreadMock).not.toHaveBeenCalled();
  });

  it("propagates backend errors from agent_chat_start_session (so the caller can log)", async () => {
    waitForWorkspaceCwdMock.mockResolvedValue("/tmp/w");
    startSessionMock.mockRejectedValueOnce(new Error("sidecar spawn failed"));
    await expect(prestartWorktreeSession("ws-new")).rejects.toThrow(
      "sidecar spawn failed",
    );
    // create_pane still ran; the caller's catch block logs and then
    // the caller continues with activateWorkspace — failure is
    // non-blocking for the rest of the flow.
    expect(createPaneMock).toHaveBeenCalledTimes(1);
  });

  // ── Thread Scope deferred-worktree extensions ──

  it("returns { paneId, threadId } matching the created pane + session (and null on the store fallback)", async () => {
    waitForWorkspaceCwdMock.mockResolvedValue("/tmp/w");
    const result = await prestartWorktreeSession("ws-new");
    expect(result).not.toBeNull();
    expect(result!.paneId).toBe("pane-1");
    const startInput = startSessionMock.mock.calls[0][2];
    expect(result!.threadId).toBe(startInput.thread_id);

    waitForWorkspaceCwdMock.mockResolvedValue(null);
    const missing = await prestartWorktreeSession("ws-missing");
    expect(missing).toBeNull();
  });

  it("plumbs the optional config into start_session and the slice seed", async () => {
    waitForWorkspaceCwdMock.mockResolvedValue("/tmp/w");
    const result = await prestartWorktreeSession("ws-new", {
      model: "claude-opus-4-7",
      permissionMode: "plan",
      effort: "high",
      contextWindow: "1m",
      mode: "plan",
    });
    const [, , input] = startSessionMock.mock.calls[0];
    expect(input.model).toBe("claude-opus-4-7");
    expect(input.permission_mode).toBe("plan");
    expect(input.effort).toBe("high");
    expect(input.context_window).toBe("1m");
    const tid = result!.threadId;
    expect(setModelMock).toHaveBeenCalledWith(tid, "claude-opus-4-7");
    expect(setEffortMock).toHaveBeenCalledWith(tid, "high");
    expect(setContextWindowMock).toHaveBeenCalledWith(tid, "1m");
    expect(setModeMock).toHaveBeenCalledWith(tid, "plan");
    expect(setPermissionModeMock).toHaveBeenCalledWith(tid, "plan");
    expect(setSessionLaunchModeMock).toHaveBeenCalledWith(tid, "plan");
  });

  it("first-send succeeds with the worktree cwd even when it resolves after a delay", async () => {
    // Regression (PR #142 deferred-worktree cwd bug): the store often hasn't landed the new
    // worktree by the time prestart runs. `waitForWorkspaceCwd` awaits
    // it; prestart must then launch the pane + session at the WORKTREE
    // cwd, not return null on a routine store miss.
    waitForWorkspaceCwdMock.mockImplementationOnce(
      () =>
        new Promise((r) => setTimeout(() => r("/tmp/worktree-delayed"), 10)),
    );
    const result = await prestartWorktreeSession("ws-new");
    expect(waitForWorkspaceCwdMock).toHaveBeenCalledWith("ws-new");
    expect(result).not.toBeNull();
    expect(createPaneMock).toHaveBeenCalledWith(
      "ws-new",
      "claude",
      "/tmp/worktree-delayed",
    );
    const [, , input] = startSessionMock.mock.calls[0];
    expect(input.cwd).toBe("/tmp/worktree-delayed");
  });

  it("omitted config keeps the legacy defaults and skips the optional slice setters", async () => {
    waitForWorkspaceCwdMock.mockResolvedValue("/tmp/w");
    await prestartWorktreeSession("ws-new");
    const [, , input] = startSessionMock.mock.calls[0];
    expect(input.model).toBeNull();
    expect(input.permission_mode).toBe("bypassPermissions");
    expect(setModelMock).not.toHaveBeenCalled();
    expect(setEffortMock).not.toHaveBeenCalled();
    expect(setContextWindowMock).not.toHaveBeenCalled();
    expect(setModeMock).not.toHaveBeenCalled();
  });
});
