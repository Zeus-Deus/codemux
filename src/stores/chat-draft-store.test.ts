import { beforeEach, describe, expect, it } from "vitest";

import {
  selectActiveDraft,
  selectDraftForWorkspace,
  useChatDraftStore,
  type ChatDraft,
  type DraftId,
} from "./chat-draft-store";

function resetStore() {
  useChatDraftStore.setState({
    draftsById: {},
    activeHomeDraftId: null,
    projectDraftIdByPath: {},
    activeDraftId: null,
  });
}

describe("chat-draft-store", () => {
  beforeEach(() => {
    resetStore();
    // Clear localStorage between tests so persistence round-trip tests
    // do not see leftover state from earlier cases.
    if (typeof window !== "undefined") {
      try {
        window.localStorage.removeItem("codemux:chat-drafts:v1");
      } catch {
        // Ignore — jsdom/happy-dom should always expose localStorage.
      }
    }
  });

  describe("getOrCreateHomeDraft", () => {
    it("creates a new draft on first call", () => {
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      expect(draft.target).toEqual({ kind: "home" });
      expect(draft.provider).toBe("claude");
      expect(draft.permissionMode).toBe("bypassPermissions");
      // Default model is seeded so capability-dependent pickers
      // (Effort, ContextWindow) render on first paint. In the test
      // environment the `provider-capabilities-store` is not
      // hydrated, so `defaultModelId` returns the hardcoded fallback
      // `claude-opus-4-8`, and `capabilityDefaults` returns null
      // effort / contextWindow because the model payload isn't
      // available yet. The Stage C Effort-lock fix means these null
      // values flow through materialize into the slice; real runtime
      // gets non-null values once caps hydrate.
      expect(draft.model).toBe("claude-opus-4-8");
      expect(draft.effort).toBeNull();
      expect(draft.contextWindow).toBeNull();
      expect(draft.threadId).toBeTruthy();
      expect(draft.threadId).not.toBe(draft.draftId);
      expect(useChatDraftStore.getState().activeHomeDraftId).toBe(draft.draftId);
    });

    it("returns the existing draft on a second call — single home slot", () => {
      const first = useChatDraftStore.getState().getOrCreateHomeDraft();
      const second = useChatDraftStore.getState().getOrCreateHomeDraft();
      expect(second.draftId).toBe(first.draftId);
      expect(Object.keys(useChatDraftStore.getState().draftsById)).toHaveLength(1);
    });

    it("creates a fresh draft after the previous one is marked promoted", () => {
      const first = useChatDraftStore.getState().getOrCreateHomeDraft();
      useChatDraftStore.getState().markPromoted(first.draftId, {
        workspaceId: "ws-1",
        paneId: "pane-1",
        threadId: first.threadId,
      });
      const second = useChatDraftStore.getState().getOrCreateHomeDraft();
      expect(second.draftId).not.toBe(first.draftId);
    });

    it("defaults to `lockedToHome: false` for implicit (empty-state) calls", () => {
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      expect(draft.lockedToHome).toBe(false);
    });

    it("propagates `lockedToHome: true` for explicit New-agent calls", () => {
      const draft = useChatDraftStore
        .getState()
        .getOrCreateHomeDraft({ lockedToHome: true });
      expect(draft.lockedToHome).toBe(true);
    });

    it("does NOT reuse an implicit pristine draft when an explicit lockedToHome draft is requested", () => {
      // The implicit empty-state path may have already minted a home
      // draft whose target the mount-effect auto-seeded to an existing
      // workspace. Reusing it for an explicit "New agent" click would
      // defeat the lockedToHome promise — make sure a fresh draft is
      // minted instead.
      const implicit = useChatDraftStore.getState().getOrCreateHomeDraft();
      const explicit = useChatDraftStore
        .getState()
        .getOrCreateHomeDraft({ lockedToHome: true });
      expect(explicit.draftId).not.toBe(implicit.draftId);
      expect(explicit.lockedToHome).toBe(true);
    });

    it("does reuse a pristine lockedToHome draft on repeated explicit clicks", () => {
      const first = useChatDraftStore
        .getState()
        .getOrCreateHomeDraft({ lockedToHome: true });
      const second = useChatDraftStore
        .getState()
        .getOrCreateHomeDraft({ lockedToHome: true });
      expect(second.draftId).toBe(first.draftId);
    });
  });

  describe("getOrCreateProjectDraft", () => {
    it("creates one draft per project path", () => {
      const a = useChatDraftStore.getState().getOrCreateProjectDraft("/a");
      const b = useChatDraftStore.getState().getOrCreateProjectDraft("/b");
      const aAgain = useChatDraftStore.getState().getOrCreateProjectDraft("/a");
      expect(aAgain.draftId).toBe(a.draftId);
      expect(b.draftId).not.toBe(a.draftId);
      expect(useChatDraftStore.getState().projectDraftIdByPath).toEqual({
        "/a": a.draftId,
        "/b": b.draftId,
      });
    });

    it("does not collide with the home-slot draft", () => {
      const home = useChatDraftStore.getState().getOrCreateHomeDraft();
      const project = useChatDraftStore.getState().getOrCreateProjectDraft("/x");
      expect(home.draftId).not.toBe(project.draftId);
      expect(project.target).toEqual({ kind: "project", projectPath: "/x" });
    });
  });

  describe("updateDraftTarget", () => {
    it("rewrites target without wiping composer content", () => {
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      useChatDraftStore.getState().updateDraftInput(draft.draftId, "half-written");
      useChatDraftStore
        .getState()
        .updateDraftTarget(draft.draftId, { kind: "project", projectPath: "/x" });
      const next = useChatDraftStore.getState().draftsById[draft.draftId];
      expect(next.inputDraft).toBe("half-written");
      expect(next.target).toEqual({ kind: "project", projectPath: "/x" });
    });

    it("moves slot pointers when the target kind changes", () => {
      const home = useChatDraftStore.getState().getOrCreateHomeDraft();
      expect(useChatDraftStore.getState().activeHomeDraftId).toBe(home.draftId);
      useChatDraftStore
        .getState()
        .updateDraftTarget(home.draftId, { kind: "project", projectPath: "/p" });
      expect(useChatDraftStore.getState().activeHomeDraftId).toBeNull();
      expect(useChatDraftStore.getState().projectDraftIdByPath["/p"]).toBe(home.draftId);

      useChatDraftStore.getState().updateDraftTarget(home.draftId, { kind: "home" });
      expect(useChatDraftStore.getState().activeHomeDraftId).toBe(home.draftId);
      expect(useChatDraftStore.getState().projectDraftIdByPath["/p"]).toBeUndefined();
    });

    it("updates the project slot when moving between project paths", () => {
      const d = useChatDraftStore.getState().getOrCreateProjectDraft("/a");
      useChatDraftStore
        .getState()
        .updateDraftTarget(d.draftId, { kind: "project", projectPath: "/b" });
      const map = useChatDraftStore.getState().projectDraftIdByPath;
      expect(map["/a"]).toBeUndefined();
      expect(map["/b"]).toBe(d.draftId);
    });
  });

  describe("updateDraftConfig", () => {
    it("preserves composer content when session config changes", () => {
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      useChatDraftStore.getState().updateDraftInput(draft.draftId, "hello");
      useChatDraftStore
        .getState()
        .updateDraftConfig(draft.draftId, { model: "claude-sonnet-4-6", effort: "high" });
      const next = useChatDraftStore.getState().draftsById[draft.draftId];
      expect(next.inputDraft).toBe("hello");
      expect(next.model).toBe("claude-sonnet-4-6");
      expect(next.effort).toBe("high");
    });

  });

  describe("promotion lifecycle", () => {
    it("markPromoting sets promoting and clears any prior error", () => {
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      useChatDraftStore.getState().markSendFailed(draft.draftId, "boom");
      useChatDraftStore.getState().markPromoting(draft.draftId);
      const next = useChatDraftStore.getState().draftsById[draft.draftId];
      expect(next.promoting).toBe(true);
      expect(next.lastSendError).toBeNull();
    });

    it("markPromoted clears slot pointers but keeps the draft entry", () => {
      const draft = useChatDraftStore.getState().getOrCreateProjectDraft("/p");
      useChatDraftStore.getState().markPromoted(draft.draftId, {
        workspaceId: "ws-1",
        paneId: "pane-1",
        threadId: draft.threadId,
      });
      expect(useChatDraftStore.getState().projectDraftIdByPath["/p"]).toBeUndefined();
      expect(useChatDraftStore.getState().draftsById[draft.draftId]).toBeDefined();
      expect(
        useChatDraftStore.getState().draftsById[draft.draftId].promotedTo?.workspaceId,
      ).toBe("ws-1");
    });

    it("markSendFailed stores the error without promoting", () => {
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      useChatDraftStore.getState().markSendFailed(draft.draftId, "git lock");
      const next = useChatDraftStore.getState().draftsById[draft.draftId];
      expect(next.promoting).toBe(false);
      expect(next.lastSendError).toBe("git lock");
      expect(next.promotedTo).toBeNull();
    });
  });

  describe("clearDraft", () => {
    it("removes the draft and unsets any slot pointers", () => {
      const draft = useChatDraftStore.getState().getOrCreateProjectDraft("/p");
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
      useChatDraftStore.getState().clearDraft(draft.draftId);
      const state = useChatDraftStore.getState();
      expect(state.draftsById[draft.draftId]).toBeUndefined();
      expect(state.projectDraftIdByPath["/p"]).toBeUndefined();
      expect(state.activeDraftId).toBeNull();
    });
  });

  describe("clearDraftsForProject", () => {
    it("removes every draft targeting a given project path", () => {
      const store = useChatDraftStore.getState();
      const d1 = store.getOrCreateProjectDraft("/p");
      const d2 = store.getOrCreateProjectDraft("/q");
      // Force a second draft against /p for testing completeness.
      const extraId = "extra" as DraftId;
      useChatDraftStore.setState((s) => {
        const extra: ChatDraft = {
          ...d2,
          draftId: extraId,
          target: { kind: "project", projectPath: "/p" },
        };
        return {
          draftsById: { ...s.draftsById, [extraId]: extra },
        };
      });
      useChatDraftStore.getState().clearDraftsForProject("/p", null);
      const state = useChatDraftStore.getState();
      expect(state.draftsById[d1.draftId]).toBeUndefined();
      expect(state.draftsById[extraId]).toBeUndefined();
      expect(state.draftsById[d2.draftId]).toBeDefined();
      expect(state.projectDraftIdByPath["/p"]).toBeUndefined();
      expect(state.projectDraftIdByPath["/q"]).toBe(d2.draftId);
    });

    it("leaves home-target drafts alone when projectPath ≠ homeDir", () => {
      const store = useChatDraftStore.getState();
      const homeDraft = store.getOrCreateHomeDraft();
      const projectDraft = store.getOrCreateProjectDraft("/p");
      useChatDraftStore
        .getState()
        .clearDraftsForProject("/p", "/home/user");
      const state = useChatDraftStore.getState();
      expect(state.draftsById[projectDraft.draftId]).toBeUndefined();
      expect(state.draftsById[homeDraft.draftId]).toBeDefined();
      expect(state.activeHomeDraftId).toBe(homeDraft.draftId);
    });

    it("also sweeps home-target drafts when projectPath === homeDir (closing the Home group)", () => {
      const store = useChatDraftStore.getState();
      const homeDraft = store.getOrCreateHomeDraft();
      store.setActiveDraft(homeDraft.draftId);
      const projectDraft = store.getOrCreateProjectDraft("/other");
      expect(useChatDraftStore.getState().activeHomeDraftId).toBe(
        homeDraft.draftId,
      );

      useChatDraftStore
        .getState()
        .clearDraftsForProject("/home/user", "/home/user");

      const state = useChatDraftStore.getState();
      expect(state.draftsById[homeDraft.draftId]).toBeUndefined();
      expect(state.activeHomeDraftId).toBeNull();
      expect(state.activeDraftId).toBeNull();
      // Untouched: a draft for an unrelated project stays.
      expect(state.draftsById[projectDraft.draftId]).toBeDefined();
    });

    it("does NOT sweep home-target drafts when homeDir is null (fall-back safety)", () => {
      const store = useChatDraftStore.getState();
      const homeDraft = store.getOrCreateHomeDraft();
      useChatDraftStore
        .getState()
        .clearDraftsForProject("/home/user", null);
      expect(
        useChatDraftStore.getState().draftsById[homeDraft.draftId],
      ).toBeDefined();
    });
  });

  describe("selectors", () => {
    it("selectActiveDraft returns null when none is active", () => {
      expect(selectActiveDraft(useChatDraftStore.getState())).toBeNull();
    });

    it("selectActiveDraft returns the active draft", () => {
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
      expect(selectActiveDraft(useChatDraftStore.getState())?.draftId).toBe(draft.draftId);
    });

    it("selectDraftForWorkspace finds a promoted draft by workspace id", () => {
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      useChatDraftStore.getState().markPromoted(draft.draftId, {
        workspaceId: "ws-42",
        paneId: "pane-42",
        threadId: draft.threadId,
      });
      expect(
        selectDraftForWorkspace(useChatDraftStore.getState(), "ws-42")?.draftId,
      ).toBe(draft.draftId);
    });
  });

  describe("persistence", () => {
    it("persists state to localStorage and hydrates a second store with the same payload", async () => {
      const draft = useChatDraftStore.getState().getOrCreateProjectDraft("/persist");
      useChatDraftStore.getState().updateDraftInput(draft.draftId, "remembered");
      useChatDraftStore.getState().setActiveDraft(draft.draftId);

      // The persist middleware debounces writes by 300ms — wait for the
      // timeout to fire before inspecting localStorage.
      await new Promise((r) => setTimeout(r, 350));

      const raw = window.localStorage.getItem("codemux:chat-drafts:v1");
      expect(raw).toBeTruthy();
      const parsed = JSON.parse(raw!);
      expect(parsed.state.draftsById[draft.draftId].inputDraft).toBe("remembered");
      expect(parsed.state.projectDraftIdByPath["/persist"]).toBe(draft.draftId);
      expect(parsed.state.activeDraftId).toBe(draft.draftId);
      expect(parsed.version).toBe(2);
    });

    it("wipes draft state on v1→v2 migration (context-window default flip)", async () => {
      // Simulate a v1 persistence payload where a draft was seeded with
      // the pre-flip `contextWindow: "200k"` default. Writing this
      // directly to localStorage mimics what a user who launched a
      // prior build would have on disk.
      const stalePayload = {
        state: {
          draftsById: {
            "legacy-draft": {
              draftId: "legacy-draft",
              createdAt: new Date().toISOString(),
              target: { kind: "home" },
              provider: "claude",
              model: "claude-opus-4-7",
              effort: "xhigh",
              contextWindow: "200k",
              permissionMode: "bypassPermissions",
              inputDraft: "",
              threadId: "t-legacy",
              promotedTo: null,
              promoting: false,
              lastSendError: null,
            },
          },
          activeHomeDraftId: "legacy-draft",
          projectDraftIdByPath: {},
          activeDraftId: "legacy-draft",
        },
        version: 1,
      };
      window.localStorage.setItem(
        "codemux:chat-drafts:v1",
        JSON.stringify(stalePayload),
      );

      // Rehydrate from storage — the persist middleware reads the stale
      // payload, sees version < 2, and applies the migration. We test
      // `rehydrate()` (the public persist API) rather than re-importing
      // the module because Vitest caches modules across tests.
      await useChatDraftStore.persist.rehydrate();

      const state = useChatDraftStore.getState();
      expect(state.draftsById).toEqual({});
      expect(state.activeHomeDraftId).toBeNull();
      expect(state.activeDraftId).toBeNull();
    });
  });
});
