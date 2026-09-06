import { describe, it, expect, beforeEach } from "vitest";

import {
  DEFAULT_THREAD_PERMISSION_MODE,
  useAgentChatStore,
  type Attachment,
} from "./agent-chat-store";

function makeFileAttachment(overrides: Partial<Attachment> = {}): Attachment {
  return {
    id: "att-1",
    kind: "file",
    ref: "src/components/chat/Composer.tsx",
    metadata: { label: "Composer.tsx", lineCount: 421 },
    ...overrides,
  };
}

// Reset the store between tests — Zustand's singleton keeps state
// across test cases inside the same suite otherwise.
function resetStore() {
  useAgentChatStore.setState({ threads: {} });
}

describe("agent-chat-store", () => {
  it("keeps the active turn when an async question is answered", () => {
    resetStore();
    const store = useAgentChatStore.getState();
    store.ensureThread("async");
    store.applyEvent("async", {
      type: "session_state_changed",
      thread_id: "async",
      status: { status: "running", active_turn: "turn" },
    });
    store.applyEvent("async", {
      type: "questions_asked",
      thread_id: "async",
      question: {
        id: "q",
        target: "native",
        source_item_id: "q",
        source_turn_id: "turn",
        text: "",
        questions: [{ title: "Storage?", options: [] }],
      },
    });
    store.applyEvent("async", {
      type: "question_resolved",
      thread_id: "async",
      question_id: "q",
      resolution: {
        status: "answered",
        answers: ["SQLite"],
        submission_id: "reply",
        delivery: { kind: "inflight", turn_id: "turn" },
      },
    });
    expect(useAgentChatStore.getState().threads.async.activeTurnId).toBe(
      "turn",
    );
    expect(useAgentChatStore.getState().threads.async.streaming).toBe(true);
  });
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
      // Stage 6 debug-flag defaults.
      expect(slice.hasDebugActivity).toBe(false);
      expect(slice.debugActivityResolved).toBe(false);
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

    it("the context-usage snapshot survives migrateThreadId", () => {
      // The snapshot lives inside the reducer thread state, so the
      // silent restart carries it with the rest of the slice — the
      // meter must not blank out when the session id changes.
      useAgentChatStore.getState().ensureThread("old-thread");
      useAgentChatStore.getState().applyEvent("old-thread", {
        type: "context_usage_updated",
        thread_id: "old-thread",
        usage: { used_tokens: 44_000, max_tokens: 200_000 },
      });
      useAgentChatStore.getState().migrateThreadId("old-thread", "new-thread");
      expect(
        useAgentChatStore.getState().threads["new-thread"].contextUsage,
      ).toMatchObject({ used_tokens: 44_000, max_tokens: 200_000 });
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

  describe("setHasDebugActivity / setDebugActivityResolved", () => {
    it("setHasDebugActivity flips the slice flag and is a no-op on unchanged value", () => {
      useAgentChatStore.getState().ensureThread("t1");
      useAgentChatStore.getState().setHasDebugActivity("t1", true);
      expect(
        useAgentChatStore.getState().threads["t1"].hasDebugActivity,
      ).toBe(true);
      const before = useAgentChatStore.getState().threads["t1"];
      useAgentChatStore.getState().setHasDebugActivity("t1", true);
      expect(useAgentChatStore.getState().threads["t1"]).toBe(before);
    });

    it("setDebugActivityResolved flips the resolved flag and is a no-op on unchanged value", () => {
      useAgentChatStore.getState().ensureThread("t1");
      useAgentChatStore.getState().setDebugActivityResolved("t1", true);
      expect(
        useAgentChatStore.getState().threads["t1"].debugActivityResolved,
      ).toBe(true);
      const before = useAgentChatStore.getState().threads["t1"];
      useAgentChatStore.getState().setDebugActivityResolved("t1", true);
      expect(useAgentChatStore.getState().threads["t1"]).toBe(before);
    });
  });

  // End-to-end over the store action, which is where the `previousUnsettled`
  // seed actually lives. Nothing here was covered before: this file had zero
  // assertions on `interrupted`.
  describe("applyPayloadsTail — interrupted across a remount", () => {
    function rows(startId: number, ...payloads: string[]) {
      return payloads.map((payload, index) => ({
        id: startId + index,
        payload,
      }));
    }
    const subagentRow = (status: string) =>
      JSON.stringify({
        type: "subagent_updated",
        thread_id: "t",
        subagent: { subagent_id: "sub-1", name: "Explore", status },
      });

    /** Drive a thread into an interim hold through the live event path,
     *  then park a cursor on it — a pane that watched a delegated `Task`
     *  start and is about to be unmounted by a tab switch. */
    function seedInterimHold() {
      const store = useAgentChatStore.getState();
      store.ensureThread("t");
      store.applyEvent("t", {
        type: "user_message",
        thread_id: "t",
        text: "go",
      } as never);
      store.applyEvent("t", JSON.parse(subagentRow("running")));
      store.applyEvent("t", {
        type: "turn_completed",
        thread_id: "t",
        turn_id: "turn-1",
        status: { kind: "success" },
      } as never);
      useAgentChatStore.setState((state) => ({
        threads: {
          ...state.threads,
          t: { ...state.threads.t, lastPersistedEventId: 10 },
        },
      }));
      return useAgentChatStore.getState().threads["t"];
    }

    it("keeps a delegated-work hold live instead of showing Continue", () => {
      const before = seedInterimHold();
      expect(before.streaming).toBe(true);
      expect(before.turnUnsettled).toBe(false);

      // Remount: the tail is the subagent rows persisted while unmounted,
      // and the backend reports the run live because delegated work holds it.
      useAgentChatStore
        .getState()
        .applyPayloadsTail("t", rows(11, subagentRow("running")), {
          runLive: true,
          provider: "claude",
        });

      const after = useAgentChatStore.getState().threads["t"];
      expect(after.interrupted).toBe(false);
      expect(after.streaming).toBe(true);
      expect(after.interrupted && !after.streaming).toBe(false);
    });

    it("does not resurrect an unsettled tail from the streaming flag alone", () => {
      // The narrow regression: even if the probe comes back false (a stale
      // read, a slow tracker), the tail scan must not INVENT an unsettled
      // turn — history genuinely ended on a `turn_completed`.
      seedInterimHold();
      useAgentChatStore
        .getState()
        .applyPayloadsTail("t", rows(11, subagentRow("running")), {
          runLive: false,
          provider: "claude",
        });
      expect(useAgentChatStore.getState().threads["t"].interrupted).toBe(false);
    });

    it("still raises the divider for a turn that never settled", () => {
      const store = useAgentChatStore.getState();
      store.ensureThread("t");
      store.applyEvent("t", {
        type: "user_message",
        thread_id: "t",
        text: "go",
      } as never);
      useAgentChatStore.setState((state) => ({
        threads: {
          ...state.threads,
          t: { ...state.threads.t, lastPersistedEventId: 10 },
        },
      }));
      expect(useAgentChatStore.getState().threads["t"].turnUnsettled).toBe(true);

      useAgentChatStore
        .getState()
        .applyPayloadsTail("t", rows(11, subagentRow("running")), {
          runLive: false,
          provider: "claude",
        });

      const after = useAgentChatStore.getState().threads["t"];
      expect(after.interrupted).toBe(true);
      expect(after.streaming).toBe(false);
    });
  });

  describe("hydrateThread", () => {
    /** Wrap raw payloads as cursor rows (ids ascending from 1), the shape
     *  `agent_chat_list_messages_after` returns. */
    function rows(...payloads: string[]) {
      return payloads.map((payload, index) => ({ id: index + 1, payload }));
    }
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
        .hydrateThread(
          "fresh",
          rows(userPayload("hi"), assistantPayload("turn-1", "hello back")),
        );
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
        .hydrateThread("t", rows(userPayload("recovered")));

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
        .hydrateThread("t", rows(userPayload("fresh-1"), userPayload("fresh-2")));

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
          .hydrateThread("t", rows("not-json", userPayload("ok"))),
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

  describe("user turns fanned out to a second client", () => {
    // Providers never echo user turns, so the persisted row is the only
    // record of one. The backend now fans that row out live to every client
    // attached to the thread. What these tests pin is WHY: without the
    // fan-out, a second client received only the assistant reply — persisted
    // at a HIGHER id — and `applyLiveEvents` walked its cursor past a user
    // row it had never seen, after which every `id > cursor` tail read
    // skipped that row permanently.

    function userEvent(text: string, clientNonce?: string) {
      return {
        type: "user_message" as const,
        thread_id: "t",
        text,
        ...(clientNonce ? { client_nonce: clientNonce } : {}),
      };
    }

    function assistantEvent(text: string, turnId = "turn-1") {
      return {
        type: "item_completed" as const,
        thread_id: "t",
        turn_id: turnId,
        item: { kind: "assistant_text" as const, text },
      };
    }

    function userTexts(threadId: string): string[] {
      return useAgentChatStore
        .getState()
        .threads[threadId].messages.filter((m) => m.kind === "user_message")
        .map((m) => (m.kind === "user_message" ? m.text : ""));
    }

    /** A warm, synced slice — the state a second client is in while it
     *  watches a thread someone else is typing into. */
    function warmSlice(cursor: number) {
      useAgentChatStore.getState().hydrateThread("t", []);
      useAgentChatStore.setState((s) => ({
        threads: { ...s.threads, t: { ...s.threads.t, lastPersistedEventId: cursor } },
      }));
    }

    it("renders the other client's turn and advances the cursor over its row", () => {
      warmSlice(10);

      // The phone sends; the desktop receives the persisted row live, then
      // the assistant reply that follows it.
      useAgentChatStore
        .getState()
        .applyLiveEvents("t", [
          { event: userEvent("from the phone", "nonce-phone"), persistedId: 11 },
          { event: assistantEvent("on it"), persistedId: 12 },
        ]);

      const slice = useAgentChatStore.getState().threads["t"];
      expect(userTexts("t")).toEqual(["from the phone"]);
      expect(slice.messages).toHaveLength(2);
      // The cursor moved over the user row, not past it — so the next tail
      // read starts at 12 having actually seen 11.
      expect(slice.lastPersistedEventId).toBe(12);
    });

    it("the sending client dedups its own turn by nonce", () => {
      warmSlice(10);
      // The composer already painted this bubble optimistically.
      useAgentChatStore
        .getState()
        .appendUserMessage("t", "from this client", "nonce-mine");
      expect(userTexts("t")).toEqual(["from this client"]);

      // The same turn comes back as the fan-out of its persisted row.
      useAgentChatStore
        .getState()
        .applyLiveEvents("t", [
          { event: userEvent("from this client", "nonce-mine"), persistedId: 11 },
        ]);

      const slice = useAgentChatStore.getState().threads["t"];
      expect(userTexts("t")).toEqual(["from this client"]); // not doubled
      // Deduped in the transcript, but the cursor still owns the row — the
      // sender must not re-read a row it has already rendered.
      expect(slice.lastPersistedEventId).toBe(11);
    });

    it("a tail read that overlaps the live fan-out does not double the bubble", () => {
      // The other client's turn arrives live at id 11, and a hydrate tail
      // read straddling the same instant returns row 11 again. The
      // persisted-id guard drops the replay; the nonce guard is the second
      // net under it.
      warmSlice(10);
      useAgentChatStore
        .getState()
        .applyLiveEvents("t", [
          { event: userEvent("overlapping", "nonce-x"), persistedId: 11 },
        ]);
      expect(userTexts("t")).toEqual(["overlapping"]);

      useAgentChatStore
        .getState()
        .applyLiveEvents("t", [
          { event: userEvent("overlapping", "nonce-x"), persistedId: 11 },
        ]);

      expect(userTexts("t")).toEqual(["overlapping"]);
      expect(
        useAgentChatStore.getState().threads["t"].lastPersistedEventId,
      ).toBe(11);
    });

    it("a nonce-less turn from another client still lands", () => {
      // Older senders (and any caller that supplies no correlation token)
      // must not be invisible to other clients just because they cannot be
      // deduped — insertion is the correct default.
      warmSlice(10);
      useAgentChatStore
        .getState()
        .applyLiveEvents("t", [{ event: userEvent("no nonce"), persistedId: 11 }]);

      expect(userTexts("t")).toEqual(["no nonce"]);
      expect(
        useAgentChatStore.getState().threads["t"].lastPersistedEventId,
      ).toBe(11);
    });

    it("hydrating the same rows afterwards reproduces one bubble, not two", () => {
      // Live fan-out and replay share one reducer case, so a client that saw
      // the turn live and a client that only ever replays the rows must end
      // up with identical transcripts.
      warmSlice(10);
      useAgentChatStore
        .getState()
        .applyLiveEvents("t", [
          { event: userEvent("shared", "nonce-s"), persistedId: 11 },
        ]);

      useAgentChatStore
        .getState()
        .hydrateThread("t", [
          {
            id: 11,
            payload: JSON.stringify({
              type: "user_message",
              thread_id: "t",
              text: "shared",
              client_nonce: "nonce-s",
            }),
          },
        ]);

      expect(userTexts("t")).toEqual(["shared"]);
    });
  });

  describe("stagedAttachments (Step 8 Stage 1)", () => {
    it("seeds a fresh slice with an empty stagedAttachments array", () => {
      useAgentChatStore.getState().ensureThread("t1");
      const slice = useAgentChatStore.getState().threads["t1"];
      expect(slice.stagedAttachments).toEqual([]);
    });

    it("addStagedAttachment appends to the array", () => {
      useAgentChatStore.getState().ensureThread("t1");
      const att = makeFileAttachment();
      useAgentChatStore.getState().addStagedAttachment("t1", att);
      const slice = useAgentChatStore.getState().threads["t1"];
      expect(slice.stagedAttachments).toEqual([att]);
    });

    it("addStagedAttachment creates a slice if the thread is unknown", () => {
      const att = makeFileAttachment();
      useAgentChatStore.getState().addStagedAttachment("new-thread", att);
      const slice = useAgentChatStore.getState().threads["new-thread"];
      expect(slice).toBeDefined();
      expect(slice.stagedAttachments).toEqual([att]);
    });

    it("preserves insertion order across multiple adds", () => {
      useAgentChatStore.getState().ensureThread("t1");
      const a = makeFileAttachment({ id: "a", metadata: { label: "A.ts" } });
      const b = makeFileAttachment({ id: "b", metadata: { label: "B.ts" } });
      const c = makeFileAttachment({ id: "c", metadata: { label: "C.ts" } });
      useAgentChatStore.getState().addStagedAttachment("t1", a);
      useAgentChatStore.getState().addStagedAttachment("t1", b);
      useAgentChatStore.getState().addStagedAttachment("t1", c);
      const ids = useAgentChatStore
        .getState()
        .threads["t1"].stagedAttachments.map((x) => x.id);
      expect(ids).toEqual(["a", "b", "c"]);
    });

    it("updateStagedAttachment patches metadata without clobbering siblings", () => {
      useAgentChatStore.getState().ensureThread("t1");
      const att = makeFileAttachment({
        metadata: { label: "Composer.tsx", lineCount: 421, isLoading: true },
      });
      useAgentChatStore.getState().addStagedAttachment("t1", att);
      useAgentChatStore.getState().updateStagedAttachment("t1", "att-1", {
        metadata: { label: "Composer.tsx", isLoading: false, fetchedAt: 123 },
      });
      const next = useAgentChatStore
        .getState()
        .threads["t1"].stagedAttachments[0];
      expect(next.metadata.isLoading).toBe(false);
      expect(next.metadata.fetchedAt).toBe(123);
      // lineCount survives the patch (one-level merge).
      expect(next.metadata.lineCount).toBe(421);
    });

    it("updateStagedAttachment patches resolvedContent", () => {
      useAgentChatStore.getState().ensureThread("t1");
      useAgentChatStore.getState().addStagedAttachment("t1", makeFileAttachment());
      useAgentChatStore
        .getState()
        .updateStagedAttachment("t1", "att-1", {
          resolvedContent: "// composer body",
        });
      const next = useAgentChatStore
        .getState()
        .threads["t1"].stagedAttachments[0];
      expect(next.resolvedContent).toBe("// composer body");
    });

    it("updateStagedAttachment is a no-op for unknown id", () => {
      useAgentChatStore.getState().ensureThread("t1");
      useAgentChatStore.getState().addStagedAttachment("t1", makeFileAttachment());
      const before = useAgentChatStore.getState().threads["t1"];
      useAgentChatStore.getState().updateStagedAttachment("t1", "nope", {
        metadata: { label: "ghost" },
      });
      const after = useAgentChatStore.getState().threads["t1"];
      expect(after).toBe(before);
    });

    it("removeStagedAttachment drops a single attachment by id", () => {
      useAgentChatStore.getState().ensureThread("t1");
      const a = makeFileAttachment({ id: "a", metadata: { label: "A.ts" } });
      const b = makeFileAttachment({ id: "b", metadata: { label: "B.ts" } });
      useAgentChatStore.getState().addStagedAttachment("t1", a);
      useAgentChatStore.getState().addStagedAttachment("t1", b);
      useAgentChatStore.getState().removeStagedAttachment("t1", "a");
      const ids = useAgentChatStore
        .getState()
        .threads["t1"].stagedAttachments.map((x) => x.id);
      expect(ids).toEqual(["b"]);
    });

    it("removeStagedAttachment is a no-op for unknown id (same reference)", () => {
      useAgentChatStore.getState().ensureThread("t1");
      useAgentChatStore.getState().addStagedAttachment("t1", makeFileAttachment());
      const before = useAgentChatStore.getState().threads["t1"];
      useAgentChatStore.getState().removeStagedAttachment("t1", "nope");
      const after = useAgentChatStore.getState().threads["t1"];
      expect(after).toBe(before);
    });

    it("clearStagedAttachments empties the array", () => {
      useAgentChatStore.getState().ensureThread("t1");
      useAgentChatStore
        .getState()
        .addStagedAttachment("t1", makeFileAttachment({ id: "a" }));
      useAgentChatStore
        .getState()
        .addStagedAttachment("t1", makeFileAttachment({ id: "b" }));
      useAgentChatStore.getState().clearStagedAttachments("t1");
      expect(
        useAgentChatStore.getState().threads["t1"].stagedAttachments,
      ).toEqual([]);
    });

    it("clearStagedAttachments is a no-op when already empty (same reference)", () => {
      useAgentChatStore.getState().ensureThread("t1");
      const before = useAgentChatStore.getState().threads["t1"];
      useAgentChatStore.getState().clearStagedAttachments("t1");
      const after = useAgentChatStore.getState().threads["t1"];
      expect(after).toBe(before);
    });

    it("does not enforce a cap — slice accepts 21+ attachments", () => {
      // Cap (20 hard / 10 soft warn) is enforced by the composer UI,
      // not by the slice. Tests should not see ergonomic rejection.
      useAgentChatStore.getState().ensureThread("t1");
      for (let i = 0; i < 25; i += 1) {
        useAgentChatStore
          .getState()
          .addStagedAttachment(
            "t1",
            makeFileAttachment({ id: `a-${i}`, metadata: { label: `f${i}` } }),
          );
      }
      expect(
        useAgentChatStore.getState().threads["t1"].stagedAttachments,
      ).toHaveLength(25);
    });

    it("survives migrateThreadId — the silent restart preserves attachments", () => {
      useAgentChatStore.getState().ensureThread("old");
      useAgentChatStore.getState().addStagedAttachment("old", makeFileAttachment());
      useAgentChatStore.getState().migrateThreadId("old", "new");
      expect(useAgentChatStore.getState().threads["old"]).toBeUndefined();
      expect(
        useAgentChatStore.getState().threads["new"].stagedAttachments,
      ).toHaveLength(1);
    });
  });
});
