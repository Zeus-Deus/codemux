import { beforeEach, describe, expect, it, vi } from "vitest";

const { invoke, toastSuccess, toastFailure, activate } = vi.hoisted(() => ({
  invoke: vi.fn(),
  toastSuccess: vi.fn(),
  toastFailure: vi.fn(),
  activate: vi.fn(async () => {}),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke, Channel: class {} }));
vi.mock("@/lib/toast", () => ({
  toast: { success: toastSuccess, failure: toastFailure },
  errorMessage: (error: unknown) => String(error),
}));
vi.mock("@/lib/perf/instrumented-activate", () => ({ activateWorkspaceInteraction: activate }));

import type { ChatViewItem } from "@/lib/agent-chat/types";
import type { AgentChatTurnCheckpointRecord } from "@/tauri/commands";

import { chatForkSupported, computeForkPoints, forkChat } from "./chat-fork";

const user = (id: string, row: number | undefined, extra: Partial<ChatViewItem> = {}): ChatViewItem =>
  ({ kind: "user_message", id, seq: 0, text: id, source_event_id: row, clientNonce: `n-${id}`, ...extra }) as ChatViewItem;
const reply = (id: string, extra: Partial<ChatViewItem> = {}): ChatViewItem =>
  ({ kind: "assistant_message", id, seq: 0, turn_id: "t", text: "ok", streaming: false, ...extra }) as ChatViewItem;

const TRANSCRIPT: ChatViewItem[] = [
  user("u1", 1),
  reply("a1-commentary"),
  reply("a1"),
  user("u2", 10),
  reply("a2"),
  user("steer", 15, { inflight: true } as Partial<ChatViewItem>),
  reply("a2b"),
  user("u3", 20),
  reply("a3"),
];

describe("computeForkPoints", () => {
  it("offers the last reply of each finished turn, cut before the next turn", () => {
    const points = computeForkPoints(TRANSCRIPT, "claude", undefined, false);
    expect([...points]).toEqual([
      ["a1", 10],
      ["a2b", 20],
      ["a3", null],
    ]);
  });

  it("only cuts Codex turns that have a checkpoint for that user message", () => {
    const checkpoints = new Map<string, AgentChatTurnCheckpointRecord>([
      ["n-u3", { transcript_cutoff_id: 19 } as AgentChatTurnCheckpointRecord],
      // A stale record that doesn't match the row is not trusted.
      ["n-u2", { transcript_cutoff_id: 3 } as AgentChatTurnCheckpointRecord],
    ]);
    const points = computeForkPoints(TRANSCRIPT, "codex", checkpoints, false);
    expect([...points]).toEqual([
      ["a2b", 20],
      ["a3", null],
    ]);
  });

  it("offers nothing while busy or for providers that can't fork", () => {
    expect(computeForkPoints(TRANSCRIPT, "claude", undefined, true).size).toBe(0);
    expect(computeForkPoints(TRANSCRIPT, "opencode", undefined, false).size).toBe(0);
    expect(chatForkSupported("hermes")).toBe(false);
    expect(chatForkSupported("codex")).toBe(true);
  });

  it("skips queued prompts, optimistic rows and streaming replies", () => {
    const items = [
      user("u1", 1),
      reply("a1"),
      user("queued", undefined, { queued: { queuedId: "q" } } as Partial<ChatViewItem>),
      user("optimistic", undefined),
      reply("a2", { streaming: true } as Partial<ChatViewItem>),
    ];
    // a1 is followed by a sent prompt with no durable row yet: no cut.
    expect([...computeForkPoints(items, "claude", undefined, false)]).toEqual([]);
  });
});

describe("forkChat", () => {
  beforeEach(() => {
    invoke.mockReset();
    toastSuccess.mockReset();
    toastFailure.mockReset();
    activate.mockClear();
  });

  it("forks in place and opens the fork in a new tab", async () => {
    invoke.mockImplementation(async (command: string) => {
      if (command === "agent_chat_fork_thread") {
        return { thread_id: "fork-1", provider: "claude", title: "X (fork)", workspace_id: "ws", cwd: "/repo" };
      }
      return "pane-1";
    });
    await expect(
      forkChat({ sourceThreadId: "src", workspaceId: "ws", before: 10, target: "here" }),
    ).resolves.toBe("fork-1");
    expect(invoke).toHaveBeenCalledWith("agent_chat_fork_thread", {
      sourceThreadId: "src",
      beforeMessageId: 10,
      workspaceId: null,
    });
    expect(invoke).toHaveBeenCalledWith(
      "agent_chat_create_pane",
      expect.objectContaining({ workspaceId: "ws", threadId: "fork-1", launchMode: "new_tab" }),
    );
    expect(activate).not.toHaveBeenCalled();
    expect(toastSuccess).toHaveBeenCalledWith("Chat forked", { description: "X (fork)" });
  });

  it("reports a refused fork without opening anything", async () => {
    invoke.mockRejectedValue("Wait for the current turn to finish, then fork the chat.");
    await expect(
      forkChat({ sourceThreadId: "src", workspaceId: "ws", before: null, target: "here" }),
    ).resolves.toBeNull();
    expect(toastFailure).toHaveBeenCalledWith(
      "Couldn't fork the chat",
      "Wait for the current turn to finish, then fork the chat.",
    );
    expect(invoke).toHaveBeenCalledTimes(1);
  });
});
