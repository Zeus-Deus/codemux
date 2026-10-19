import { describe, expect, it, vi } from "vitest";
import { act, renderHook, waitFor } from "@testing-library/react";

import type { ChatForkOrigin } from "@/tauri/types";

import { useChatForkOrigin } from "./ChatFork";

const commands = vi.hoisted(() => ({
  agentChatForkOrigin: vi.fn(),
  agentChatOpenSearchResult: vi.fn(),
}));
vi.mock("@/tauri/commands", () => commands);

const events = vi.hoisted(() => ({
  merged: null as ((payload: { survivor: string; merged: string[] }) => void) | null,
}));
vi.mock("@/tauri/events", () => ({
  onAgentChatSessionsMerged: vi.fn(
    (cb: (payload: { survivor: string; merged: string[] }) => void) => {
      events.merged = cb;
      return Promise.resolve(() => {});
    },
  ),
}));

const origin: ChatForkOrigin = {
  source_thread_id: "source",
  source_title: "Fix the parser",
  source_exists: true,
  boundary_message_id: 7,
};

describe("useChatForkOrigin", () => {
  it("reads the lineage again once a reopened fork is merged into the chat", async () => {
    // The pane is bound to the reopened thread before its lineage moves.
    commands.agentChatForkOrigin.mockResolvedValueOnce(null).mockResolvedValue(origin);
    const { result } = renderHook(() => useChatForkOrigin("reopened"));
    await waitFor(() => expect(events.merged).not.toBeNull());
    await waitFor(() => expect(commands.agentChatForkOrigin).toHaveBeenCalledTimes(1));
    expect(result.current).toBeNull();

    act(() => events.merged?.({ survivor: "another-chat", merged: ["old"] }));
    expect(commands.agentChatForkOrigin).toHaveBeenCalledTimes(1);

    act(() => events.merged?.({ survivor: "reopened", merged: ["fork"] }));
    await waitFor(() => expect(result.current).toEqual(origin));
    expect(commands.agentChatForkOrigin).toHaveBeenLastCalledWith("reopened");
  });
});
