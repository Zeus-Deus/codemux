import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, renderHook, waitFor } from "@testing-library/react";

import type { AgentChatTurnCheckpointRecord } from "@/tauri/commands";
import type { AgentChatTurnCheckpointPayload } from "@/tauri/events";

/**
 * Capture the event handlers the hook registers so a test can push a live
 * checkpoint event, and keep the mount-time list fetch pending so its
 * resolution can be ordered AFTER that event.
 */
const { agentChatListTurnCheckpoints, handlers } = vi.hoisted(() => ({
  agentChatListTurnCheckpoints: vi.fn(),
  handlers: {} as {
    checkpoint?: (payload: AgentChatTurnCheckpointPayload) => void;
    invalidated?: (threadId: string) => void;
  },
}));

vi.mock("@/tauri/commands", () => ({ agentChatListTurnCheckpoints }));
vi.mock("@/tauri/events", () => ({
  onAgentChatTurnCheckpoint: (
    cb: (payload: AgentChatTurnCheckpointPayload) => void,
  ) => {
    handlers.checkpoint = cb;
    return Promise.resolve(() => {});
  },
  onAgentChatTurnCheckpointReverted: () => Promise.resolve(() => {}),
  onAgentChatTurnCheckpointsInvalidated: (cb: (threadId: string) => void) => {
    handlers.invalidated = cb;
    return Promise.resolve(() => {});
  },
}));

import { useAgentChatTurnCheckpoints } from "./use-agent-chat-turn-checkpoints";

function record(turnIndex: number): AgentChatTurnCheckpointRecord {
  return {
    thread_id: "t1",
    workspace_id: "ws-1",
    repo_path: "/repo",
    turn_index: turnIndex,
    client_nonce: `nonce-${turnIndex}`,
    transcript_cutoff_id: turnIndex,
    ref_name: `refs/codemux/turn-checkpoints/t1/${turnIndex}`,
    snapshot_commit: `snap-${turnIndex}`,
    head_commit: "head",
    branch: "main",
    created_at: "2026-01-01",
  };
}

describe("useAgentChatTurnCheckpoints", () => {
  beforeEach(() => {
    delete handlers.checkpoint;
    delete handlers.invalidated;
    agentChatListTurnCheckpoints.mockReset();
  });

  it("hydrates from the mount-time fetch", async () => {
    agentChatListTurnCheckpoints.mockResolvedValue([record(1)]);
    const { result } = renderHook(() => useAgentChatTurnCheckpoints("t1"));
    await waitFor(() => expect(result.current).toHaveLength(1));
    expect(result.current[0]?.turn_index).toBe(1);
  });

  it("keeps a live checkpoint that arrived before the fetch resolved", async () => {
    let resolveList: (records: AgentChatTurnCheckpointRecord[]) => void = () => {};
    agentChatListTurnCheckpoints.mockReturnValue(
      new Promise<AgentChatTurnCheckpointRecord[]>((resolve) => {
        resolveList = resolve;
      }),
    );
    const { result } = renderHook(() => useAgentChatTurnCheckpoints("t1"));
    await waitFor(() => expect(handlers.checkpoint).toBeDefined());

    act(() => {
      handlers.checkpoint?.({
        thread_id: "t1",
        checkpoint: record(1),
        oldest_turn_index: 1,
      });
    });
    expect(result.current).toHaveLength(1);

    // The pre-event snapshot lands late and must not clobber the event.
    await act(async () => {
      resolveList([]);
    });
    expect(result.current.map((entry) => entry.turn_index)).toEqual([1]);
  });

  it("keeps an invalidation that arrived before the fetch resolved", async () => {
    let resolveList: (records: AgentChatTurnCheckpointRecord[]) => void = () => {};
    agentChatListTurnCheckpoints.mockReturnValue(
      new Promise<AgentChatTurnCheckpointRecord[]>((resolve) => {
        resolveList = resolve;
      }),
    );
    const { result } = renderHook(() => useAgentChatTurnCheckpoints("t1"));
    await waitFor(() => expect(handlers.invalidated).toBeDefined());

    act(() => {
      handlers.invalidated?.("t1");
    });
    await act(async () => {
      resolveList([record(1), record(2)]);
    });
    expect(result.current).toEqual([]);
  });
});
