import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const { invoke, listen, toastError, toastInfo } = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn(async () => () => {}),
  toastError: vi.fn(),
  toastInfo: vi.fn(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen }));
vi.mock("@/lib/toast", () => ({ toast: { error: toastError, info: toastInfo } }));

import { useAgentChatStore } from "@/stores/agent-chat-store";
import { useUIStore } from "@/stores/ui-store";

import {
  parseBtwCommand,
  sendSideChat,
  sideChatSupported,
  sideChatUnavailableMessage,
  startSideChat,
  useSideChatStore,
} from "./side-chat";
import { askSideQuestion, withBtwPrefix } from "./side-chat-launch";

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

beforeEach(() => {
  invoke.mockReset();
  invoke.mockResolvedValue(undefined);
  toastError.mockReset();
  toastInfo.mockReset();
  useSideChatStore.setState({ chats: {}, activeByWorkspace: {} });
});
afterEach(() => {
  useSideChatStore.setState({ chats: {}, activeByWorkspace: {} });
});

describe("parseBtwCommand", () => {
  it("reads the question after /btw", () => {
    expect(parseBtwCommand("/btw why is this slow?")).toEqual({ question: "why is this slow?" });
    expect(parseBtwCommand("  /BTW  multi\nline ")).toEqual({ question: "multi\nline" });
    expect(parseBtwCommand("/btw")).toEqual({ question: "" });
  });

  it("ignores anything that isn't a leading /btw command", () => {
    expect(parseBtwCommand("/btwx hi")).toBeNull();
    expect(parseBtwCommand("hey /btw hi")).toBeNull();
    expect(parseBtwCommand("/workflow hi")).toBeNull();
  });
});

describe("provider support", () => {
  it("only Claude and Codex fork side chats", () => {
    expect(sideChatSupported("claude")).toBe(true);
    expect(sideChatSupported("codex")).toBe(true);
    for (const provider of ["cursor", "grok", "opencode", "hermes", "acp"] as const) {
      expect(sideChatSupported(provider)).toBe(false);
    }
    expect(sideChatUnavailableMessage("cursor")).toBe("Side chat isn't available for Cursor yet.");
    expect(sideChatUnavailableMessage("acp")).toBe("Side chat isn't available for this agent yet.");
  });
});

describe("withBtwPrefix", () => {
  it("adds /btw once", () => {
    expect(withBtwPrefix("")).toBe("/btw ");
    expect(withBtwPrefix("  what now")).toBe("/btw what now");
    expect(withBtwPrefix("/btw already")).toBe("/btw already");
  });
});

describe("askSideQuestion", () => {
  const base = { threadId: "t-1", provider: "claude" as const, workspaceId: "ws-1" };

  it("is not involved for ordinary messages", () => {
    expect(askSideQuestion({ ...base, text: "hello" })).toBeNull();
    expect(invoke).not.toHaveBeenCalled();
  });

  it("starts a side chat and opens the pane without sending to the main thread", async () => {
    const setRightPanelTab = vi.spyOn(useUIStore.getState(), "setRightPanelTab");
    expect(askSideQuestion({ ...base, text: "/btw what changed?" })).toBe("started");
    await flush();
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledWith("agent_chat_side_chat_start", {
      sideId: expect.any(String),
      threadId: "t-1",
      text: "what changed?",
    });
    expect(invoke.mock.calls.some(([cmd]) => cmd === "agent_chat_send_turn")).toBe(false);
    expect(setRightPanelTab).toHaveBeenCalledWith("ws-1", "side-chat");
    setRightPanelTab.mockRestore();
  });

  it("refuses unsupported providers, empty questions and unstarted chats", () => {
    expect(askSideQuestion({ ...base, provider: "cursor", text: "/btw x" })).toBe("refused");
    expect(toastInfo).toHaveBeenCalledWith("Side chat isn't available for Cursor yet.");
    expect(askSideQuestion({ ...base, text: "/btw" })).toBe("refused");
    expect(toastError).toHaveBeenCalled();
    expect(askSideQuestion({ ...base, threadId: null, text: "/btw x" })).toBe("refused");
    expect(invoke).not.toHaveBeenCalled();
  });
});

describe("side chat store", () => {
  it("streams deltas, completes, and continues the same fork", async () => {
    const sideId = await startSideChat({ threadId: "t-1", workspaceId: "ws-1", provider: "codex", question: "q1" });
    const apply = useSideChatStore.getState().apply;
    apply({ side_id: sideId, kind: "delta", text: "Hel" });
    apply({ side_id: sideId, kind: "delta", text: "lo" });
    apply({ side_id: sideId, kind: "turn_completed", text: "ignored when deltas streamed" });
    let chat = useSideChatStore.getState().chats[sideId]!;
    expect(chat.turns).toEqual([{ question: "q1", answer: "Hello", status: "done" }]);

    await sendSideChat(sideId, "q2");
    expect(invoke).toHaveBeenLastCalledWith("agent_chat_side_chat_send", { sideId, text: "q2" });
    apply({ side_id: sideId, kind: "turn_completed", text: "Final" });
    chat = useSideChatStore.getState().chats[sideId]!;
    expect(chat.turns[1]).toEqual({ question: "q2", answer: "Final", status: "done" });
  });

  it("names the main chat's model and effort, then takes the backend's report", async () => {
    useAgentChatStore.getState().ensureThread("t-model");
    useAgentChatStore.getState().setModel("t-model", "claude-opus-4-8");
    useAgentChatStore.getState().setEffort("t-model", "high");
    let release!: (value: unknown) => void;
    invoke.mockImplementationOnce(() => new Promise((resolve) => (release = resolve)));
    const pending = startSideChat({ threadId: "t-model", workspaceId: "ws-1", provider: "claude", question: "q" });
    await flush();
    const sideId = useSideChatStore.getState().activeByWorkspace["ws-1"]!;
    expect(useSideChatStore.getState().chats[sideId]).toMatchObject({ model: "claude-opus-4-8", effort: "high" });
    release({ model: "claude-opus-5-5", effort: "max" });
    await pending;
    expect(useSideChatStore.getState().chats[sideId]).toMatchObject({ model: "claude-opus-5-5", effort: "max" });
  });

  it("a backend default keeps the thread's own model name", async () => {
    useAgentChatStore.getState().ensureThread("t-default");
    useAgentChatStore.getState().setModel("t-default", "gpt-5.6-sol");
    invoke.mockResolvedValueOnce({ model: null, effort: null });
    const sideId = await startSideChat({ threadId: "t-default", workspaceId: "ws-1", provider: "codex", question: "q" });
    expect(useSideChatStore.getState().chats[sideId]).toMatchObject({ model: "gpt-5.6-sol", effort: null });
  });

  it("a new /btw replaces the open side chat and closes its fork", async () => {
    const first = await startSideChat({ threadId: "t-1", workspaceId: "ws-1", provider: "claude", question: "a" });
    const second = await startSideChat({ threadId: "t-1", workspaceId: "ws-1", provider: "claude", question: "b" });
    await flush();
    expect(invoke).toHaveBeenCalledWith("agent_chat_side_chat_close", { sideId: first });
    const state = useSideChatStore.getState();
    expect(state.activeByWorkspace["ws-1"]).toBe(second);
    expect(state.chats[first]).toBeUndefined();
  });

  it("a failed start shows the backend's reason and ends the chat", async () => {
    invoke.mockRejectedValueOnce("This chat has no provider session yet.");
    const sideId = await startSideChat({ threadId: "t-1", workspaceId: "ws-1", provider: "claude", question: "a" });
    const chat = useSideChatStore.getState().chats[sideId]!;
    expect(chat.ended).toBe(true);
    expect(chat.turns[0]).toMatchObject({ status: "error", error: "This chat has no provider session yet." });
  });

  it("a fork that exits mid-answer is reported, and sends stop", async () => {
    const sideId = await startSideChat({ threadId: "t-1", workspaceId: "ws-1", provider: "claude", question: "a" });
    useSideChatStore.getState().apply({ side_id: sideId, kind: "closed" });
    const chat = useSideChatStore.getState().chats[sideId]!;
    expect(chat.ended).toBe(true);
    expect(chat.turns[0]!.status).toBe("error");
    invoke.mockClear();
    await sendSideChat(sideId, "more");
    expect(invoke).not.toHaveBeenCalled();
  });
});
