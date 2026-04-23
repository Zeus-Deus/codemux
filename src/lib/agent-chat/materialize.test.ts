import { beforeEach, describe, expect, it, vi } from "vitest";

import type { ChatDraft, DraftId } from "@/stores/chat-draft-store";
import type { TerminalPreset } from "@/tauri/types";

// ── Module mocks ──

vi.mock("@/tauri/commands", () => ({
  activateWorkspace: vi.fn().mockResolvedValue(undefined),
  agentChatCreatePane: vi.fn().mockResolvedValue("pane-new"),
  agentChatSendTurn: vi.fn().mockResolvedValue("turn-1"),
  agentChatStartSession: vi.fn().mockResolvedValue("thread-echoed"),
  applyPreset: vi.fn().mockResolvedValue(undefined),
  // `createEmptyWorkspace` is called with either `/home/user` (home
  // targets since Stage C) or a project path. Return ids that
  // reflect the caller so assertions can distinguish the two.
  createEmptyWorkspace: vi.fn((cwd: string) =>
    Promise.resolve(cwd === "/home/user" ? "ws-home" : "ws-project"),
  ),
  getHomeDir: vi.fn().mockResolvedValue("/home/user"),
  renameWorkspace: vi.fn().mockResolvedValue(undefined),
}));

import {
  materializeAndSend,
  materializeWithPreset,
  type MaterializeActions,
} from "./materialize";
import {
  activateWorkspace,
  agentChatCreatePane,
  agentChatSendTurn,
  agentChatStartSession,
  applyPreset,
  createEmptyWorkspace,
  getHomeDir,
  renameWorkspace,
} from "@/tauri/commands";
import { useAppStore } from "@/stores/app-store";

type SpyActions = {
  [K in keyof MaterializeActions]: ReturnType<typeof vi.fn>;
};

function makeActions(): SpyActions {
  return {
    markPromoting: vi.fn(),
    markPromoted: vi.fn(),
    markSendFailed: vi.fn(),
    ensureThread: vi.fn(),
    appendUserMessage: vi.fn(),
    setModel: vi.fn(),
    setPermissionMode: vi.fn(),
    setSessionLaunchMode: vi.fn(),
    setEffort: vi.fn(),
    setContextWindow: vi.fn(),
  };
}

function makeDraft(overrides: Partial<ChatDraft> = {}): ChatDraft {
  return {
    draftId: "draft-1" as DraftId,
    createdAt: "2026-01-01T00:00:00.000Z",
    target: { kind: "home" },
    provider: "claude",
    model: null,
    effort: null,
    contextWindow: null,
    permissionMode: "bypassPermissions",
    inputDraft: "hello",
    threadId: "pre-minted-thread-xyz",
    promotedTo: null,
    promoting: false,
    lastSendError: null,
    ...overrides,
  };
}

describe("materializeAndSend", () => {
  beforeEach(() => {
    vi.mocked(activateWorkspace).mockClear().mockResolvedValue(undefined);
    vi.mocked(agentChatCreatePane).mockClear().mockResolvedValue("pane-new");
    vi.mocked(agentChatSendTurn).mockClear().mockResolvedValue("turn-1");
    vi.mocked(agentChatStartSession)
      .mockClear()
      .mockResolvedValue("thread-echoed");
    vi.mocked(applyPreset).mockClear().mockResolvedValue(undefined);
    vi.mocked(createEmptyWorkspace)
      .mockClear()
      .mockImplementation((cwd: string) =>
        Promise.resolve(cwd === "/home/user" ? "ws-home" : "ws-project"),
      );
    vi.mocked(getHomeDir).mockClear().mockResolvedValue("/home/user");
    vi.mocked(renameWorkspace).mockClear().mockResolvedValue(undefined);
    // Stage C home-branch reads homeDir from the app-store cache.
    useAppStore.setState({ homeDir: "/home/user" });
  });

  describe("happy path", () => {
    it("creates a fresh workspace at $HOME with a message-derived title for a home draft", async () => {
      const actions = makeActions();
      const draft = makeDraft();

      const result = await materializeAndSend(draft, "hello world", "/home/user", actions);

      expect(result).toEqual({
        success: true,
        workspaceId: "ws-home",
        paneId: "pane-new",
        threadId: draft.threadId,
      });
      // Stage C: no more getOrCreateHomeWorkspace — home targets go
      // through createEmptyWorkspace(homeDir, { skipSetup: true })
      // plus a best-effort rename.
      expect(createEmptyWorkspace).toHaveBeenCalledWith("/home/user", {
        skipSetup: true,
      });
      expect(renameWorkspace).toHaveBeenCalledWith("ws-home", "hello world");
      expect(actions.markPromoted).toHaveBeenCalledWith(draft.draftId, {
        workspaceId: "ws-home",
        paneId: "pane-new",
        threadId: draft.threadId,
      });
      expect(actions.markSendFailed).not.toHaveBeenCalled();
      expect(agentChatCreatePane).toHaveBeenCalledWith(
        "ws-home",
        "claude",
        "/home/user",
      );
      expect(activateWorkspace).toHaveBeenCalledWith("ws-home");
    });

    it("uses createEmptyWorkspace without skipSetup for a project target, and does not rename", async () => {
      const actions = makeActions();
      const draft = makeDraft({
        target: { kind: "project", projectPath: "/projects/foo" },
      });

      const result = await materializeAndSend(
        draft,
        "hello",
        "/projects/foo",
        actions,
      );

      expect(result.success).toBe(true);
      expect(createEmptyWorkspace).toHaveBeenCalledWith("/projects/foo");
      // Project paths are named after the basename by default; no rename.
      expect(renameWorkspace).not.toHaveBeenCalled();
      expect(agentChatCreatePane).toHaveBeenCalledWith(
        "ws-project",
        "claude",
        "/projects/foo",
      );
    });

    it("skips workspace creation entirely for an existing_workspace target", async () => {
      const actions = makeActions();
      const draft = makeDraft({
        target: { kind: "existing_workspace", workspaceId: "ws-42" },
      });

      const result = await materializeAndSend(draft, "hi", "/ws/42/cwd", actions);

      expect(result.success).toBe(true);
      if (result.success) expect(result.workspaceId).toBe("ws-42");
      expect(createEmptyWorkspace).not.toHaveBeenCalled();
      expect(renameWorkspace).not.toHaveBeenCalled();
      expect(agentChatCreatePane).toHaveBeenCalledWith(
        "ws-42",
        "claude",
        "/ws/42/cwd",
      );
    });

    it("empty composer text leaves the home workspace with its default title (no rename call)", async () => {
      const actions = makeActions();
      const draft = makeDraft();
      // An empty or whitespace-only first message means
      // `deriveTitleFromFirstMessage` returns null — the caller must
      // skip the rename, keeping the path-derived basename.
      await materializeAndSend(draft, "   ", "/home/user", actions);
      expect(createEmptyWorkspace).toHaveBeenCalledWith("/home/user", {
        skipSetup: true,
      });
      expect(renameWorkspace).not.toHaveBeenCalled();
    });

    it("rename failure is non-fatal — workspace + session still proceed", async () => {
      vi.mocked(renameWorkspace).mockRejectedValueOnce(
        new Error("rename crashed"),
      );
      const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
      const actions = makeActions();
      const draft = makeDraft();

      const result = await materializeAndSend(
        draft,
        "title this",
        "/home/user",
        actions,
      );

      expect(result.success).toBe(true);
      expect(actions.markPromoted).toHaveBeenCalled();
      expect(actions.markSendFailed).not.toHaveBeenCalled();
      warnSpy.mockRestore();
    });

    it("home-target fails cleanly when the homeDir cache is not hydrated", async () => {
      useAppStore.setState({ homeDir: null });
      const actions = makeActions();
      const draft = makeDraft();

      const result = await materializeAndSend(
        draft,
        "hello",
        "/home/user",
        actions,
      );

      expect(result).toEqual({
        success: false,
        error: "Home directory not loaded yet",
      });
      expect(actions.markSendFailed).toHaveBeenCalledWith(
        "draft-1",
        "Home directory not loaded yet",
      );
      expect(createEmptyWorkspace).not.toHaveBeenCalled();
      expect(renameWorkspace).not.toHaveBeenCalled();
      expect(agentChatCreatePane).not.toHaveBeenCalled();
    });

    it("passes the draft's pre-minted thread_id to start_session unchanged", async () => {
      const actions = makeActions();
      const draft = makeDraft({ threadId: "my-custom-pre-minted-id" });

      await materializeAndSend(draft, "hello", "/home/user", actions);

      expect(agentChatStartSession).toHaveBeenCalledTimes(1);
      const [, , input] = vi.mocked(agentChatStartSession).mock.calls[0];
      expect(input.thread_id).toBe("my-custom-pre-minted-id");
      expect(input.cwd).toBe("/home/user");
      expect(input.resume_cursor).toBeNull();
    });

    it("threads session config (model, effort, contextWindow, permissionMode) through to start_session", async () => {
      const actions = makeActions();
      const draft = makeDraft({
        model: "claude-opus-4-7",
        effort: "high",
        contextWindow: "1m",
        permissionMode: "plan",
      });

      await materializeAndSend(draft, "hello", "/home/user", actions);

      const [, , input] = vi.mocked(agentChatStartSession).mock.calls[0];
      expect(input.model).toBe("claude-opus-4-7");
      expect(input.effort).toBe("high");
      expect(input.context_window).toBe("1m");
      expect(input.permission_mode).toBe("plan");
    });

    it("sends the turn with the pre-minted thread_id and the submitted text", async () => {
      const actions = makeActions();
      const draft = makeDraft({ threadId: "tid-42", effort: "high" });

      await materializeAndSend(draft, "first message", "/home/user", actions);

      expect(agentChatSendTurn).toHaveBeenCalledTimes(1);
      const [provider, input] = vi.mocked(agentChatSendTurn).mock.calls[0];
      expect(provider).toBe("claude");
      expect(input).toEqual({
        thread_id: "tid-42",
        text: "first message",
        model_override: null,
        effort_override: "high",
        permission_mode_override: null,
      });
    });

    it("does not fail when activateWorkspace rejects — treats it as non-fatal", async () => {
      vi.mocked(activateWorkspace).mockRejectedValueOnce(new Error("boom"));
      const actions = makeActions();
      const draft = makeDraft();
      const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});

      const result = await materializeAndSend(draft, "hi", "/home/user", actions);

      expect(result.success).toBe(true);
      expect(actions.markPromoted).toHaveBeenCalled();
      expect(actions.markSendFailed).not.toHaveBeenCalled();
      warnSpy.mockRestore();
    });
  });

  describe("optimistic message seeding", () => {
    it("calls ensureThread + appendUserMessage BEFORE startSession, with slice seeding between", async () => {
      const actions = makeActions();
      const order: string[] = [];
      actions.ensureThread.mockImplementation(() => {
        order.push("ensureThread");
      });
      actions.setModel.mockImplementation(() => {
        order.push("setModel");
      });
      actions.appendUserMessage.mockImplementation(() => {
        order.push("appendUserMessage");
      });
      vi.mocked(agentChatStartSession).mockImplementationOnce(async () => {
        order.push("startSession");
        return "thread-echoed";
      });

      await materializeAndSend(makeDraft(), "hello", "/home/user", actions);

      // Slice seed (at minimum setModel) must land between ensureThread
      // and appendUserMessage, so the append sees a fully-populated
      // slice and later subscribers (AgentChatPane) don't find a slice
      // with `model: null`.
      expect(order).toEqual([
        "ensureThread",
        "setModel",
        "appendUserMessage",
        "startSession",
      ]);
    });

    it("seeds the optimistic message using the pre-minted thread id", async () => {
      const actions = makeActions();
      const draft = makeDraft({ threadId: "tid-7" });

      await materializeAndSend(draft, "first turn text", "/home/user", actions);

      expect(actions.ensureThread).toHaveBeenCalledWith("tid-7");
      expect(actions.appendUserMessage).toHaveBeenCalledWith(
        "tid-7",
        "first turn text",
      );
    });

    it("still seeds the optimistic message when startSession fails afterwards", async () => {
      vi.mocked(agentChatStartSession).mockRejectedValueOnce(
        new Error("start failed"),
      );
      const actions = makeActions();

      await materializeAndSend(makeDraft(), "hello", "/home/user", actions);

      expect(actions.appendUserMessage).toHaveBeenCalledTimes(1);
    });
  });

  describe("slice seeding from draft (Stage C Effort-lock fix)", () => {
    it("mirrors model + permissionMode + sessionLaunchMode onto the slice", async () => {
      const actions = makeActions();
      const draft = makeDraft({
        model: "claude-opus-4-7",
        permissionMode: "plan",
      });

      await materializeAndSend(draft, "hi", "/home/user", actions);

      expect(actions.setModel).toHaveBeenCalledWith(
        draft.threadId,
        "claude-opus-4-7",
      );
      expect(actions.setPermissionMode).toHaveBeenCalledWith(
        draft.threadId,
        "plan",
      );
      expect(actions.setSessionLaunchMode).toHaveBeenCalledWith(
        draft.threadId,
        "plan",
      );
    });

    it("falls back to DEFAULT_THREAD_PERMISSION_MODE when draft.permissionMode is null", async () => {
      const actions = makeActions();
      const draft = makeDraft({ permissionMode: null });

      await materializeAndSend(draft, "hi", "/home/user", actions);

      expect(actions.setPermissionMode).toHaveBeenCalledWith(
        draft.threadId,
        "bypassPermissions",
      );
      expect(actions.setSessionLaunchMode).toHaveBeenCalledWith(
        draft.threadId,
        "bypassPermissions",
      );
    });

    it("forwards effort and contextWindow only when the draft has them set", async () => {
      const actions = makeActions();
      const draft = makeDraft({ effort: "high", contextWindow: "1m" });

      await materializeAndSend(draft, "hi", "/home/user", actions);

      expect(actions.setEffort).toHaveBeenCalledWith(draft.threadId, "high");
      expect(actions.setContextWindow).toHaveBeenCalledWith(
        draft.threadId,
        "1m",
      );
    });

    it("does not call setEffort / setContextWindow when the draft's values are null", async () => {
      const actions = makeActions();
      const draft = makeDraft({ effort: null, contextWindow: null });

      await materializeAndSend(draft, "hi", "/home/user", actions);

      expect(actions.setEffort).not.toHaveBeenCalled();
      expect(actions.setContextWindow).not.toHaveBeenCalled();
    });
  });

  describe("failure paths", () => {
    it("workspace create failure → markSendFailed, no downstream calls", async () => {
      vi.mocked(createEmptyWorkspace).mockRejectedValueOnce(
        new Error("workspace boom"),
      );
      const actions = makeActions();

      const result = await materializeAndSend(
        makeDraft(),
        "hi",
        "/home/user",
        actions,
      );

      expect(result).toEqual({ success: false, error: "workspace boom" });
      expect(actions.markSendFailed).toHaveBeenCalledWith(
        "draft-1",
        "workspace boom",
      );
      expect(actions.markPromoted).not.toHaveBeenCalled();
      expect(agentChatCreatePane).not.toHaveBeenCalled();
      expect(agentChatStartSession).not.toHaveBeenCalled();
      expect(agentChatSendTurn).not.toHaveBeenCalled();
      expect(activateWorkspace).not.toHaveBeenCalled();
      // Optimistic message was never seeded — the workspace never existed.
      expect(actions.appendUserMessage).not.toHaveBeenCalled();
    });

    it("pane create failure → markSendFailed, no session calls", async () => {
      vi.mocked(agentChatCreatePane).mockRejectedValueOnce(
        new Error("pane boom"),
      );
      const actions = makeActions();

      const result = await materializeAndSend(
        makeDraft(),
        "hi",
        "/home/user",
        actions,
      );

      expect(result).toEqual({ success: false, error: "pane boom" });
      expect(actions.markSendFailed).toHaveBeenCalledWith("draft-1", "pane boom");
      expect(actions.markPromoted).not.toHaveBeenCalled();
      expect(agentChatStartSession).not.toHaveBeenCalled();
      expect(agentChatSendTurn).not.toHaveBeenCalled();
      expect(activateWorkspace).not.toHaveBeenCalled();
      // The workspace was created but not rolled back — matches the
      // failure policy locked in for Stage C.
      expect(createEmptyWorkspace).toHaveBeenCalled();
    });

    it("start session failure → markSendFailed, optimistic message already seeded, no send_turn", async () => {
      vi.mocked(agentChatStartSession).mockRejectedValueOnce(
        new Error("session boom"),
      );
      const actions = makeActions();

      const result = await materializeAndSend(
        makeDraft(),
        "hi",
        "/home/user",
        actions,
      );

      expect(result).toEqual({ success: false, error: "session boom" });
      expect(actions.markSendFailed).toHaveBeenCalledWith(
        "draft-1",
        "session boom",
      );
      expect(actions.markPromoted).not.toHaveBeenCalled();
      // Optimistic write happened before the failing step.
      expect(actions.appendUserMessage).toHaveBeenCalledTimes(1);
      expect(agentChatSendTurn).not.toHaveBeenCalled();
      expect(activateWorkspace).not.toHaveBeenCalled();
    });

    it("send turn failure → markSendFailed, no markPromoted, no activateWorkspace", async () => {
      vi.mocked(agentChatSendTurn).mockRejectedValueOnce(
        new Error("turn boom"),
      );
      const actions = makeActions();

      const result = await materializeAndSend(
        makeDraft(),
        "hi",
        "/home/user",
        actions,
      );

      expect(result).toEqual({ success: false, error: "turn boom" });
      expect(actions.markSendFailed).toHaveBeenCalledWith("draft-1", "turn boom");
      expect(actions.markPromoted).not.toHaveBeenCalled();
      expect(activateWorkspace).not.toHaveBeenCalled();
      // Everything before the failing step ran.
      expect(agentChatCreatePane).toHaveBeenCalled();
      expect(agentChatStartSession).toHaveBeenCalled();
      expect(actions.appendUserMessage).toHaveBeenCalled();
    });

    it("extracts error.message for Error instances", async () => {
      vi.mocked(agentChatCreatePane).mockRejectedValueOnce(
        new Error("descriptive message"),
      );
      const actions = makeActions();

      const result = await materializeAndSend(
        makeDraft(),
        "hi",
        "/home/user",
        actions,
      );

      expect(result).toEqual({ success: false, error: "descriptive message" });
    });

    it("uses plain strings verbatim when Tauri rejects with a string", async () => {
      vi.mocked(agentChatCreatePane).mockRejectedValueOnce("backend said no");
      const actions = makeActions();

      const result = await materializeAndSend(
        makeDraft(),
        "hi",
        "/home/user",
        actions,
      );

      expect(result).toEqual({ success: false, error: "backend said no" });
    });
  });
});

// ── materializeWithPreset ──

function makePreset(overrides: Partial<TerminalPreset> = {}): TerminalPreset {
  return {
    id: "builtin-claude",
    name: "Claude Code",
    description: null,
    commands: ["claude --dangerously-skip-permissions"],
    working_directory: null,
    launch_mode: "new_tab",
    icon: "claude",
    pinned: true,
    is_builtin: true,
    auto_run_on_workspace: false,
    auto_run_on_new_tab: false,
    kind: "cli",
    ...overrides,
  };
}

describe("materializeWithPreset", () => {
  beforeEach(() => {
    vi.mocked(activateWorkspace).mockClear().mockResolvedValue(undefined);
    vi.mocked(agentChatCreatePane).mockClear().mockResolvedValue("pane-new");
    vi.mocked(agentChatSendTurn).mockClear().mockResolvedValue("turn-1");
    vi.mocked(agentChatStartSession)
      .mockClear()
      .mockResolvedValue("thread-echoed");
    vi.mocked(applyPreset).mockClear().mockResolvedValue(undefined);
    vi.mocked(createEmptyWorkspace)
      .mockClear()
      .mockImplementation((cwd: string) =>
        Promise.resolve(cwd === "/home/user" ? "ws-home" : "ws-project"),
      );
    vi.mocked(getHomeDir).mockClear().mockResolvedValue("/home/user");
    vi.mocked(renameWorkspace).mockClear().mockResolvedValue(undefined);
    // Stage C home-branch reads homeDir from the app-store cache.
    useAppStore.setState({ homeDir: "/home/user" });
  });

  describe("CLI preset dispatch", () => {
    it("happy path: resolves workspace, activates it, delegates to applyPreset with the prompt", async () => {
      const actions = makeActions();
      const draft = makeDraft({
        target: { kind: "project", projectPath: "/projects/foo" },
      });
      const preset = makePreset({ kind: "cli", id: "builtin-claude" });

      const result = await materializeWithPreset(draft, preset, "ship it", actions);

      expect(result.success).toBe(true);
      expect(createEmptyWorkspace).toHaveBeenCalledWith("/projects/foo");
      expect(activateWorkspace).toHaveBeenCalledWith("ws-project");
      expect(applyPreset).toHaveBeenCalledWith(
        "ws-project",
        "builtin-claude",
        "current_terminal",
        "ship it",
      );
      // CLI path does not mint its own chat pane / session / turn.
      expect(agentChatCreatePane).not.toHaveBeenCalled();
      expect(agentChatStartSession).not.toHaveBeenCalled();
      expect(agentChatSendTurn).not.toHaveBeenCalled();
      expect(actions.markPromoted).toHaveBeenCalled();
    });

    it("empty/whitespace prompt becomes null when delegated to applyPreset", async () => {
      const actions = makeActions();
      const draft = makeDraft({
        target: { kind: "project", projectPath: "/projects/foo" },
      });
      const preset = makePreset({ kind: "cli" });

      await materializeWithPreset(draft, preset, "   ", actions);

      expect(applyPreset).toHaveBeenCalledWith(
        "ws-project",
        "builtin-claude",
        "current_terminal",
        null,
      );
    });

    it("workspace-create failure → markSendFailed, applyPreset not called", async () => {
      vi.mocked(createEmptyWorkspace).mockRejectedValueOnce(
        new Error("fs denied"),
      );
      const actions = makeActions();
      const draft = makeDraft({
        target: { kind: "project", projectPath: "/projects/foo" },
      });
      const preset = makePreset({ kind: "cli" });

      const result = await materializeWithPreset(draft, preset, "hi", actions);

      expect(result).toEqual({ success: false, error: "fs denied" });
      expect(actions.markSendFailed).toHaveBeenCalledWith("draft-1", "fs denied");
      expect(applyPreset).not.toHaveBeenCalled();
      expect(activateWorkspace).not.toHaveBeenCalled();
      expect(actions.markPromoted).not.toHaveBeenCalled();
    });

    it("applyPreset failure → markSendFailed, no markPromoted", async () => {
      vi.mocked(applyPreset).mockRejectedValueOnce(
        new Error("claude is not installed"),
      );
      const actions = makeActions();
      const draft = makeDraft({
        target: { kind: "project", projectPath: "/projects/foo" },
      });
      const preset = makePreset({ kind: "cli" });

      const result = await materializeWithPreset(draft, preset, "hi", actions);

      expect(result).toEqual({
        success: false,
        error: "claude is not installed",
      });
      expect(actions.markSendFailed).toHaveBeenCalledWith(
        "draft-1",
        "claude is not installed",
      );
      expect(actions.markPromoted).not.toHaveBeenCalled();
      // The workspace was created but not rolled back (locked policy).
      expect(createEmptyWorkspace).toHaveBeenCalled();
      expect(activateWorkspace).toHaveBeenCalled();
    });
  });

  describe("ChatAgent preset dispatch", () => {
    it("happy path with prompt: creates pane, seeds transcript, starts session, sends turn", async () => {
      const actions = makeActions();
      const draft = makeDraft({
        target: { kind: "project", projectPath: "/projects/foo" },
      });
      const preset = makePreset({
        kind: "chat_agent",
        id: "builtin-chat-agent",
        commands: [],
      });

      const result = await materializeWithPreset(
        draft,
        preset,
        "first message",
        actions,
      );

      expect(result.success).toBe(true);
      expect(createEmptyWorkspace).toHaveBeenCalledWith("/projects/foo");
      expect(activateWorkspace).toHaveBeenCalledWith("ws-project");
      expect(agentChatCreatePane).toHaveBeenCalledWith(
        "ws-project",
        "claude",
        "/projects/foo",
      );
      expect(actions.ensureThread).toHaveBeenCalledWith(draft.threadId);
      expect(actions.appendUserMessage).toHaveBeenCalledWith(
        draft.threadId,
        "first message",
      );
      const [, , startInput] = vi.mocked(agentChatStartSession).mock.calls[0];
      expect(startInput.thread_id).toBe(draft.threadId);
      expect(agentChatSendTurn).toHaveBeenCalledTimes(1);
      // CLI path must not fire for a ChatAgent preset.
      expect(applyPreset).not.toHaveBeenCalled();
      expect(actions.markPromoted).toHaveBeenCalled();
    });

    it("resolves home cwd via getHomeDir + creates workspace at $HOME with a title for home-target drafts", async () => {
      const actions = makeActions();
      const draft = makeDraft({ target: { kind: "home" } });
      const preset = makePreset({ kind: "chat_agent" });

      await materializeWithPreset(draft, preset, "hi", actions);

      // Cwd for the agent session still comes from `getHomeDir()`
      // (via `resolveCwdForTarget`).
      expect(getHomeDir).toHaveBeenCalled();
      // Workspace is created fresh under $HOME (no more
      // getOrCreateHomeWorkspace) and renamed to the prompt.
      expect(createEmptyWorkspace).toHaveBeenCalledWith("/home/user", {
        skipSetup: true,
      });
      expect(renameWorkspace).toHaveBeenCalledWith("ws-home", "hi");
      expect(agentChatCreatePane).toHaveBeenCalledWith(
        "ws-home",
        "claude",
        "/home/user",
      );
    });

    it("home-target ChatAgent preset fails cleanly when homeDir cache is not hydrated", async () => {
      useAppStore.setState({ homeDir: null });
      const actions = makeActions();
      const draft = makeDraft({ target: { kind: "home" } });
      const preset = makePreset({ kind: "chat_agent" });

      const result = await materializeWithPreset(draft, preset, "hi", actions);

      expect(result).toEqual({
        success: false,
        error: "Home directory not loaded yet",
      });
      expect(actions.markSendFailed).toHaveBeenCalled();
      expect(createEmptyWorkspace).not.toHaveBeenCalled();
      expect(agentChatCreatePane).not.toHaveBeenCalled();
    });

    it("empty prompt: creates pane + starts session but does NOT fire send_turn", async () => {
      const actions = makeActions();
      const draft = makeDraft({
        target: { kind: "project", projectPath: "/projects/foo" },
      });
      const preset = makePreset({ kind: "chat_agent" });

      const result = await materializeWithPreset(draft, preset, "", actions);

      expect(result.success).toBe(true);
      expect(agentChatCreatePane).toHaveBeenCalled();
      expect(agentChatStartSession).toHaveBeenCalled();
      expect(agentChatSendTurn).not.toHaveBeenCalled();
      expect(actions.appendUserMessage).not.toHaveBeenCalled();
      // ensureThread still runs so AgentChatPane picks up a clean slice.
      expect(actions.ensureThread).toHaveBeenCalledWith(draft.threadId);
    });

    it("pane-create failure → markSendFailed, no session / no send", async () => {
      vi.mocked(agentChatCreatePane).mockRejectedValueOnce(
        new Error("pane boom"),
      );
      const actions = makeActions();
      const draft = makeDraft({
        target: { kind: "project", projectPath: "/projects/foo" },
      });
      const preset = makePreset({ kind: "chat_agent" });

      const result = await materializeWithPreset(draft, preset, "hi", actions);

      expect(result).toEqual({ success: false, error: "pane boom" });
      expect(actions.markSendFailed).toHaveBeenCalledWith("draft-1", "pane boom");
      expect(agentChatStartSession).not.toHaveBeenCalled();
      expect(agentChatSendTurn).not.toHaveBeenCalled();
      expect(actions.markPromoted).not.toHaveBeenCalled();
    });

    it("start-session failure after pane created → optimistic message already seeded", async () => {
      vi.mocked(agentChatStartSession).mockRejectedValueOnce(
        new Error("session boom"),
      );
      const actions = makeActions();
      const draft = makeDraft({
        target: { kind: "project", projectPath: "/projects/foo" },
      });
      const preset = makePreset({ kind: "chat_agent" });

      await materializeWithPreset(draft, preset, "hi", actions);

      // The optimistic user message is seeded BEFORE start_session.
      expect(actions.appendUserMessage).toHaveBeenCalledWith(
        draft.threadId,
        "hi",
      );
      expect(agentChatSendTurn).not.toHaveBeenCalled();
    });
  });

  describe("slice seeding from draft (Stage C Effort-lock fix)", () => {
    it("ChatAgent preset seeds slice config even when the composer text is empty", async () => {
      const actions = makeActions();
      const draft = makeDraft({
        target: { kind: "project", projectPath: "/projects/foo" },
        model: "claude-opus-4-7",
        permissionMode: "plan",
        effort: "high",
        contextWindow: "1m",
      });
      const preset = makePreset({ kind: "chat_agent", commands: [] });

      await materializeWithPreset(draft, preset, "", actions);

      // Slice config lands even without a first turn — the picker
      // state must be available as soon as the pane mounts.
      expect(actions.setModel).toHaveBeenCalledWith(
        draft.threadId,
        "claude-opus-4-7",
      );
      expect(actions.setPermissionMode).toHaveBeenCalledWith(
        draft.threadId,
        "plan",
      );
      expect(actions.setSessionLaunchMode).toHaveBeenCalledWith(
        draft.threadId,
        "plan",
      );
      expect(actions.setEffort).toHaveBeenCalledWith(draft.threadId, "high");
      expect(actions.setContextWindow).toHaveBeenCalledWith(
        draft.threadId,
        "1m",
      );
      expect(actions.appendUserMessage).not.toHaveBeenCalled();
    });
  });

  describe("activateWorkspace is non-fatal for either dispatch", () => {
    it("CLI: preset still dispatches when activateWorkspace rejects", async () => {
      vi.mocked(activateWorkspace).mockRejectedValueOnce(new Error("boom"));
      const actions = makeActions();
      const draft = makeDraft({
        target: { kind: "project", projectPath: "/projects/foo" },
      });
      const preset = makePreset({ kind: "cli" });
      const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});

      const result = await materializeWithPreset(draft, preset, "hi", actions);

      expect(result.success).toBe(true);
      expect(applyPreset).toHaveBeenCalled();
      warnSpy.mockRestore();
    });

    it("ChatAgent: preset still dispatches when activateWorkspace rejects", async () => {
      vi.mocked(activateWorkspace).mockRejectedValueOnce(new Error("boom"));
      const actions = makeActions();
      const draft = makeDraft({
        target: { kind: "project", projectPath: "/projects/foo" },
      });
      const preset = makePreset({ kind: "chat_agent" });
      const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});

      const result = await materializeWithPreset(draft, preset, "hi", actions);

      expect(result.success).toBe(true);
      expect(agentChatCreatePane).toHaveBeenCalled();
      warnSpy.mockRestore();
    });
  });
});
