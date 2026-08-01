import { beforeEach, describe, expect, it, vi } from "vitest";

import { useAgentChatStore } from "@/stores/agent-chat-store";
import type { ProviderRuntimeEvent } from "@/tauri/events";

import type { LiveChatEvent } from "./types";

import {
  createAgentChatEventBatcher,
  HIDDEN_FLUSH_INTERVAL_MS,
} from "./event-batcher";

const THREAD = "t1";

function delta(text: string, turnId = "turn-1"): ProviderRuntimeEvent {
  return {
    type: "content_delta",
    thread_id: THREAD,
    turn_id: turnId,
    delta: { kind: "text", text },
  };
}

function turnCompleted(turnId = "turn-1"): ProviderRuntimeEvent {
  return {
    type: "turn_completed",
    thread_id: THREAD,
    turn_id: turnId,
    status: { kind: "success" },
    usage: null,
  };
}

/** Manual rAF: `runFrame()` stands in for the browser's next paint. */
function manualScheduler() {
  let pending: (() => void) | null = null;
  return {
    schedule: (run: () => void) => {
      pending = run;
      return () => {
        pending = null;
      };
    },
    runFrame: () => {
      const run = pending;
      pending = null;
      run?.();
    },
    get scheduled() {
      return pending !== null;
    },
  };
}

describe("agent-chat event batcher", () => {
  it("coalesces a burst of content deltas into a single apply", () => {
    const frames = manualScheduler();
    const apply = vi.fn();
    const batcher = createAgentChatEventBatcher({
      apply,
      scheduleFrame: frames.schedule,
      isHidden: () => false,
      observeVisibility: () => () => undefined,
    });

    for (let i = 0; i < 100; i += 1) batcher.enqueue(THREAD, delta(`${i} `));
    expect(apply).not.toHaveBeenCalled();
    expect(batcher.pending(THREAD)).toBe(100);

    frames.runFrame();
    expect(apply).toHaveBeenCalledTimes(1);
    expect(apply.mock.calls[0][0]).toBe(THREAD);
    expect(apply.mock.calls[0][1]).toHaveLength(100);
    expect(batcher.pending(THREAD)).toBe(0);
  });

  it("preserves arrival order across frames", () => {
    const frames = manualScheduler();
    const applied: string[] = [];
    const batcher = createAgentChatEventBatcher({
      apply: (_thread, items) => {
        for (const { event } of items) {
          if (event.type === "content_delta" && event.delta.kind === "text") {
            applied.push(event.delta.text);
          }
        }
      },
      scheduleFrame: frames.schedule,
      isHidden: () => false,
      observeVisibility: () => () => undefined,
    });

    batcher.enqueue(THREAD, delta("a"));
    batcher.enqueue(THREAD, delta("b"));
    frames.runFrame();
    batcher.enqueue(THREAD, delta("c"));
    frames.runFrame();

    expect(applied).toEqual(["a", "b", "c"]);
  });

  it("flushes synchronously on a non-delta event, with it last in order", () => {
    const frames = manualScheduler();
    const apply = vi.fn();
    const batcher = createAgentChatEventBatcher({
      apply,
      scheduleFrame: frames.schedule,
      isHidden: () => false,
      observeVisibility: () => () => undefined,
    });

    batcher.enqueue(THREAD, delta("a"));
    batcher.enqueue(THREAD, delta("b"));
    expect(apply).not.toHaveBeenCalled();

    batcher.enqueue(THREAD, turnCompleted());
    expect(apply).toHaveBeenCalledTimes(1);
    const items = apply.mock.calls[0][1] as LiveChatEvent[];
    expect(items.map((i) => i.event.type)).toEqual([
      "content_delta",
      "content_delta",
      "turn_completed",
    ]);
    // The pending frame is cancelled, not left to fire on an empty queue.
    expect(frames.scheduled).toBe(false);
  });

  it("falls back to a timer while the document is hidden", () => {
    vi.useFakeTimers();
    try {
      const frames = manualScheduler();
      const apply = vi.fn();
      const batcher = createAgentChatEventBatcher({
        apply,
        scheduleFrame: frames.schedule,
        isHidden: () => true,
        observeVisibility: () => () => undefined,
      });

      batcher.enqueue(THREAD, delta("a"));
      // rAF never fires for a hidden document, so nothing may depend on it.
      expect(frames.scheduled).toBe(false);
      expect(apply).not.toHaveBeenCalled();

      vi.advanceTimersByTime(HIDDEN_FLUSH_INTERVAL_MS);
      expect(apply).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it("drains the queue when the document becomes hidden mid-frame", () => {
    const frames = manualScheduler();
    const apply = vi.fn();
    let onHidden = () => undefined as void;
    const batcher = createAgentChatEventBatcher({
      apply,
      scheduleFrame: frames.schedule,
      isHidden: () => false,
      observeVisibility: (handler) => {
        onHidden = handler;
        return () => undefined;
      },
    });

    batcher.enqueue(THREAD, delta("a"));
    expect(frames.scheduled).toBe(true);
    onHidden();
    expect(apply).toHaveBeenCalledTimes(1);
    expect(frames.scheduled).toBe(false);
  });

  it("flushes on demand (detach / pre-hydrate seams)", () => {
    const frames = manualScheduler();
    const apply = vi.fn();
    const batcher = createAgentChatEventBatcher({
      apply,
      scheduleFrame: frames.schedule,
      isHidden: () => false,
      observeVisibility: () => () => undefined,
    });

    batcher.enqueue(THREAD, delta("a"));
    batcher.flush(THREAD);
    expect(apply).toHaveBeenCalledTimes(1);
    // Nothing queued -> a redundant flush is a no-op, not an empty apply.
    batcher.flush(THREAD);
    expect(apply).toHaveBeenCalledTimes(1);
  });

  it("keeps per-thread queues independent", () => {
    const frames = manualScheduler();
    const apply = vi.fn();
    const batcher = createAgentChatEventBatcher({
      apply,
      scheduleFrame: frames.schedule,
      isHidden: () => false,
      observeVisibility: () => () => undefined,
    });

    batcher.enqueue("a", { ...delta("1"), thread_id: "a" });
    batcher.enqueue("b", { ...delta("2"), thread_id: "b" });
    batcher.enqueue("a", { ...turnCompleted(), thread_id: "a" });

    expect(apply).toHaveBeenCalledTimes(1);
    expect(apply.mock.calls[0][0]).toBe("a");
    expect(batcher.pending("b")).toBe(1);

    frames.runFrame();
    expect(apply).toHaveBeenCalledTimes(2);
    expect(apply.mock.calls[1][0]).toBe("b");
  });
});

describe("agent-chat event batcher — hold ownership", () => {
  function batcher(apply: (t: string, e: LiveChatEvent[]) => void) {
    return createAgentChatEventBatcher({
      apply,
      scheduleFrame: manualScheduler().schedule,
      isHidden: () => false,
      observeVisibility: () => () => undefined,
    });
  }

  it("a superseded hold's drop is a no-op and leaves the newer hold intact", () => {
    // Hydrate #1 is still in its IPC window when the pane remounts and
    // hydrate #2 takes the hold. #1's teardown must not unpark the thread:
    // events would then apply mid-read and #2's tail would double-apply.
    const apply = vi.fn();
    const b = batcher(apply);

    const first = b.hold(THREAD);
    const second = b.hold(THREAD);
    b.enqueue(THREAD, turnCompleted());

    b.drop(THREAD, first);
    expect(b.isHeld(THREAD)).toBe(true);
    expect(b.pending(THREAD)).toBe(1);
    expect(apply).not.toHaveBeenCalled();

    b.release(THREAD, second);
    expect(apply).toHaveBeenCalledTimes(1);
  });

  it("a superseded hold's release is a no-op too", () => {
    const apply = vi.fn();
    const b = batcher(apply);
    const first = b.hold(THREAD);
    b.hold(THREAD);
    b.enqueue(THREAD, turnCompleted());

    b.release(THREAD, first);
    expect(apply).not.toHaveBeenCalled();
    expect(b.isHeld(THREAD)).toBe(true);
  });

  it("release after the hold already ended does not re-apply", () => {
    const apply = vi.fn();
    const b = batcher(apply);
    const token = b.hold(THREAD);
    b.enqueue(THREAD, turnCompleted());
    b.release(THREAD, token);
    expect(apply).toHaveBeenCalledTimes(1);

    b.enqueue(THREAD, turnCompleted());
    b.release(THREAD, token);
    // The second enqueue flushed on its own (unheld); the stale release
    // must not double-deliver it.
    expect(apply).toHaveBeenCalledTimes(2);
  });

  it("a filtered release drops the events the filter removes", () => {
    const apply = vi.fn();
    const b = batcher(apply);
    const token = b.hold(THREAD);
    b.enqueue(THREAD, delta("stale"));
    b.enqueue(THREAD, turnCompleted());

    b.release(THREAD, token, (events) =>
      events.filter((e) => e.event.type !== "content_delta"),
    );

    expect(apply).toHaveBeenCalledTimes(1);
    const items = apply.mock.calls[0][1] as LiveChatEvent[];
    expect(items.map((i) => i.event.type)).toEqual(["turn_completed"]);
  });

  it("a filter that empties the buffer applies nothing at all", () => {
    const apply = vi.fn();
    const b = batcher(apply);
    const token = b.hold(THREAD);
    b.enqueue(THREAD, delta("stale"));
    b.release(THREAD, token, () => []);
    expect(apply).not.toHaveBeenCalled();
  });
});

describe("agent-chat event batcher → store", () => {
  beforeEach(() => {
    useAgentChatStore.setState({ threads: {} });
  });

  it("100 deltas produce one store update with the exact final text", () => {
    const frames = manualScheduler();
    const batcher = createAgentChatEventBatcher({
      apply: (threadId, items) =>
        useAgentChatStore.getState().applyLiveEvents(threadId, items),
      scheduleFrame: frames.schedule,
      isHidden: () => false,
      observeVisibility: () => () => undefined,
    });
    useAgentChatStore.getState().ensureThread(THREAD);

    let sets = 0;
    const unsubscribe = useAgentChatStore.subscribe(() => {
      sets += 1;
    });

    let expected = "";
    for (let i = 0; i < 100; i += 1) {
      const chunk = `tok${i} `;
      expected += chunk;
      batcher.enqueue(THREAD, delta(chunk));
    }
    expect(sets).toBe(0);
    frames.runFrame();
    unsubscribe();

    expect(sets).toBe(1);
    const messages = useAgentChatStore.getState().threads[THREAD].messages;
    const assistant = messages.filter((m) => m.kind === "assistant_message");
    expect(assistant).toHaveLength(1);
    expect(assistant[0].kind === "assistant_message" && assistant[0].text).toBe(
      expected,
    );
  });

  it("applyEvents matches event-at-a-time applyEvent", () => {
    const events = [delta("hello "), delta("world"), turnCompleted()];

    useAgentChatStore.getState().ensureThread("batched");
    useAgentChatStore.getState().applyEvents("batched", events);

    useAgentChatStore.getState().ensureThread("serial");
    for (const event of events) {
      useAgentChatStore.getState().applyEvent("serial", event);
    }

    const batched = useAgentChatStore.getState().threads["batched"];
    const serial = useAgentChatStore.getState().threads["serial"];
    expect(batched.messages.map((m) => m.kind)).toEqual(
      serial.messages.map((m) => m.kind),
    );
    const batchedText = batched.messages.find(
      (m) => m.kind === "assistant_message",
    );
    const serialText = serial.messages.find(
      (m) => m.kind === "assistant_message",
    );
    expect(
      batchedText?.kind === "assistant_message" && batchedText.text,
    ).toBe("hello world");
    expect(serialText?.kind === "assistant_message" && serialText.text).toBe(
      "hello world",
    );
    expect(batched.streaming).toBe(serial.streaming);
    expect(batched.activeTurnId).toBe(serial.activeTurnId);
  });

  it("an empty batch does not touch the store", () => {
    useAgentChatStore.getState().ensureThread(THREAD);
    const before = useAgentChatStore.getState().threads;
    useAgentChatStore.getState().applyEvents(THREAD, []);
    expect(useAgentChatStore.getState().threads).toBe(before);
  });
});
