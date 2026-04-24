import { describe, it, expect, beforeEach } from "vitest";

import {
  DEFAULT_THREAD_PERMISSION_MODE,
  useAgentChatStore,
} from "./agent-chat-store";

// Reset the store between tests — Zustand's singleton keeps state
// across test cases inside the same suite otherwise.
function resetStore() {
  useAgentChatStore.setState({ threads: {} });
}

describe("agent-chat-store", () => {
  beforeEach(() => {
    resetStore();
  });

  describe("ensureThread / emptySlice defaults", () => {
    it("seeds a fresh slice with effort=null and contextWindow=null", () => {
      useAgentChatStore.getState().ensureThread("t1");
      const slice = useAgentChatStore.getState().threads["t1"];
      expect(slice).toBeDefined();
      expect(slice.effort).toBeNull();
      expect(slice.contextWindow).toBeNull();
      expect(slice.permissionMode).toBe(DEFAULT_THREAD_PERMISSION_MODE);
      expect(slice.model).toBeNull();
    });

    it("is idempotent — ensuring twice keeps the same slice instance", () => {
      useAgentChatStore.getState().ensureThread("t1");
      const first = useAgentChatStore.getState().threads["t1"];
      useAgentChatStore.getState().ensureThread("t1");
      const second = useAgentChatStore.getState().threads["t1"];
      expect(second).toBe(first);
    });
  });

  describe("setEffort", () => {
    it("writes the effort on an existing slice", () => {
      useAgentChatStore.getState().ensureThread("t1");
      useAgentChatStore.getState().setEffort("t1", "xhigh");
      expect(useAgentChatStore.getState().threads["t1"].effort).toBe("xhigh");
    });

    it("creates a slice if the thread id is unknown (updateSlice semantics)", () => {
      useAgentChatStore.getState().setEffort("new", "high");
      expect(useAgentChatStore.getState().threads["new"].effort).toBe("high");
    });

    it("is a no-op when the value is unchanged (same reference)", () => {
      useAgentChatStore.getState().ensureThread("t1");
      useAgentChatStore.getState().setEffort("t1", "high");
      const before = useAgentChatStore.getState().threads["t1"];
      useAgentChatStore.getState().setEffort("t1", "high");
      const after = useAgentChatStore.getState().threads["t1"];
      expect(after).toBe(before);
    });

    it("clears to null", () => {
      useAgentChatStore.getState().setEffort("t1", "high");
      useAgentChatStore.getState().setEffort("t1", null);
      expect(useAgentChatStore.getState().threads["t1"].effort).toBeNull();
    });
  });

  describe("setContextWindow", () => {
    it("writes the context window on an existing slice", () => {
      useAgentChatStore.getState().ensureThread("t1");
      useAgentChatStore.getState().setContextWindow("t1", "1m");
      expect(useAgentChatStore.getState().threads["t1"].contextWindow).toBe(
        "1m",
      );
    });

    it("is a no-op when the value is unchanged", () => {
      useAgentChatStore.getState().setContextWindow("t1", "1m");
      const before = useAgentChatStore.getState().threads["t1"];
      useAgentChatStore.getState().setContextWindow("t1", "1m");
      const after = useAgentChatStore.getState().threads["t1"];
      expect(after).toBe(before);
    });

    it("clears to null", () => {
      useAgentChatStore.getState().setContextWindow("t1", "1m");
      useAgentChatStore.getState().setContextWindow("t1", null);
      expect(
        useAgentChatStore.getState().threads["t1"].contextWindow,
      ).toBeNull();
    });
  });

  describe("effort + contextWindow survive migrateThreadId", () => {
    it("the silent-restart migration preserves effort + contextWindow", () => {
      useAgentChatStore.getState().ensureThread("old-thread");
      useAgentChatStore.getState().setEffort("old-thread", "xhigh");
      useAgentChatStore.getState().setContextWindow("old-thread", "1m");
      useAgentChatStore.getState().migrateThreadId("old-thread", "new-thread");
      const migrated = useAgentChatStore.getState().threads["new-thread"];
      expect(migrated.effort).toBe("xhigh");
      expect(migrated.contextWindow).toBe("1m");
      expect(
        useAgentChatStore.getState().threads["old-thread"],
      ).toBeUndefined();
    });

    it("permissionMode survives migrateThreadId (silent restart)", () => {
      useAgentChatStore.getState().ensureThread("old-thread");
      useAgentChatStore
        .getState()
        .setPermissionMode("old-thread", "acceptEdits");
      useAgentChatStore
        .getState()
        .migrateThreadId("old-thread", "new-thread");
      expect(
        useAgentChatStore.getState().threads["new-thread"].permissionMode,
      ).toBe("acceptEdits");
    });
  });

  describe("setPermissionMode", () => {
    it("writes the value on an existing slice", () => {
      useAgentChatStore.getState().ensureThread("t1");
      useAgentChatStore
        .getState()
        .setPermissionMode("t1", "read-only");
      expect(useAgentChatStore.getState().threads["t1"].permissionMode).toBe(
        "read-only",
      );
    });

    it("is a no-op when unchanged", () => {
      useAgentChatStore.getState().setPermissionMode("t1", "workspace-write");
      const before = useAgentChatStore.getState().threads["t1"];
      useAgentChatStore.getState().setPermissionMode("t1", "workspace-write");
      const after = useAgentChatStore.getState().threads["t1"];
      expect(after).toBe(before);
    });
  });

  describe("hydrateThread", () => {
    function userPayload(text: string): string {
      return JSON.stringify({ type: "user_message", thread_id: "t", text });
    }
    function assistantPayload(turnId: string, text: string): string {
      return JSON.stringify({
        type: "item_completed",
        thread_id: "t",
        turn_id: turnId,
        item: { kind: "assistant_text", text },
      });
    }

    it("creates a slice with the replayed transcript when the thread is unknown", () => {
      useAgentChatStore
        .getState()
        .hydrateThread("fresh", [
          userPayload("hi"),
          assistantPayload("turn-1", "hello back"),
        ]);
      const slice = useAgentChatStore.getState().threads["fresh"];
      expect(slice).toBeDefined();
      expect(slice.messages).toHaveLength(2);
      expect(slice.messages[0].kind).toBe("user_message");
      expect(slice.messages[1].kind).toBe("assistant_message");
      // Defaults preserved on the slice's UI fields.
      expect(slice.permissionMode).toBe(DEFAULT_THREAD_PERMISSION_MODE);
      expect(slice.model).toBeNull();
      expect(slice.streaming).toBe(false);
      expect(slice.activeTurnId).toBeNull();
    });

    it("preserves UI-only fields on an existing slice", () => {
      // The user picked a model, mode, etc. Resume populates the
      // transcript but must not bulldoze those choices, otherwise
      // the picker pills would silently revert on every resume.
      useAgentChatStore.getState().ensureThread("t");
      useAgentChatStore.getState().setModel("t", "claude-opus-4-7");
      useAgentChatStore.getState().setPermissionMode("t", "acceptEdits");
      useAgentChatStore.getState().setMode("t", "plan");
      useAgentChatStore.getState().setEffort("t", "high");
      useAgentChatStore.getState().setContextWindow("t", "1m");
      useAgentChatStore.getState().setInputDraft("t", "wip text");

      useAgentChatStore
        .getState()
        .hydrateThread("t", [userPayload("recovered")]);

      const after = useAgentChatStore.getState().threads["t"];
      expect(after.model).toBe("claude-opus-4-7");
      expect(after.permissionMode).toBe("acceptEdits");
      expect(after.mode).toBe("plan");
      expect(after.effort).toBe("high");
      expect(after.contextWindow).toBe("1m");
      expect(after.inputDraft).toBe("wip text");
      // Replayed transcript replaced.
      expect(after.messages).toHaveLength(1);
    });

    it("replaces the messages list — does not append to existing transcript", () => {
      // If the user resumed onto an already-populated slice (rare:
      // SessionSelector resets first, but we still guarantee the
      // contract for clarity), the new transcript fully overwrites.
      useAgentChatStore.getState().ensureThread("t");
      useAgentChatStore.getState().appendUserMessage("t", "stale");
      expect(useAgentChatStore.getState().threads["t"].messages).toHaveLength(
        1,
      );

      useAgentChatStore
        .getState()
        .hydrateThread("t", [userPayload("fresh-1"), userPayload("fresh-2")]);

      const after = useAgentChatStore.getState().threads["t"];
      expect(after.messages).toHaveLength(2);
      const texts = after.messages
        .filter((m) => m.kind === "user_message")
        .map((m) => (m.kind === "user_message" ? m.text : ""));
      expect(texts).toEqual(["fresh-1", "fresh-2"]);
    });

    it("clears ephemeral turn-state fields", () => {
      useAgentChatStore.getState().ensureThread("t");
      // Simulate a stale active turn from a prior session that
      // never sealed cleanly.
      useAgentChatStore.setState((s) => ({
        threads: {
          ...s.threads,
          t: { ...s.threads.t, activeTurnId: "turn-stuck" },
        },
      }));
      useAgentChatStore.getState().hydrateThread("t", []);
      expect(useAgentChatStore.getState().threads["t"].activeTurnId).toBeNull();
    });

    it("ignores malformed payloads instead of throwing", () => {
      // A single corrupt row must not block resume.
      expect(() =>
        useAgentChatStore
          .getState()
          .hydrateThread("t", ["not-json", userPayload("ok")]),
      ).not.toThrow();
      const slice = useAgentChatStore.getState().threads["t"];
      expect(slice.messages).toHaveLength(1);
    });

    it("handles an empty payload list (resume of a never-used session)", () => {
      useAgentChatStore.getState().hydrateThread("empty", []);
      const slice = useAgentChatStore.getState().threads["empty"];
      expect(slice).toBeDefined();
      expect(slice.messages).toEqual([]);
      expect(slice.streaming).toBe(false);
    });
  });
});
