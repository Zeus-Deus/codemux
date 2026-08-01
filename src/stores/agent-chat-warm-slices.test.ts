import { beforeEach, describe, expect, it } from "vitest";

import { LAZY_TOOL_RESULT_KEY } from "@/lib/agent-chat/lazy-tool-result";
import type { ToolCallItem } from "@/lib/agent-chat/types";
import type { AgentChatMessageRow } from "@/tauri/commands";
import type { ProviderRuntimeEvent } from "@/tauri/events";

import {
  registerMountedThread,
  useAgentChatStore,
  WARM_SLICE_BUDGET_UNITS,
} from "./agent-chat-store";

function assistantEvent(
  threadId: string,
  text: string,
  turnId = "turn-1",
): ProviderRuntimeEvent {
  return {
    type: "item_completed",
    thread_id: threadId,
    turn_id: turnId,
    item: { kind: "assistant_text", text },
  } as ProviderRuntimeEvent;
}

function row(id: number, event: ProviderRuntimeEvent): AgentChatMessageRow {
  return { id, payload: JSON.stringify(event) };
}

function texts(threadId: string): string[] {
  const slice = useAgentChatStore.getState().threads[threadId];
  return (slice?.messages ?? []).flatMap((m) =>
    m.kind === "assistant_message" ? [m.text] : [],
  );
}

beforeEach(() => {
  useAgentChatStore.setState({ threads: {} });
});

describe("resume cursor", () => {
  it("hydrate sets the cursor to the last row id — and 0 for an empty thread", () => {
    useAgentChatStore
      .getState()
      .hydrateThread("t", [row(7, assistantEvent("t", "a")), row(9, assistantEvent("t", "b"))]);
    expect(useAgentChatStore.getState().threads.t.lastPersistedEventId).toBe(9);

    useAgentChatStore.getState().hydrateThread("empty", []);
    // 0, not null: the thread IS synced, its history just starts at the
    // beginning of the id space.
    expect(useAgentChatStore.getState().threads.empty.lastPersistedEventId).toBe(0);
  });

  it("drops a live event at or below the cursor and advances past a newer one", () => {
    useAgentChatStore.getState().hydrateThread("t", [row(5, assistantEvent("t", "a"))]);
    useAgentChatStore.getState().applyLiveEvents("t", [
      { event: assistantEvent("t", "a", "turn-1"), persistedId: 5 }, // replayed
      { event: assistantEvent("t", "b", "turn-2"), persistedId: 6 },
    ]);
    expect(texts("t")).toEqual(["a", "b"]);
    expect(useAgentChatStore.getState().threads.t.lastPersistedEventId).toBe(6);
  });

  it("never advances the cursor of an unsynced slice", () => {
    // A slice that has never hydrated has no idea which rows sit below a
    // live event's id — advancing would strand every one of them.
    useAgentChatStore.getState().ensureThread("t");
    useAgentChatStore
      .getState()
      .applyLiveEvents("t", [{ event: assistantEvent("t", "live"), persistedId: 900 }]);
    expect(texts("t")).toEqual(["live"]);
    expect(useAgentChatStore.getState().threads.t.lastPersistedEventId).toBeNull();
  });

  it("leaves the cursor alone for ephemeral events", () => {
    useAgentChatStore.getState().hydrateThread("t", [row(5, assistantEvent("t", "a"))]);
    useAgentChatStore
      .getState()
      .applyLiveEvents("t", [{ event: assistantEvent("t", "b", "turn-2"), persistedId: null }]);
    expect(useAgentChatStore.getState().threads.t.lastPersistedEventId).toBe(5);
    expect(texts("t")).toEqual(["a", "b"]);
  });

  it("a tail merge appends without rebuilding the prefix and keeps activeTurnId", () => {
    useAgentChatStore.getState().hydrateThread("t", [row(5, assistantEvent("t", "a"))]);
    useAgentChatStore.setState((s) => ({
      threads: { ...s.threads, t: { ...s.threads.t, activeTurnId: "turn-live" } },
    }));
    const before = useAgentChatStore.getState().threads.t.messages[0];

    useAgentChatStore
      .getState()
      .applyPayloadsTail("t", [row(6, assistantEvent("t", "b", "turn-2"))]);

    const after = useAgentChatStore.getState().threads.t;
    expect(texts("t")).toEqual(["a", "b"]);
    // The prefix item is the SAME object — the transcript's slot reuse
    // (and every memoized row) survives a warm resume.
    expect(after.messages[0]).toBe(before);
    expect(after.lastPersistedEventId).toBe(6);
    expect(after.activeTurnId).toBe("turn-live");
  });

  it("an empty tail is a no-op", () => {
    useAgentChatStore.getState().hydrateThread("t", [row(5, assistantEvent("t", "a"))]);
    const before = useAgentChatStore.getState().threads;
    useAgentChatStore.getState().applyPayloadsTail("t", []);
    expect(useAgentChatStore.getState().threads).toBe(before);
  });

  it("a thread-id migration drops the cursor", () => {
    // Rows are re-homed between threads keeping their original ids, so a
    // carried-over cursor can sit above the new thread's head.
    useAgentChatStore.getState().hydrateThread("old", [row(5, assistantEvent("old", "a"))]);
    useAgentChatStore.getState().migrateThreadId("old", "new");
    expect(useAgentChatStore.getState().threads.new.lastPersistedEventId).toBeNull();
    expect(texts("new")).toEqual(["a"]);
  });
});

describe("lazy tool-result resolution", () => {
  function stubItem(rowId: number): ToolCallItem {
    return {
      kind: "tool_call",
      id: `tool-${rowId}`,
      seq: 1,
      tool_use_id: `tu-${rowId}`,
      tool_name: "Bash",
      input: {},
      status: "done",
      result_content: {
        [LAZY_TOOL_RESULT_KEY]: {
          row_id: rowId,
          bytes: 40_000,
          preview: "head",
          line_count: 900,
          has_images: false,
        },
      },
      approval_request_id: null,
    };
  }

  it("swaps the body into the matching item across threads, by row id", () => {
    useAgentChatStore.getState().ensureThread("a");
    useAgentChatStore.getState().ensureThread("b");
    useAgentChatStore.setState((s) => ({
      threads: {
        ...s.threads,
        a: { ...s.threads.a, messages: [stubItem(1)] },
        b: { ...s.threads.b, messages: [stubItem(2)] },
      },
    }));

    useAgentChatStore.getState().resolveLazyToolResult(2, "full body");

    const a = useAgentChatStore.getState().threads.a.messages[0] as ToolCallItem;
    const b = useAgentChatStore.getState().threads.b.messages[0] as ToolCallItem;
    expect(b.result_content).toBe("full body");
    // The other thread's stub is untouched.
    expect(a.result_content).toEqual(stubItem(1).result_content);
  });

  it("is a no-op for an unknown row id", () => {
    useAgentChatStore.getState().ensureThread("a");
    const before = useAgentChatStore.getState().threads;
    useAgentChatStore.getState().resolveLazyToolResult(999, "x");
    expect(useAgentChatStore.getState().threads).toBe(before);
  });
});

describe("warm-slice eviction", () => {
  /** A slice heavy enough to matter against the budget. */
  function seedHeavyThread(threadId: string, units: number) {
    useAgentChatStore.getState().hydrateThread(threadId, [
      row(1, assistantEvent(threadId, "x".repeat(units))),
    ]);
  }

  it("keeps everything while under budget", () => {
    seedHeavyThread("a", 1000);
    seedHeavyThread("b", 1000);
    useAgentChatStore.getState().evictColdThreads([]);
    expect(Object.keys(useAgentChatStore.getState().threads).sort()).toEqual([
      "a",
      "b",
    ]);
  });

  it("evicts oldest-touched first until back under budget", () => {
    const heavy = Math.ceil(WARM_SLICE_BUDGET_UNITS / 2);
    seedHeavyThread("oldest", heavy);
    seedHeavyThread("middle", heavy);
    seedHeavyThread("newest", heavy);

    useAgentChatStore.getState().evictColdThreads([]);

    const remaining = Object.keys(useAgentChatStore.getState().threads);
    expect(remaining).not.toContain("oldest");
    expect(remaining).toContain("newest");
  });

  it("never evicts the active thread or a busy one", () => {
    const heavy = Math.ceil(WARM_SLICE_BUDGET_UNITS / 2);
    seedHeavyThread("active", heavy);
    seedHeavyThread("running", heavy);
    seedHeavyThread("idle", heavy);
    useAgentChatStore.setState((s) => ({
      threads: {
        ...s.threads,
        running: { ...s.threads.running, streaming: true },
      },
    }));

    useAgentChatStore.getState().evictColdThreads(["active"]);

    const remaining = Object.keys(useAgentChatStore.getState().threads);
    expect(remaining).toContain("active");
    expect(remaining).toContain("running");
    expect(remaining).not.toContain("idle");
  });

  it("never evicts a thread a pane is mounted on, even when idle", () => {
    // `keep` only names the thread that just hydrated. A second pane in a
    // split (or the pane the user is reading while another thread works)
    // is idle by every slice-level measure — and evicting it blanks the
    // transcript in place.
    const heavy = Math.ceil(WARM_SLICE_BUDGET_UNITS / 2);
    seedHeavyThread("hydrating", heavy);
    seedHeavyThread("mounted-idle", heavy);
    seedHeavyThread("gone", heavy);
    const unmount = registerMountedThread("mounted-idle");

    useAgentChatStore.getState().evictColdThreads(["hydrating"]);

    const remaining = Object.keys(useAgentChatStore.getState().threads);
    expect(remaining).toContain("mounted-idle");
    expect(remaining).not.toContain("gone");

    // Once the pane unmounts the same thread is an ordinary candidate.
    unmount();
    useAgentChatStore.getState().evictColdThreads(["hydrating"]);
    expect(Object.keys(useAgentChatStore.getState().threads)).not.toContain(
      "mounted-idle",
    );
  });

  it("never evicts a thread holding unsent composer work", () => {
    // A draft and a staged attachment exist nowhere but this slice —
    // eviction is data loss, not a cache miss.
    const heavy = Math.ceil(WARM_SLICE_BUDGET_UNITS / 2);
    seedHeavyThread("drafted", heavy);
    seedHeavyThread("attached", heavy);
    seedHeavyThread("empty-draft", heavy);
    useAgentChatStore.getState().setInputDraft("drafted", "half a thought");
    // Whitespace is not work.
    useAgentChatStore.getState().setInputDraft("empty-draft", "   ");
    useAgentChatStore.getState().addStagedAttachment("attached", {
      id: "att-1",
      kind: "file",
      ref: "notes.md",
      metadata: { label: "notes.md" },
    });

    useAgentChatStore.getState().evictColdThreads([]);

    const remaining = Object.keys(useAgentChatStore.getState().threads);
    expect(remaining).toContain("drafted");
    expect(remaining).toContain("attached");
    expect(remaining).not.toContain("empty-draft");
  });

  it("an evicted thread simply cold-hydrates again", () => {
    const heavy = Math.ceil(WARM_SLICE_BUDGET_UNITS + 10);
    seedHeavyThread("gone", heavy);
    useAgentChatStore.getState().evictColdThreads([]);
    expect(useAgentChatStore.getState().threads.gone).toBeUndefined();

    useAgentChatStore.getState().hydrateThread("gone", [
      row(3, assistantEvent("gone", "back")),
    ]);
    expect(texts("gone")).toEqual(["back"]);
    expect(useAgentChatStore.getState().threads.gone.lastPersistedEventId).toBe(3);
  });
});
