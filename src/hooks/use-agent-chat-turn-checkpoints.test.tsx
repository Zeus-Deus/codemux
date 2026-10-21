import { StrictMode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";

import type { AgentChatTurnCheckpointRecord } from "@/tauri/commands";
import type {
  AgentChatTurnCheckpointPayload,
  AgentChatTurnCheckpointRevertedPayload,
} from "@/tauri/events";

/**
 * Capture the event handlers the hook registers so a test can push a live
 * checkpoint event, and keep the mount-time list fetch pending so its
 * resolution can be ordered AFTER that event.
 */
const { agentChatListTurnCheckpoints, handlers } = vi.hoisted(() => ({
  agentChatListTurnCheckpoints: vi.fn(),
  handlers: {} as {
    checkpoint?: (payload: AgentChatTurnCheckpointPayload) => void;
    reverted?: (payload: AgentChatTurnCheckpointRevertedPayload) => void;
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
  onAgentChatTurnCheckpointReverted: (
    cb: (payload: AgentChatTurnCheckpointRevertedPayload) => void,
  ) => {
    handlers.reverted = cb;
    return Promise.resolve(() => {});
  },
  onAgentChatTurnCheckpointsInvalidated: (cb: (threadId: string) => void) => {
    handlers.invalidated = cb;
    return Promise.resolve(() => {});
  },
}));

import { useAgentChatTurnCheckpoints } from "./use-agent-chat-turn-checkpoints";

function record(turnIndex: number, threadId = "t1"): AgentChatTurnCheckpointRecord {
  return {
    thread_id: threadId,
    workspace_id: "ws-1",
    repo_path: "/repo",
    turn_index: turnIndex,
    client_nonce: `nonce-${turnIndex}`,
    transcript_cutoff_id: turnIndex,
    ref_name: `refs/codemux/turn-checkpoints/${threadId}/${turnIndex}`,
    snapshot_commit: `snap-${turnIndex}`,
    head_commit: "head",
    branch: "main",
    created_at: "2026-01-01",
  };
}

function deferredRecords() {
  let resolve!: (records: AgentChatTurnCheckpointRecord[]) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<AgentChatTurnCheckpointRecord[]>((done, fail) => {
    resolve = done;
    reject = fail;
  });
  return { promise, resolve, reject };
}

describe("useAgentChatTurnCheckpoints", () => {
  beforeEach(() => {
    delete handlers.checkpoint;
    delete handlers.reverted;
    delete handlers.invalidated;
    agentChatListTurnCheckpoints.mockReset();
  });
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
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

  it("merges live additions with older checkpoints from the pending fetch", async () => {
    const pending = deferredRecords();
    agentChatListTurnCheckpoints.mockReturnValue(pending.promise);
    const { result } = renderHook(() => useAgentChatTurnCheckpoints("t1"));

    act(() => {
      handlers.checkpoint?.({
        thread_id: "t1",
        checkpoint: record(3),
        oldest_turn_index: 1,
      });
    });
    await act(async () => {
      pending.resolve([record(1), record(2)]);
    });
    expect(result.current.map((entry) => entry.turn_index)).toEqual([1, 2, 3]);
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

  it("replays additions in order with pruning and updates to an existing turn", async () => {
    const pending = deferredRecords();
    agentChatListTurnCheckpoints.mockReturnValue(pending.promise);
    const { result } = renderHook(() => useAgentChatTurnCheckpoints("t1"));
    const boundCheckpoint = { ...record(4), transcript_cutoff_id: 40 };

    act(() => {
      for (const checkpoint of [
        record(4), boundCheckpoint, boundCheckpoint, record(6),
      ]) {
        handlers.checkpoint?.({
          thread_id: "t1",
          checkpoint,
          oldest_turn_index: checkpoint.turn_index === 6 ? 3 : 2,
        });
      }
    });
    await act(async () => {
      pending.resolve([record(1), record(2), record(3), record(4)]);
    });
    expect(result.current).toEqual([record(3), boundCheckpoint, record(6)]);
  });

  it.each(["revert", "invalidation"] as const)(
    "keeps the %s replacement and subsequent additions over a late fetch",
    async (replacement) => {
      const pending = deferredRecords();
      agentChatListTurnCheckpoints.mockReturnValue(pending.promise);
      const { result } = renderHook(() => useAgentChatTurnCheckpoints("t1"));
      const remaining = replacement === "revert" ? [record(1)] : [];

      act(() => {
        handlers.checkpoint?.({
          thread_id: "t1",
          checkpoint: record(3),
          oldest_turn_index: 1,
        });
        if (replacement === "revert") {
          handlers.reverted?.({
            thread_id: "t1",
            turn_index: 2,
            transcript_cutoff_id: 2,
            remaining_checkpoints: remaining,
          });
        } else {
          handlers.invalidated?.("t1");
        }
        handlers.checkpoint?.({
          thread_id: "t1",
          checkpoint: record(4),
          oldest_turn_index: 1,
        });
      });
      await act(async () => {
        pending.resolve([record(1), record(2), record(3)]);
      });
      expect(result.current).toEqual([...remaining, record(4)]);
    },
  );

  it("isolates pending fetches and event buffers across thread switches", async () => {
    const first = deferredRecords();
    const second = deferredRecords();
    agentChatListTurnCheckpoints
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise);
    const initialProps: { threadId: string | null } = { threadId: "t1" };
    const { result, rerender } = renderHook(
      ({ threadId }: { threadId: string | null }) =>
        useAgentChatTurnCheckpoints(threadId),
      { initialProps },
    );
    const oldHandler = handlers.checkpoint;
    act(() => {
      handlers.checkpoint?.({
        thread_id: "t1",
        checkpoint: record(3),
        oldest_turn_index: 1,
      });
    });
    rerender({ threadId: "t2" });
    expect(result.current).toEqual([]);
    act(() => {
      oldHandler?.({
        thread_id: "t1",
        checkpoint: record(4),
        oldest_turn_index: 1,
      });
      handlers.checkpoint?.({
        thread_id: "t1",
        checkpoint: record(5),
        oldest_turn_index: 1,
      });
      handlers.checkpoint?.({
        thread_id: "t2",
        checkpoint: record(3, "t2"),
        oldest_turn_index: 1,
      });
    });
    await act(async () => {
      first.resolve([record(1), record(2)]);
    });
    expect(result.current).toEqual([record(3, "t2")]);
    await act(async () => {
      second.resolve([record(1, "t2"), record(2, "t2")]);
    });
    expect(result.current).toEqual([
      record(1, "t2"),
      record(2, "t2"),
      record(3, "t2"),
    ]);
    rerender({ threadId: null });
    expect(result.current).toEqual([]);
    expect(agentChatListTurnCheckpoints).toHaveBeenCalledTimes(2);
  });

  it.each(["resolve", "reject"] as const)(
    "ignores a cancelled Strict Mode fetch that %ss without clearing the active buffer",
    async (settlement) => {
      const cancelled = deferredRecords();
      const current = deferredRecords();
      const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
      agentChatListTurnCheckpoints
        .mockReturnValueOnce(cancelled.promise)
        .mockReturnValueOnce(current.promise);
      const { result } = renderHook(() => useAgentChatTurnCheckpoints("t1"), {
        wrapper: StrictMode,
      });
      act(() => {
        handlers.checkpoint?.({
          thread_id: "t1",
          checkpoint: record(3),
          oldest_turn_index: 1,
        });
      });
      await act(async () => {
        if (settlement === "resolve") cancelled.resolve([record(99)]);
        else cancelled.reject(new Error("cancelled request"));
      });
      expect(warn).not.toHaveBeenCalled();
      expect(result.current).toEqual([record(3)]);
      await act(async () => {
        current.resolve([record(1), record(2)]);
      });
      expect(result.current).toEqual([record(1), record(2), record(3)]);
    },
  );

  it.each(["resolve", "reject"] as const)(
    "ignores stale fetches and event handlers after unmount when the fetch %ss",
    async (settlement) => {
      const pending = deferredRecords();
      agentChatListTurnCheckpoints.mockReturnValue(pending.promise);
      const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
      const { result, unmount } = renderHook(() =>
        useAgentChatTurnCheckpoints("t1"),
      );
      const oldHandlers = { ...handlers };
      act(() => {
        handlers.checkpoint?.({
          thread_id: "t1",
          checkpoint: record(3),
          oldest_turn_index: 1,
        });
      });
      const beforeUnmount = result.current;
      unmount();
      await act(async () => {
        oldHandlers.checkpoint?.({
          thread_id: "t1",
          checkpoint: record(4),
          oldest_turn_index: 1,
        });
        oldHandlers.reverted?.({
          thread_id: "t1",
          turn_index: 2,
          transcript_cutoff_id: 2,
          remaining_checkpoints: [record(1)],
        });
        oldHandlers.invalidated?.("t1");
        if (settlement === "resolve") pending.resolve([record(1), record(2)]);
        else pending.reject(new Error("unmounted request"));
      });
      expect(result.current).toBe(beforeUnmount);
      expect(warn).not.toHaveBeenCalled();
    },
  );

  it("keeps accepting live additions if the initial fetch fails", async () => {
    const pending = deferredRecords();
    agentChatListTurnCheckpoints.mockReturnValue(pending.promise);
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const { result } = renderHook(() => useAgentChatTurnCheckpoints("t1"));
    act(() => {
      handlers.checkpoint?.({
        thread_id: "t1",
        checkpoint: record(1),
        oldest_turn_index: 1,
      });
    });
    await act(async () => {
      pending.reject(new Error("unavailable"));
    });
    expect(warn).toHaveBeenCalledOnce();
    act(() => {
      handlers.checkpoint?.({
        thread_id: "t1",
        checkpoint: record(2),
        oldest_turn_index: 1,
      });
    });
    expect(result.current).toEqual([record(1), record(2)]);
  });
});
