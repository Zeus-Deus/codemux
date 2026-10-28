import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { act, renderHook } from "@testing-library/react";

import {
  chatDraftHasUserContent,
  partializeChatDraftStoreState,
  reseedAutomaticProviderDrafts,
  reseedBootstrapDrafts,
  selectActiveDraft,
  selectDraftForWorkspace,
  useChatDraftStore,
  useReseedBootstrapDrafts,
  type ChatDraft,
  type DraftId,
} from "./chat-draft-store";
import { useAgentChatStore } from "./agent-chat-store";
import { useSettingsStore } from "./settings-store";
import { useAppStore } from "./app-store";
import { createSeedAppState } from "@/dev/mock-fixtures";
import { useProviderCapabilities } from "./provider-capabilities-store";
import { emptyHealthSlot, useProviderHealth } from "./provider-health-store";
import type {
  AgentChatProviderKind,
  ChatModelInfo,
  ProviderChatCapabilities,
  ProviderHealthReport,
} from "@/tauri/types";

function claudeCaps(
  models: Array<Pick<ChatModelInfo, "id" | "label"> & Partial<ChatModelInfo>>,
): ProviderChatCapabilities {
  return {
    models: models.map((m) => ({
      description: null,
      effort_levels: [],
      default_effort: null,
      prompt_injected_effort_levels: [],
      context_window_options: [],
      supports_adaptive_thinking: false,
      supports_thinking_toggle: false,
      supports_fast_mode: false,
      supports_images: false,
      sub_provider: null,
      is_free: false,
      ...m,
    })),
    effort_granularity: "per_session",
    effort_label_map: {},
    permission_modes: [],
    default_permission_mode: "bypassPermissions",
    permission_granularity: "per_session",
  };
}

function resetStore() {
  useChatDraftStore.setState({
    draftsById: {},
    activeHomeDraftId: null,
    projectDraftIdByPath: {},
    activeDraftId: null,
  });
  useAgentChatStore.setState({ threads: {} });
  useProviderCapabilities.setState({ claude: null, codex: null, cursor: null, grok: null, opencode: null });
  useAppStore.setState({ appState: null });
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
      expect(draft.checkoutMode).toBe("worktree");
      expect(draft.permissionMode).toBe("bypassPermissions");
      // Default model is seeded so capability-dependent pickers
      // (Effort, ContextWindow) render on first paint. In the test
      // environment the `provider-capabilities-store` is not
      // hydrated, so `defaultModelId` returns the CLI's `default`
      // alias, and `capabilityDefaults` returns null
      // effort / contextWindow because the model payload isn't
      // available yet. The Stage C Effort-lock fix means these null
      // values flow through materialize into the slice; real runtime
      // gets non-null values once caps hydrate.
      expect(draft.model).toBe("default");
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

    it("preserves an invested home draft and mints a fresh one", () => {
      const first = useChatDraftStore
        .getState()
        .getOrCreateHomeDraft({ lockedToHome: true });
      useChatDraftStore
        .getState()
        .updateDraftInput(first.draftId, "unfinished investigation");

      const second = useChatDraftStore
        .getState()
        .getOrCreateHomeDraft({ lockedToHome: true });
      const state = useChatDraftStore.getState();

      expect(second.draftId).not.toBe(first.draftId);
      expect(state.activeHomeDraftId).toBe(second.draftId);
      expect(state.draftsById[first.draftId]?.inputDraft).toBe(
        "unfinished investigation",
      );
      expect(state.draftsById[second.draftId]).toBeDefined();
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
      expect(project.checkoutMode).toBe("worktree");
    });

    it("preserves an invested project draft and remaps the project slot", () => {
      const first = useChatDraftStore
        .getState()
        .getOrCreateProjectDraft("/repo");
      useChatDraftStore
        .getState()
        .updateDraftInput(first.draftId, "first idea");

      const second = useChatDraftStore
        .getState()
        .getOrCreateProjectDraft("/repo");
      const state = useChatDraftStore.getState();

      expect(second.draftId).not.toBe(first.draftId);
      expect(state.projectDraftIdByPath["/repo"]).toBe(second.draftId);
      expect(state.draftsById[first.draftId]?.inputDraft).toBe("first idea");
    });

    it("treats staged attachments as invested content", () => {
      const first = useChatDraftStore
        .getState()
        .getOrCreateProjectDraft("/repo");
      const chat = useAgentChatStore.getState();
      chat.ensureThread(first.threadId);
      chat.addStagedAttachment(first.threadId, {
        id: "attachment-1",
        kind: "file",
        ref: "/repo/README.md",
        metadata: { label: "README.md" },
      });

      const second = useChatDraftStore
        .getState()
        .getOrCreateProjectDraft("/repo");

      expect(second.draftId).not.toBe(first.draftId);
      expect(useChatDraftStore.getState().draftsById[first.draftId]).toBeDefined();
    });
  });

  describe("content detection", () => {
    it("counts text or attachments but not ambient configuration", () => {
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      expect(chatDraftHasUserContent(draft)).toBe(false);
      expect(chatDraftHasUserContent({ ...draft, inputDraft: "  hello  " })).toBe(true);
      expect(chatDraftHasUserContent(draft, 1)).toBe(true);
      expect(chatDraftHasUserContent({ ...draft, effort: "high" })).toBe(false);
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

    it("defaults new drafts to this device and stores a picked device", () => {
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      expect(draft.hostId).toBeNull();
      useChatDraftStore
        .getState()
        .updateDraftConfig(draft.draftId, { hostId: 7, lockedToHome: true });
      const next = useChatDraftStore.getState().draftsById[draft.draftId];
      expect(next.hostId).toBe(7);
      expect(next.lockedToHome).toBe(true);
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

    it("discardDraft also releases the pre-minted thread slice", () => {
      const draft = useChatDraftStore
        .getState()
        .getOrCreateProjectDraft("/p");
      const chat = useAgentChatStore.getState();
      chat.ensureThread(draft.threadId);
      chat.addStagedAttachment(draft.threadId, {
        id: "attachment-1",
        kind: "file",
        ref: "/p/README.md",
        metadata: { label: "README.md" },
      });
      useChatDraftStore.getState().setActiveDraft(draft.draftId);

      useChatDraftStore.getState().discardDraft(draft.draftId);

      expect(useChatDraftStore.getState().draftsById[draft.draftId]).toBeUndefined();
      expect(useChatDraftStore.getState().activeDraftId).toBeNull();
      expect(useAgentChatStore.getState().threads[draft.threadId]).toBeUndefined();
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

    it("keeps a promoted draft's live thread when its project is cleared", () => {
      const draft = useChatDraftStore.getState().getOrCreateProjectDraft("/p");
      const chat = useAgentChatStore.getState();
      chat.ensureThread(draft.threadId);
      useChatDraftStore.getState().markPromoted(draft.draftId, {
        workspaceId: "ws-1",
        paneId: "pane-1",
        threadId: draft.threadId,
      });

      // Archiving the project group inside the post-send grace window must not
      // reset the real pane, which inherited the pre-minted thread id.
      useChatDraftStore.getState().clearDraftsForProject("/p", null);

      expect(useAgentChatStore.getState().threads[draft.threadId]).toBeDefined();
      expect(
        useChatDraftStore.getState().draftsById[draft.draftId],
      ).toBeUndefined();
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

  describe("placeholder model reseed", () => {
    let unsubscribe: () => void = () => {};
    beforeEach(() => {
      unsubscribe = useProviderCapabilities.subscribe(reseedBootstrapDrafts);
    });
    afterEach(() => unsubscribe());

    const roster = claudeCaps([
      {
        id: "opus",
        label: "Claude Opus 5.5",
        effort_levels: ["low", "high"],
        default_effort: "high",
        context_window_options: [
          { value: "1m", label: "1M", is_default: true, context_window_tokens: 1_000_000 },
        ],
      },
      { id: "claude-opus-4-8", label: "Opus 4.8" },
    ]);

    it("moves a pre-hydration draft onto the roster default once caps load", () => {
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      expect(draft.model).toBe("default");

      useProviderCapabilities.setState({ claude: roster });

      const after = useChatDraftStore.getState().draftsById[draft.draftId];
      expect(after.model).toBe("opus");
      expect(after.effort).toBe("high");
      expect(after.contextWindow).toBe("1m");
    });

    it("keeps a supported effort saved before capabilities load", () => {
      useSettingsStore.setState({
        settings: {
          ...useSettingsStore.getState().settings,
          "chat.default_provider": "claude",
          "chat.default_model": "",
          "chat.default_effort": "low",
        },
      });
      try {
        const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
        expect(draft.effort).toBe("low");

        useProviderCapabilities.setState({ claude: roster });

        const after = useChatDraftStore.getState().draftsById[draft.draftId];
        expect(after.model).toBe("opus");
        expect(after.effort).toBe("low");
        expect(after.contextWindow).toBe("1m");
      } finally {
        useSettingsStore.setState({
          settings: {
            ...useSettingsStore.getState().settings,
            "chat.default_provider": "",
            "chat.default_model": "",
            "chat.default_effort": "",
          },
        });
      }
    });

    it("reseeds against a roster that was already restored from storage", () => {
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      unsubscribe();
      useProviderCapabilities.setState({
        claude: claudeCaps([{ id: "opus", label: "Claude Opus 5.5" }]),
      });

      reseedBootstrapDrafts();

      expect(
        useChatDraftStore.getState().draftsById[draft.draftId].model,
      ).toBe("opus");
    });

    it("preserves an explicit concrete choice absent from the refreshed roster", () => {
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      useChatDraftStore
        .getState()
        .updateDraftConfig(draft.draftId, {
          model: "claude-opus-4-8", effort: "low", contextWindow: "200k",
        });

      useProviderCapabilities.setState({
        claude: claudeCaps([{ id: "opus", label: "Claude Opus 5.5" }]),
      });

      expect(useChatDraftStore.getState().draftsById[draft.draftId]).toMatchObject({
        model: "claude-opus-4-8", effort: "low", contextWindow: "200k",
        bootstrapModel: false,
      });
    });

    it("preserves a concrete model chosen in new-chat settings before hydration", () => {
      useSettingsStore.setState({ settings: {
        "chat.default_provider": "claude", "chat.default_model": "claude-opus-4-8",
      } });
      try {
        const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
        useProviderCapabilities.setState({
          claude: claudeCaps([{ id: "opus", label: "Claude Opus 5.5" }]),
        });
        expect(useChatDraftStore.getState().draftsById[draft.draftId]).toMatchObject({
          model: "claude-opus-4-8", bootstrapModel: false,
        });
      } finally {
        useSettingsStore.setState({ settings: {} });
      }
    });

    it("hydrates an automatic provider switch but preserves an explicit model pick", () => {
      const automatic = useChatDraftStore.getState().getOrCreateHomeDraft();
      const explicit = useChatDraftStore.getState().getOrCreateProjectDraft("/explicit");
      useChatDraftStore.getState().updateDraftConfig(automatic.draftId, { provider: "codex" });
      useChatDraftStore.getState().updateDraftConfig(explicit.draftId, {
        provider: "codex", model: "gpt-5.4",
      });
      expect(useChatDraftStore.getState().draftsById[automatic.draftId]).toMatchObject({
        model: "gpt-5.4", bootstrapModel: true,
      });
      useProviderCapabilities.setState({ codex: claudeCaps([
        { id: "gpt-5.5", label: "GPT-5.5", default_effort: "high" },
      ]) });
      const { draftsById } = useChatDraftStore.getState();
      expect(draftsById[automatic.draftId]).toMatchObject({
        model: "gpt-5.5", effort: "high", bootstrapModel: false,
      });
      expect(draftsById[explicit.draftId]).toMatchObject({ model: "gpt-5.4", bootstrapModel: false });
    });

    it("finishes hydration when the automatic seed is already in the roster", () => {
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      useProviderCapabilities.setState({ claude: claudeCaps([
        { id: "default", label: "Default", default_effort: "high" },
      ]) });
      expect(useChatDraftStore.getState().draftsById[draft.draftId]).toMatchObject({
        model: "default", effort: "high", bootstrapModel: false,
      });
      useProviderCapabilities.setState({ claude: roster });
      expect(useChatDraftStore.getState().draftsById[draft.draftId].model).toBe("default");
    });

    it("preserves provider-native defaults for a draft with an explicit device", () => {
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      useChatDraftStore.getState().updateDraftConfig(draft.draftId, { hostId: 7 });
      useProviderCapabilities.setState({ claude: roster });
      expect(useChatDraftStore.getState().draftsById[draft.draftId]).toMatchObject({
        model: "default", effort: null, contextWindow: null, bootstrapModel: true,
      });
    });

    it.each([7, null])("resolves existing-workspace placement from its host %s", (hostId) => {
      const appState = createSeedAppState();
      const workspace = { ...appState.workspaces[0], host_id: hostId };
      appState.workspaces = [workspace];
      useAppStore.setState({ appState });
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      useChatDraftStore.getState().updateDraftConfig(draft.draftId, { hostId: hostId === null ? 7 : null });
      useChatDraftStore.getState().updateDraftTarget(draft.draftId, {
        kind: "existing_workspace", workspaceId: workspace.workspace_id,
      });
      useProviderCapabilities.setState({ claude: roster });
      expect(useChatDraftStore.getState().draftsById[draft.draftId]).toMatchObject(hostId === null
        ? { model: "opus", effort: "high", contextWindow: "1m", bootstrapModel: false }
        : { model: "default", effort: null, contextWindow: null, bootstrapModel: true });
    });

    it("waits for an existing workspace to confirm local placement before hydration", () => {
      const appState = createSeedAppState();
      const workspace = { ...appState.workspaces[0], host_id: null };
      appState.workspaces = [workspace];
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      useChatDraftStore.getState().updateDraftTarget(draft.draftId, {
        kind: "existing_workspace", workspaceId: workspace.workspace_id,
      });
      const { unmount } = renderHook(useReseedBootstrapDrafts);
      try {
        act(() => useProviderCapabilities.setState({ claude: roster }));
        expect(useChatDraftStore.getState().draftsById[draft.draftId].model).toBe("default");
        act(() => useAppStore.setState({ appState }));
        expect(useChatDraftStore.getState().draftsById[draft.draftId].model).toBe("opus");
      } finally {
        unmount();
      }
    });

    it("keeps a model the roster lists, even a former placeholder id", () => {
      useProviderCapabilities.setState({ claude: roster });
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      useChatDraftStore.getState().updateDraftConfig(draft.draftId, {
        model: "claude-opus-4-8",
        effort: "low",
      });

      useProviderCapabilities.setState({ claude: { ...roster } });

      const after = useChatDraftStore.getState().draftsById[draft.draftId];
      expect(after.model).toBe("claude-opus-4-8");
      expect(after.effort).toBe("low");
    });

    it("leaves drafts that are already being sent alone", () => {
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      useChatDraftStore.getState().markPromoting(draft.draftId);

      useProviderCapabilities.setState({ claude: roster });

      expect(
        useChatDraftStore.getState().draftsById[draft.draftId].model,
      ).toBe("default");
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
    it.each([false, true])("preserves model provenance across restore (explicit %s)", async (explicit) => {
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      if (explicit) {
        useChatDraftStore.getState().updateDraftConfig(draft.draftId, { model: "claude-opus-4-8" });
      }
      useChatDraftStore.getState().updateDraftInput(draft.draftId, "unfinished draft");
      await new Promise((resolve) => setTimeout(resolve, 350));
      const raw = window.localStorage.getItem("codemux:chat-drafts:v1");
      expect(raw).toBeTruthy();
      useChatDraftStore.setState({ draftsById: {}, activeHomeDraftId: null });
      useChatDraftStore.persist.clearStorage();
      window.localStorage.setItem("codemux:chat-drafts:v1", raw!);
      await useChatDraftStore.persist.rehydrate();
      expect(useChatDraftStore.getState().draftsById[draft.draftId].bootstrapModel).toBe(!explicit);
      useProviderCapabilities.setState({ claude: claudeCaps([{ id: "opus", label: "Opus 5.5" }]) });
      reseedBootstrapDrafts();
      expect(useChatDraftStore.getState().draftsById[draft.draftId]).toMatchObject({
        model: explicit ? "claude-opus-4-8" : "opus",
        inputDraft: "unfinished draft",
        bootstrapModel: false,
      });
    });

    it.each([2, 3])("preserves an unmarked legacy model restored from storage v%s", async (version) => {
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      const legacy = { ...draft, model: "claude-opus-4-8", inputDraft: "saved choice" };
      delete legacy.bootstrapModel;
      const payload = {
        version,
        state: { ...partializeChatDraftStoreState(useChatDraftStore.getState()),
          draftsById: { [draft.draftId]: legacy } },
      };
      useChatDraftStore.persist.clearStorage();
      window.localStorage.setItem("codemux:chat-drafts:v1", JSON.stringify(payload));
      await useChatDraftStore.persist.rehydrate();
      useProviderCapabilities.setState({ claude: claudeCaps([{ id: "opus", label: "Opus 5.5" }]) });
      reseedBootstrapDrafts();
      expect(useChatDraftStore.getState().draftsById[draft.draftId]).toMatchObject({
        model: "claude-opus-4-8", inputDraft: "saved choice",
      });
      expect(useChatDraftStore.getState().draftsById[draft.draftId].bootstrapModel).toBeUndefined();
    });

    it("keeps invested unmapped drafts and drops empty unmapped sessions", () => {
      const first = useChatDraftStore
        .getState()
        .getOrCreateProjectDraft("/persist");
      useChatDraftStore
        .getState()
        .updateDraftInput(first.draftId, "keep me");
      const mapped = useChatDraftStore
        .getState()
        .getOrCreateProjectDraft("/persist");
      const emptyId = "empty-unmapped" as DraftId;
      useChatDraftStore.setState((state) => ({
        draftsById: {
          ...state.draftsById,
          [emptyId]: {
            ...mapped,
            draftId: emptyId,
            threadId: "thread-empty-unmapped",
          },
        },
      }));

      const persisted = partializeChatDraftStoreState(
        useChatDraftStore.getState(),
      );

      expect(persisted.draftsById[first.draftId]).toBeDefined();
      expect(persisted.draftsById[mapped.draftId]).toBeDefined();
      expect(persisted.draftsById[emptyId]).toBeUndefined();
    });

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
      expect(parsed.version).toBe(3);
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

    it("rehydrates a v2 draft saved before devices existed without a hostId", async () => {
      const payload = {
        state: {
          draftsById: {
            "pre-device": {
              draftId: "pre-device",
              createdAt: new Date().toISOString(),
              target: { kind: "home" },
              provider: "claude",
              model: "claude-opus-4-8",
              effort: null,
              contextWindow: null,
              permissionMode: "bypassPermissions",
              inputDraft: "kept",
              threadId: "t-pre-device",
              promotedTo: null,
              materializedTo: null,
              promoting: false,
              lastSendError: null,
            },
          },
          activeHomeDraftId: "pre-device",
          projectDraftIdByPath: {},
          activeDraftId: "pre-device",
        },
        version: 2,
      };
      // Drop the debounced write `resetStore` queued, which the next read
      // would otherwise flush over the payload below.
      useChatDraftStore.persist.clearStorage();
      window.localStorage.setItem(
        "codemux:chat-drafts:v1",
        JSON.stringify(payload),
      );

      await useChatDraftStore.persist.rehydrate();

      const draft = useChatDraftStore.getState().draftsById["pre-device" as DraftId];
      expect(draft.inputDraft).toBe("kept");
      expect(draft.hostId ?? null).toBeNull();
    });
  });

  describe("new-chat defaults", () => {
    afterEach(() => {
      useSettingsStore.setState({ settings: {} });
    });

    it("starts drafts in the configured checkout mode", () => {
      useSettingsStore.setState({
        settings: { "chat.default_checkout": "current" },
      });
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      expect(draft.checkoutMode).toBe("current");
    });

    it("re-seeds untouched drafts and leaves ones with text alone", () => {
      const store = useChatDraftStore.getState();
      const empty = store.getOrCreateHomeDraft();
      const typed = store.getOrCreateProjectDraft("/repo");
      store.updateDraftInput(typed.draftId, "keep me");

      useSettingsStore.setState({
        settings: {
          "chat.default_provider": "codex",
          "chat.default_model": "gpt-5.4",
          "chat.default_checkout": "current",
        },
      });
      useChatDraftStore.getState().applyNewChatDefaults();

      const { draftsById } = useChatDraftStore.getState();
      expect(draftsById[empty.draftId]).toMatchObject({
        provider: "codex",
        model: "gpt-5.4",
        permissionMode: "danger-full-access",
        checkoutMode: "current",
        threadId: empty.threadId,
      });
      expect(draftsById[typed.draftId]).toMatchObject({
        provider: "claude",
        checkoutMode: "worktree",
      });
    });

    it("moves an empty draft off a provider that was turned off, and only those", () => {
      useSettingsStore.setState({ settings: {} });
      const store = useChatDraftStore.getState();
      const empty = store.getOrCreateHomeDraft();
      expect(empty.provider).toBe("claude");
      // Pretend another untouched draft was manually put on Codex.
      const other = store.getOrCreateProjectDraft("/repo");
      useChatDraftStore.setState((s) => ({
        draftsById: { ...s.draftsById, [other.draftId]: { ...s.draftsById[other.draftId], provider: "codex" } },
      }));

      useSettingsStore.setState({ settings: { "providers.disabled": "claude" } });
      useChatDraftStore.getState().applyNewChatDefaults(new Set(["claude"]));

      const { draftsById } = useChatDraftStore.getState();
      expect(draftsById[empty.draftId].provider).not.toBe("claude");
      // Reusing the home draft (New agent) now starts on an enabled provider.
      expect(useChatDraftStore.getState().getOrCreateHomeDraft().provider).not.toBe("claude");
      // Untouched drafts on other providers keep their pick.
      expect(draftsById[other.draftId].provider).toBe("codex");
    });

    describe("following the health probes", () => {
      function settle(ready: AgentChatProviderKind[], inFlight: AgentChatProviderKind[] = []) {
        const slots = { ...useProviderHealth.getState().slots };
        for (const provider of ["claude", "codex", "opencode", "cursor", "grok"] as const) {
          const report: ProviderHealthReport = {
            provider,
            status: ready.includes(provider) ? "ready" : "error",
            installed: ready.includes(provider),
            message: null,
            version: null,
          };
          slots[provider] = {
            ...emptyHealthSlot(),
            report,
            fetchedAt: 1,
            inFlight: inFlight.includes(provider) ? new Promise(() => {}) : null,
          };
        }
        useProviderHealth.setState({ slots });
        reseedAutomaticProviderDrafts();
      }

      afterEach(() => {
        const slots = { ...useProviderHealth.getState().slots };
        for (const provider of Object.keys(slots) as AgentChatProviderKind[]) {
          slots[provider] = emptyHealthSlot();
        }
        useProviderHealth.setState({ slots });
      });

      it("moves a first-run draft to the first ready agent once every probe settles", () => {
        const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
        expect(draft).toMatchObject({ provider: "claude", automaticProvider: true });
        useChatDraftStore.getState().updateDraftInput(draft.draftId, "typed early");

        settle(["opencode"], ["codex"]);
        expect(useChatDraftStore.getState().draftsById[draft.draftId].provider).toBe("claude");

        settle(["opencode"]);
        expect(useChatDraftStore.getState().draftsById[draft.draftId]).toMatchObject({
          provider: "opencode",
          automaticProvider: true,
          inputDraft: "typed early",
          threadId: draft.threadId,
        });
      });

      it("does not move a draft back to the fallback while nothing is known to run", () => {
        const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
        settle(["opencode"]);
        expect(useChatDraftStore.getState().draftsById[draft.draftId].provider).toBe("opencode");
        // An app restart: the cache is empty again.
        useProviderHealth.setState((state) => {
          const slots = { ...state.slots };
          for (const provider of Object.keys(slots) as AgentChatProviderKind[]) {
            slots[provider] = emptyHealthSlot();
          }
          return { slots };
        });
        reseedAutomaticProviderDrafts();
        expect(useChatDraftStore.getState().draftsById[draft.draftId].provider).toBe("opencode");
        settle([]);
        expect(useChatDraftStore.getState().draftsById[draft.draftId].provider).toBe("opencode");
      });

      it("leaves drafts the user picked a provider or model on", () => {
        const store = useChatDraftStore.getState();
        const picked = store.getOrCreateHomeDraft();
        store.updateDraftConfig(picked.draftId, { provider: "claude" });
        const model = store.getOrCreateProjectDraft("/repo");
        store.updateDraftConfig(model.draftId, { model: "default" });

        settle(["codex"]);
        const { draftsById } = useChatDraftStore.getState();
        expect(draftsById[picked.draftId]).toMatchObject({ provider: "claude", automaticProvider: false });
        expect(draftsById[model.draftId]).toMatchObject({ provider: "claude", automaticProvider: false });
      });

      it("leaves drafts alone while a default is saved, and device drafts always", () => {
        const device = useChatDraftStore.getState().getOrCreateProjectDraft("/repo");
        useChatDraftStore.getState().updateDraftConfig(device.draftId, { hostId: 2 });
        useSettingsStore.setState({ settings: { "chat.default_provider": "claude" } });
        const saved = useChatDraftStore.getState().getOrCreateHomeDraft();
        expect(saved.automaticProvider).toBe(false);
        settle(["codex"]);
        expect(useChatDraftStore.getState().draftsById[saved.draftId].provider).toBe("claude");

        useSettingsStore.setState({ settings: {} });
        settle(["codex"]);
        expect(useChatDraftStore.getState().draftsById[device.draftId].provider).toBe("claude");
      });
    });
  });

  it("migrates legacy Fast drafts without losing their text or identity", async () => {
    const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
    useChatDraftStore.persist.clearStorage();
    window.localStorage.setItem("codemux:chat-drafts:v1", JSON.stringify({
      version: 2,
      state: {
        draftsById: { [draft.draftId]: { ...draft, inputDraft: "Keep this draft", fastMode: true, serviceTier: undefined } },
        activeHomeDraftId: draft.draftId,
        activeDraftId: draft.draftId,
        projectDraftIdByPath: {},
      },
    }));
    await useChatDraftStore.persist.rehydrate();
    const restored = useChatDraftStore.getState().draftsById[draft.draftId];
    expect(restored.serviceTier).toBe("fast");
    expect(restored.inputDraft).toBe("Keep this draft");
    expect(restored.threadId).toBe(draft.threadId);
    expect(restored).not.toHaveProperty("fastMode");
  });

});
