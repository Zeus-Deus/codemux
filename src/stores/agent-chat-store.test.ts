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
});
