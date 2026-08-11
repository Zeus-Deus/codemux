import { create } from "zustand";
import { createJSONStorage, persist, type StateStorage } from "zustand/middleware";
import {
  capabilityDefaults,
  defaultModelId,
} from "@/lib/agent-chat/capability-defaults";
import { discardStagedImage } from "@/lib/agent-chat/image-staging";
import {
  useAgentChatStore,
  type ChatMode,
} from "@/stores/agent-chat-store";
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
  fastMode?: boolean;
  permissionMode: string | null;
  /** Composer mode pill carried through draft → slice on materialise.
   *  `default` renders no pill. Drafts can pre-select a mode before
   *  the session exists; materialise applies the mode-specific
   *  session config (for `plan` that's `permission_mode: "plan"`). */
  mode: ChatMode;
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
  /** Set once the workspace + pane resources have been created on the
   *  backend, BEFORE the session-start step. Distinct from `promotedTo`,
   *  which only fires on full success. If `start_session` or `send_turn`
   *  fail, this stays set while `promotedTo` stays null — that's the
   *  signal AgentChatPane uses to recover an orphaned partial
   *  materialise: adopt the pre-minted thread id, start the session it
   *  needs, then complete the promotion.
   *
   *  Cleared by `clearDraft`. */
  materializedTo: {
    workspaceId: string;
    paneId: string;
    threadId: string;
  } | null;
  /** Non-null while materialise-and-send is in flight. */
  promoting: boolean;
  /** Last materialise-and-send error, surfaced as an inline retry
   *  affordance on the composer. Cleared on next successful send. */
  lastSendError: string | null;
  /** When `true`, this draft was created by an explicit user action
   *  (clicking the sidebar "New agent" button) and should NOT be
   *  auto-redirected to the active project workspace by the mount-
   *  time seed effect or the submit-time salvage in `DraftChatSurface`.
   *
   *  The auto-redirect exists to help the *implicit* empty-state
   *  path (`useEnsureDraftWhenEmpty`) — it mirrors the sidebar context
   *  so a fresh empty workspace lands on something useful. But for
   *  explicit "New agent" clicks the tooltip promises "New chat in
   *  home directory", so the redirect would lie to the user.
   *
   *  Optional + defaults to `false` so older persisted drafts
   *  deserialize unchanged. */
  lockedToHome?: boolean;
  /** Thread Scope redesign — where the agent should work relative to
   *  the target project. `"current"` (default) sends into the
   *  project's existing checkout; `"worktree"` defers creation of a
   *  fresh git worktree to submit time (`materializeAndSend`), named
   *  either from `worktreeName` (if set) or auto-derived from the
   *  first message / a random fallback. Irrelevant for `target.kind
   *  === "home"` drafts (no project, nothing to branch off of).
   *
   *  Optional + defaults to `"current"` via read-site fallbacks so
   *  older persisted drafts deserialize unchanged. */
  checkoutMode?: "current" | "worktree";
  /** User-typed worktree name for a deferred `"worktree"` checkout.
   *  Empty string (the default) means "auto-name at submit time" —
   *  see `checkoutMode`. Optional, defaults to `""`. */
  worktreeName?: string;
  /** Base branch the Thread Scope branch control is pointed at. Also
   *  doubles as the best-effort display of "the project's current
   *  checked-out branch" while `checkoutMode === "current"` (see
   *  `ThreadScopeRow`) — picking a DIFFERENT branch while on
   *  `"current"` flips `checkoutMode` to `"worktree"` with that pick
   *  as the base, since silently repointing the user's real checkout
   *  would be destructive. Optional, defaults to `""` (picker seeds
   *  it once branches load, mirroring the branch popover's
   *  main/master/first-branch heuristic). */
  baseBranch?: string;
}

export interface ChatDraftStore {
  draftsById: Record<DraftId, ChatDraft>;
  activeHomeDraftId: DraftId | null;
  projectDraftIdByPath: Record<string, DraftId>;
  /** Draft currently rendered in the main pane area, if any. */
  activeDraftId: DraftId | null;

  // Slot lookup / creation
  /** Look up the singleton home draft, or create one. When called
   *  with `{ lockedToHome: true }` (explicit "New agent" click from
   *  the sidebar), any existing pristine home draft is discarded
   *  and a freshly-flagged one returned — reusing a draft created
   *  by the implicit empty-state path would let the auto-redirect
   *  effect have already kicked in. */
  getOrCreateHomeDraft: (opts?: { lockedToHome?: boolean }) => ChatDraft;
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
        | "provider"
        | "model"
        | "effort"
        | "contextWindow"
        | "fastMode"
        | "permissionMode"
        | "mode"
        | "checkoutMode"
        | "worktreeName"
        | "baseBranch"
      >
    >,
  ) => void;
  updateDraftInput: (draftId: DraftId, input: string) => void;

  // Promotion lifecycle
  markPromoting: (draftId: DraftId) => void;
  /** Records the backend resources created by materialize step 2
   *  (workspace + pane). Called BEFORE the session-start step, so that
   *  if start_session or send_turn fail later, AgentChatPane can find
   *  the partial materialise on click-into-workspace and finish it. */
  markMaterialized: (
    draftId: DraftId,
    materializedTo: NonNullable<ChatDraft["materializedTo"]>,
  ) => void;
  markPromoted: (
    draftId: DraftId,
    promotedTo: NonNullable<ChatDraft["promotedTo"]>,
  ) => void;
  markSendFailed: (draftId: DraftId, error: string) => void;
  clearSendError: (draftId: DraftId) => void;

  // Removal
  /** Permanently discard an unsent draft and its in-memory attachment
   *  slice. Unlike `clearDraft` (the post-promotion metadata cleanup),
   *  this also releases staged image files because no real thread will
   *  inherit them. */
  discardDraft: (draftId: DraftId) => void;
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

/** A draft is reusable by `getOrCreate*Draft` only when it has never
 *  been sent (no `promotedTo`), never reached the workspace+pane stage
 *  of a materialise (no `materializedTo`), and isn't currently mid-
 *  flight (`promoting === false`). The combined gate matters because:
 *
 *  - `promotedTo` set → already a real workspace, the user is asking
 *     for a NEW chat.
 *  - `materializedTo` set without `promotedTo` → partial materialise
 *     left an orphan workspace; the user can recover it by clicking
 *     that workspace in the sidebar (AgentChatPane mount-effect handles
 *     the recovery), but `+` should give them a fresh slate.
 *  - `promoting === true` without resolution → a previous send hung or
 *     was interrupted; reusing renders a permanently-greyed composer
 *     because Send is disabled iff `draft.promoting`. Drafts can land
 *     in this state across an app restart since `promoting` is
 *     persisted via the zustand persist middleware.
 */
function isReusableDraft(draft: ChatDraft | undefined): draft is ChatDraft {
  if (!draft) return false;
  return (
    draft.promotedTo === null &&
    draft.materializedTo === null &&
    !draft.promoting
  );
}

/** Whether the user has put meaningful work into a draft. Provider/model,
 *  permission and checkout choices are ambient defaults and deliberately do
 *  not count; text and attachments are the things a user expects to find
 *  again after leaving the composer. */
export function chatDraftHasUserContent(
  draft: ChatDraft | null | undefined,
  attachmentCount = 0,
): boolean {
  return !!draft && (draft.inputDraft.trim().length > 0 || attachmentCount > 0);
}

function liveDraftAttachmentCount(draft: ChatDraft): number {
  return (
    useAgentChatStore.getState().threads[draft.threadId]?.stagedAttachments
      .length ?? 0
  );
}

function draftHasLiveUserContent(draft: ChatDraft | undefined): boolean {
  return !!draft && chatDraftHasUserContent(draft, liveDraftAttachmentCount(draft));
}

/** Release runtime-only state owned by an unsent draft. This must never run
 *  for the delayed post-promotion cleanup: the real thread deliberately
 *  inherits the same pre-minted thread id and slice. */
function discardDraftRuntime(draft: ChatDraft): void {
  const chat = useAgentChatStore.getState();
  const attachments = chat.threads[draft.threadId]?.stagedAttachments ?? [];
  for (const attachment of attachments) {
    if (attachment.stagedImage) {
      discardStagedImage(attachment.stagedImage.path);
    }
  }
  chat.resetThread(draft.threadId);
}

function makeDraft(
  target: DraftTarget,
  opts: { lockedToHome?: boolean } = {},
): ChatDraft {
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
    fastMode: false,
    permissionMode: defaults.permissionMode,
    mode: "default",
    inputDraft: "",
    threadId: newThreadId(),
    promotedTo: null,
    materializedTo: null,
    promoting: false,
    lastSendError: null,
    lockedToHome: opts.lockedToHome ?? false,
    checkoutMode: "current",
    worktreeName: "",
    baseBranch: "",
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

      getOrCreateHomeDraft: (opts = {}) => {
        const state = get();
        const existingId = state.activeHomeDraftId;
        let discardedEmptyDraft: ChatDraft | null = null;
        if (existingId) {
          const existing = state.draftsById[existingId];
          // Reuse only an EMPTY draft whose home-lock semantics still
          // match. Invested drafts stay alive in draftsById so their sidebar
          // row can reopen them; the home slot moves to a fresh session.
          if (
            isReusableDraft(existing) &&
            !draftHasLiveUserContent(existing) &&
            !!existing.lockedToHome === !!opts.lockedToHome
          ) {
            return existing;
          }
          // A lock mismatch can leave behind a pristine implicit home draft.
          // It has no user work to preserve and would never earn a sidebar
          // row, so remove it immediately instead of waiting for persistence
          // compaction.
          if (isReusableDraft(existing) && !draftHasLiveUserContent(existing)) {
            discardedEmptyDraft = existing;
          }
        }
        const draft = makeDraft({ kind: "home" }, opts);
        const nextDraftsById = { ...state.draftsById };
        if (discardedEmptyDraft) {
          delete nextDraftsById[discardedEmptyDraft.draftId];
          discardDraftRuntime(discardedEmptyDraft);
        }
        set({
          draftsById: { ...nextDraftsById, [draft.draftId]: draft },
          activeHomeDraftId: draft.draftId,
          ...(discardedEmptyDraft && state.activeDraftId === discardedEmptyDraft.draftId
            ? { activeDraftId: draft.draftId }
            : {}),
        });
        return draft;
      },

      getOrCreateProjectDraft: (projectPath) => {
        const state = get();
        const existingId = state.projectDraftIdByPath[projectPath];
        if (existingId) {
          const existing = state.draftsById[existingId];
          // A project slot is only reusable while it is empty. Once the user
          // types or attaches context, "new agent" means a genuinely new
          // draft; the invested session remains reachable from the sidebar.
          if (isReusableDraft(existing) && !draftHasLiveUserContent(existing)) {
            return existing;
          }
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

      markMaterialized: (draftId, materializedTo) =>
        set((s) => {
          const existing = s.draftsById[draftId];
          if (!existing) return s;
          return {
            draftsById: {
              ...s.draftsById,
              [draftId]: { ...existing, materializedTo },
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

      discardDraft: (draftId) => {
        const existing = get().draftsById[draftId];
        if (!existing) return;
        discardDraftRuntime(existing);
        set((s) => {
          const current = s.draftsById[draftId];
          if (!current) return s;
          const nextDrafts = { ...s.draftsById };
          delete nextDrafts[draftId];
          const patch: Partial<ChatDraftStore> = { draftsById: nextDrafts };
          if (s.activeHomeDraftId === draftId) patch.activeHomeDraftId = null;
          if (s.activeDraftId === draftId) patch.activeDraftId = null;
          if (current.target.kind === "project") {
            const path = current.target.projectPath;
            if (s.projectDraftIdByPath[path] === draftId) {
              const nextMap = { ...s.projectDraftIdByPath };
              delete nextMap[path];
              patch.projectDraftIdByPath = nextMap;
            }
          }
          return patch as ChatDraftStore;
        });
      },

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

      clearDraftsForProject: (projectPath, homeDir) => {
        const state = get();
        const sweepHomeDrafts = homeDir !== null && projectPath === homeDir;
        const matching = Object.values(state.draftsById).filter(
          (d) =>
            (d.target.kind === "project" &&
              d.target.projectPath === projectPath) ||
            (sweepHomeDrafts && d.target.kind === "home"),
        );
        if (matching.length === 0) return;
        for (const draft of matching) discardDraftRuntime(draft);
        set((s) => {
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
          if (
            s.activeDraftId &&
            matching.some((d) => d.draftId === s.activeDraftId)
          ) {
            patch.activeDraftId = null;
          }
          return patch as ChatDraftStore;
        });
      },
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
      partialize: partializeChatDraftStoreState,
    },
  ),
);

export type PersistedChatDraftStoreState = Pick<
  ChatDraftStore,
  | "draftsById"
  | "activeHomeDraftId"
  | "projectDraftIdByPath"
  | "activeDraftId"
>;

/** Persist only sessions that still have a way back: the currently active or
 *  mapped empty draft, an invested draft, or an interrupted promotion that
 *  must remain recoverable. Empty unmapped sessions are implementation
 *  leftovers and otherwise accumulate forever. */
export function partializeChatDraftStoreState(
  state: ChatDraftStore,
): PersistedChatDraftStoreState {
  const mappedDraftIds = new Set<DraftId>([
    ...(state.activeHomeDraftId ? [state.activeHomeDraftId] : []),
    ...Object.values(state.projectDraftIdByPath),
  ]);
  const draftsById = Object.fromEntries(
    Object.entries(state.draftsById).filter(([draftId, draft]) => {
      const id = draftId as DraftId;
      return (
        id === state.activeDraftId ||
        mappedDraftIds.has(id) ||
        draft.promoting ||
        draft.materializedTo !== null ||
        draft.lastSendError !== null ||
        draftHasLiveUserContent(draft)
      );
    }),
  ) as Record<DraftId, ChatDraft>;

  return {
    draftsById,
    activeHomeDraftId:
      state.activeHomeDraftId && draftsById[state.activeHomeDraftId]
        ? state.activeHomeDraftId
        : null,
    projectDraftIdByPath: Object.fromEntries(
      Object.entries(state.projectDraftIdByPath).filter(
        ([, draftId]) => draftsById[draftId] !== undefined,
      ),
    ),
    activeDraftId:
      state.activeDraftId && draftsById[state.activeDraftId]
        ? state.activeDraftId
        : null,
  };
}

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
