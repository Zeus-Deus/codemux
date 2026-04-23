import { create } from "zustand";
import { createJSONStorage, persist, type StateStorage } from "zustand/middleware";
import {
  capabilityDefaults,
  defaultModelId,
} from "@/lib/agent-chat/capability-defaults";
import type { AgentChatProviderKind } from "@/tauri/types";

/** Branded draft identifier. Assigned via `crypto.randomUUID()`. */
export type DraftId = string & { readonly __brand: "DraftId" };

/** Where a draft materialises on first send. */
export type DraftTarget =
  | { kind: "home" }
  | { kind: "project"; projectPath: string }
  | { kind: "existing_workspace"; workspaceId: string };

/** A client-side chat session that has not yet materialised a workspace. */
export interface ChatDraft {
  draftId: DraftId;
  createdAt: string;
  target: DraftTarget;
  provider: AgentChatProviderKind;
  model: string | null;
  effort: string | null;
  contextWindow: string | null;
  permissionMode: string | null;
  inputDraft: string;
  /** Pre-minted thread id passed to `agent_chat_start_session` on
   *  materialise. Sharing the id with the real session means the
   *  in-memory transcript slice (keyed by thread id) does not need to
   *  migrate when the draft promotes, so there is no visual flicker. */
  threadId: string;
  /** Set once materialise-and-send succeeds. The entry lingers in
   *  `draftsById` for a short grace period so any in-flight UI can
   *  observe the promotion before cleanup. */
  promotedTo: {
    workspaceId: string;
    paneId: string;
    threadId: string;
  } | null;
  /** Non-null while materialise-and-send is in flight. */
  promoting: boolean;
  /** Last materialise-and-send error, surfaced as an inline retry
   *  affordance on the composer. Cleared on next successful send. */
  lastSendError: string | null;
}

export interface ChatDraftStore {
  draftsById: Record<DraftId, ChatDraft>;
  activeHomeDraftId: DraftId | null;
  projectDraftIdByPath: Record<string, DraftId>;
  /** Draft currently rendered in the main pane area, if any. */
  activeDraftId: DraftId | null;

  // Slot lookup / creation
  getOrCreateHomeDraft: () => ChatDraft;
  getOrCreateProjectDraft: (projectPath: string) => ChatDraft;

  // Active-draft selection
  setActiveDraft: (draftId: DraftId | null) => void;

  // Mutations
  updateDraftTarget: (draftId: DraftId, target: DraftTarget) => void;
  updateDraftConfig: (
    draftId: DraftId,
    config: Partial<
      Pick<
        ChatDraft,
        "provider" | "model" | "effort" | "contextWindow" | "permissionMode"
      >
    >,
  ) => void;
  updateDraftInput: (draftId: DraftId, input: string) => void;

  // Promotion lifecycle
  markPromoting: (draftId: DraftId) => void;
  markPromoted: (
    draftId: DraftId,
    promotedTo: NonNullable<ChatDraft["promotedTo"]>,
  ) => void;
  markSendFailed: (draftId: DraftId, error: string) => void;
  clearSendError: (draftId: DraftId) => void;

  // Removal
  clearDraft: (draftId: DraftId) => void;
  /** When `projectPath === homeDir`, also sweeps home-target
   *  drafts so closing the sidebar's Home group clears the draft
   *  that would otherwise respawn on next "+" click. Pass `null`
   *  for `homeDir` to skip the home-draft sweep (e.g. in tests or
   *  early-boot before the cache hydrates). */
  clearDraftsForProject: (projectPath: string, homeDir: string | null) => void;
}

const STORAGE_KEY = "codemux:chat-drafts:v1";
// v2 — Context-window default flip: drafts created under v1 may carry
// the pre-flip `contextWindow: "200k"` seed. On v1→v2 we drop the
// persisted draft set so the next `getOrCreate*Draft` call re-seeds
// from the current `capabilityDefaults` (1M on multi-option models).
const STORAGE_VERSION = 2;

function newDraftId(): DraftId {
  return crypto.randomUUID() as DraftId;
}

function newThreadId(): string {
  return crypto.randomUUID();
}

function makeDraft(target: DraftTarget): ChatDraft {
  const provider: AgentChatProviderKind = "claude";
  // Fully-configure the draft from the capabilities store: default
  // model + its `default_effort` + its default context-window option
  // + the provider's default permission mode. Drafts entering the
  // composer with non-null effort/contextWindow means the Effort and
  // ContextWindow pickers render from minute zero — and the slice
  // materialize seeds inherits the same values, so pickers stay
  // visible post-first-send too (Stage C Effort-lock fix).
  const modelId = defaultModelId(provider);
  const defaults = capabilityDefaults(provider, modelId);
  return {
    draftId: newDraftId(),
    createdAt: new Date().toISOString(),
    target,
    provider,
    model: defaults.model,
    effort: defaults.effort,
    contextWindow: defaults.contextWindow,
    permissionMode: defaults.permissionMode,
    inputDraft: "",
    threadId: newThreadId(),
    promotedTo: null,
    promoting: false,
    lastSendError: null,
  };
}

const PERSIST_DEBOUNCE_MS = 300;

/** Debounced storage wrapper used by the persist middleware.
 *  Debouncing keeps every keystroke from hitting localStorage on Tauri
 *  webviews where sync I/O briefly blocks the render thread. Reads are
 *  pass-through — only writes are coalesced. */
function createDebouncedLocalStorage(): StateStorage {
  if (typeof window === "undefined") {
    // SSR / test harness fallback — straight through to an in-memory map.
    const mem = new Map<string, string>();
    return {
      getItem: (name) => mem.get(name) ?? null,
      setItem: (name, value) => {
        mem.set(name, value);
      },
      removeItem: (name) => {
        mem.delete(name);
      },
    };
  }

  let pending: { name: string; value: string } | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;

  const flush = () => {
    if (!pending) return;
    try {
      window.localStorage.setItem(pending.name, pending.value);
    } catch (err) {
      console.error("[chat-draft-store] localStorage write failed:", err);
    }
    pending = null;
    if (timer) {
      clearTimeout(timer);
      timer = null;
    }
  };

  if (typeof window.addEventListener === "function") {
    window.addEventListener("beforeunload", flush);
  }

  return {
    getItem: (name) => {
      // Flush any pending write so a same-tick read reflects it.
      flush();
      try {
        return window.localStorage.getItem(name);
      } catch {
        return null;
      }
    },
    setItem: (name, value) => {
      pending = { name, value };
      if (timer) clearTimeout(timer);
      timer = setTimeout(flush, PERSIST_DEBOUNCE_MS);
    },
    removeItem: (name) => {
      pending = null;
      if (timer) {
        clearTimeout(timer);
        timer = null;
      }
      try {
        window.localStorage.removeItem(name);
      } catch {
        // Ignore — nothing to undo.
      }
    },
  };
}

/** Singleton debounced storage. Reusing one instance preserves the
 *  pending-write state across calls made by the persist middleware. */
const debouncedStorage = createDebouncedLocalStorage();

export const useChatDraftStore = create<ChatDraftStore>()(
  persist(
    (set, get) => ({
      draftsById: {},
      activeHomeDraftId: null,
      projectDraftIdByPath: {},
      activeDraftId: null,

      getOrCreateHomeDraft: () => {
        const state = get();
        const existingId = state.activeHomeDraftId;
        if (existingId) {
          const existing = state.draftsById[existingId];
          if (existing && existing.promotedTo === null) return existing;
        }
        const draft = makeDraft({ kind: "home" });
        set({
          draftsById: { ...state.draftsById, [draft.draftId]: draft },
          activeHomeDraftId: draft.draftId,
        });
        return draft;
      },

      getOrCreateProjectDraft: (projectPath) => {
        const state = get();
        const existingId = state.projectDraftIdByPath[projectPath];
        if (existingId) {
          const existing = state.draftsById[existingId];
          if (existing && existing.promotedTo === null) return existing;
        }
        const draft = makeDraft({ kind: "project", projectPath });
        set({
          draftsById: { ...state.draftsById, [draft.draftId]: draft },
          projectDraftIdByPath: {
            ...state.projectDraftIdByPath,
            [projectPath]: draft.draftId,
          },
        });
        return draft;
      },

      setActiveDraft: (draftId) => set({ activeDraftId: draftId }),

      updateDraftTarget: (draftId, target) =>
        set((s) => {
          const existing = s.draftsById[draftId];
          if (!existing) return s;
          const next: ChatDraft = { ...existing, target };
          const patch: Partial<ChatDraftStore> = {
            draftsById: { ...s.draftsById, [draftId]: next },
          };
          // Move the slot pointer when the target kind changes.
          const prevKind = existing.target.kind;
          const nextKind = target.kind;
          if (prevKind !== nextKind || prevKind === "project") {
            const nextProjectMap = { ...s.projectDraftIdByPath };
            // Remove stale project mapping for the old target.
            if (prevKind === "project") {
              const prevPath = (existing.target as { kind: "project"; projectPath: string })
                .projectPath;
              if (nextProjectMap[prevPath] === draftId) {
                delete nextProjectMap[prevPath];
              }
            }
            if (nextKind === "project") {
              nextProjectMap[(target as { kind: "project"; projectPath: string }).projectPath] =
                draftId;
            }
            patch.projectDraftIdByPath = nextProjectMap;

            if (prevKind === "home" && s.activeHomeDraftId === draftId) {
              patch.activeHomeDraftId = null;
            }
            if (nextKind === "home") {
              patch.activeHomeDraftId = draftId;
            }
          }
          return patch as ChatDraftStore;
        }),

      updateDraftConfig: (draftId, config) =>
        set((s) => {
          const existing = s.draftsById[draftId];
          if (!existing) return s;
          return {
            draftsById: {
              ...s.draftsById,
              [draftId]: { ...existing, ...config },
            },
          };
        }),

      updateDraftInput: (draftId, input) =>
        set((s) => {
          const existing = s.draftsById[draftId];
          if (!existing) return s;
          return {
            draftsById: {
              ...s.draftsById,
              [draftId]: { ...existing, inputDraft: input },
            },
          };
        }),

      markPromoting: (draftId) =>
        set((s) => {
          const existing = s.draftsById[draftId];
          if (!existing) return s;
          return {
            draftsById: {
              ...s.draftsById,
              [draftId]: {
                ...existing,
                promoting: true,
                lastSendError: null,
              },
            },
          };
        }),

      markPromoted: (draftId, promotedTo) =>
        set((s) => {
          const existing = s.draftsById[draftId];
          if (!existing) return s;
          const nextDraft: ChatDraft = {
            ...existing,
            promotedTo,
            promoting: false,
            lastSendError: null,
          };
          const nextDraftsById = { ...s.draftsById, [draftId]: nextDraft };

          // Clear from slots so a follow-up click creates a fresh draft.
          const patch: Partial<ChatDraftStore> = { draftsById: nextDraftsById };
          if (s.activeHomeDraftId === draftId) patch.activeHomeDraftId = null;
          if (existing.target.kind === "project") {
            const path = existing.target.projectPath;
            if (s.projectDraftIdByPath[path] === draftId) {
              const nextMap = { ...s.projectDraftIdByPath };
              delete nextMap[path];
              patch.projectDraftIdByPath = nextMap;
            }
          }
          return patch as ChatDraftStore;
        }),

      markSendFailed: (draftId, error) =>
        set((s) => {
          const existing = s.draftsById[draftId];
          if (!existing) return s;
          return {
            draftsById: {
              ...s.draftsById,
              [draftId]: {
                ...existing,
                promoting: false,
                lastSendError: error,
              },
            },
          };
        }),

      clearSendError: (draftId) =>
        set((s) => {
          const existing = s.draftsById[draftId];
          if (!existing || existing.lastSendError === null) return s;
          return {
            draftsById: {
              ...s.draftsById,
              [draftId]: { ...existing, lastSendError: null },
            },
          };
        }),

      clearDraft: (draftId) =>
        set((s) => {
          const existing = s.draftsById[draftId];
          if (!existing) return s;
          const nextDrafts = { ...s.draftsById };
          delete nextDrafts[draftId];

          const patch: Partial<ChatDraftStore> = { draftsById: nextDrafts };
          if (s.activeHomeDraftId === draftId) patch.activeHomeDraftId = null;
          if (s.activeDraftId === draftId) patch.activeDraftId = null;
          if (existing.target.kind === "project") {
            const path = existing.target.projectPath;
            if (s.projectDraftIdByPath[path] === draftId) {
              const nextMap = { ...s.projectDraftIdByPath };
              delete nextMap[path];
              patch.projectDraftIdByPath = nextMap;
            }
          }
          return patch as ChatDraftStore;
        }),

      clearDraftsForProject: (projectPath, homeDir) =>
        set((s) => {
          const sweepHomeDrafts =
            homeDir !== null && projectPath === homeDir;
          const matching = Object.values(s.draftsById).filter(
            (d) =>
              (d.target.kind === "project" &&
                d.target.projectPath === projectPath) ||
              (sweepHomeDrafts && d.target.kind === "home"),
          );
          if (matching.length === 0) return s;
          const nextDrafts = { ...s.draftsById };
          for (const d of matching) delete nextDrafts[d.draftId];
          const nextProjectMap = { ...s.projectDraftIdByPath };
          delete nextProjectMap[projectPath];
          const patch: Partial<ChatDraftStore> = {
            draftsById: nextDrafts,
            projectDraftIdByPath: nextProjectMap,
          };
          if (sweepHomeDrafts) {
            // The single home slot points into draftsById. If its
            // draft was swept above, clear the slot pointer too.
            if (
              s.activeHomeDraftId &&
              matching.some((d) => d.draftId === s.activeHomeDraftId)
            ) {
              patch.activeHomeDraftId = null;
            }
          }
          if (s.activeDraftId && matching.some((d) => d.draftId === s.activeDraftId)) {
            patch.activeDraftId = null;
          }
          return patch as ChatDraftStore;
        }),
    }),
    {
      name: STORAGE_KEY,
      version: STORAGE_VERSION,
      storage: createJSONStorage(() => debouncedStorage),
      migrate: (persistedState, version) => {
        if (version < 2) {
          // Wipe draft state. Drafts are ephemeral — any that existed
          // under v1 were either (a) not yet sent, in which case the
          // user can re-type, or (b) already promoted, in which case
          // the real workspace lives in the backend state and doesn't
          // need this client-side record anymore.
          return {
            ...(persistedState as ChatDraftStore),
            draftsById: {},
            activeHomeDraftId: null,
            projectDraftIdByPath: {},
            activeDraftId: null,
          } as ChatDraftStore;
        }
        return persistedState as ChatDraftStore;
      },
      partialize: (state) => ({
        draftsById: state.draftsById,
        activeHomeDraftId: state.activeHomeDraftId,
        projectDraftIdByPath: state.projectDraftIdByPath,
        activeDraftId: state.activeDraftId,
      }),
    },
  ),
);

/** Selector helpers — colocated so call sites do not reinvent them. */

export function selectActiveDraft(state: ChatDraftStore): ChatDraft | null {
  if (!state.activeDraftId) return null;
  return state.draftsById[state.activeDraftId] ?? null;
}

export function selectDraftForWorkspace(
  state: ChatDraftStore,
  workspaceId: string,
): ChatDraft | null {
  const match = Object.values(state.draftsById).find(
    (d) => d.promotedTo?.workspaceId === workspaceId,
  );
  return match ?? null;
}
