import { create } from "zustand";

import {
  applyEvent,
  appendUserMessage,
  createEmptyThreadState,
  markRequestResponding,
} from "@/lib/agent-chat/reducer";
import type { ChatThreadState, ChatViewItem } from "@/lib/agent-chat/types";
import type {
  ApprovalDecision,
  ProviderRuntimeEvent,
} from "@/tauri/events";

/**
 * T3Code-aligned default: new threads launch in `bypassPermissions`
 * (Full access). Users who want stricter prompting pick it in the
 * footer picker; a mode change triggers a silent session restart.
 */
export const DEFAULT_THREAD_PERMISSION_MODE = "bypassPermissions";

export interface ChatThreadSlice extends ChatThreadState {
  model: string | null;
  /** User's current choice from the picker. */
  permissionMode: string;
  /** The mode the active SDK session was actually launched with.
   *  `null` before the session starts. Drives restart detection:
   *  if `permissionMode !== sessionLaunchMode`, restart. */
  sessionLaunchMode: string | null;
  inputDraft: string;
  activeTurnId: string | null;
  /** SDK session_id wrapped in a resume payload. `null` until the
   *  first SDK message lands and `ResumeCursorUpdated` fires. */
  resumeCursor: unknown | null;
  /** Reasoning / effort level. For Claude this is session-scoped (the
   *  pane restarts on change); for Codex it's per-turn (no restart).
   *  `null` means "use the model default". */
  effort: string | null;
  /** Context-window selection (Claude-only today). `null` means "use
   *  the model default". */
  contextWindow: string | null;
}

function emptySlice(): ChatThreadSlice {
  return {
    ...createEmptyThreadState(),
    model: null,
    permissionMode: DEFAULT_THREAD_PERMISSION_MODE,
    sessionLaunchMode: null,
    inputDraft: "",
    activeTurnId: null,
    resumeCursor: null,
    effort: null,
    contextWindow: null,
  };
}

interface AgentChatStore {
  threads: Record<string, ChatThreadSlice>;

  /** Ensure a slice exists for a given thread id. */
  ensureThread: (threadId: string) => void;
  /** Apply a canonical provider event to the matching slice. */
  applyEvent: (threadId: string, event: ProviderRuntimeEvent) => void;
  /** Append a user message optimistically (no provider echo). */
  appendUserMessage: (threadId: string, text: string) => void;
  /** Flag a permission request as in-flight while the invoke runs. */
  markRequestResponding: (
    threadId: string,
    requestId: string,
    decision: ApprovalDecision,
  ) => void;
  /** Persist the composer draft per thread. */
  setInputDraft: (threadId: string, draft: string) => void;
  /** Seed / update the active model. */
  setModel: (threadId: string, model: string | null) => void;
  /** Update the user's picked permission mode (not the launched one). */
  setPermissionMode: (threadId: string, mode: string) => void;
  /** Set the mode the current SDK session was actually launched with.
   *  Called once per session-start, echoed from the start_session
   *  response to avoid drift. */
  setSessionLaunchMode: (threadId: string, mode: string) => void;
  /** Migrate an existing slice to a new thread id (used on silent
   *  restart: the new SDK session gets a new thread id but the
   *  transcript, model, and draft should persist). */
  migrateThreadId: (oldThreadId: string, newThreadId: string) => void;
  /** Clear a thread entirely (e.g. on session stop). */
  resetThread: (threadId: string) => void;
  /** Set the thread's reasoning/effort level. `null` clears to default. */
  setEffort: (threadId: string, effort: string | null) => void;
  /** Set the thread's context-window selection. `null` clears. */
  setContextWindow: (threadId: string, contextWindow: string | null) => void;
}

function updateSlice(
  state: AgentChatStore,
  threadId: string,
  update: (slice: ChatThreadSlice) => ChatThreadSlice,
): Partial<AgentChatStore> {
  const existing = state.threads[threadId] ?? emptySlice();
  const next = update(existing);
  if (next === existing) return {};
  return { threads: { ...state.threads, [threadId]: next } };
}

export const useAgentChatStore = create<AgentChatStore>((set) => ({
  threads: {},

  ensureThread: (threadId) =>
    set((state) =>
      state.threads[threadId]
        ? {}
        : { threads: { ...state.threads, [threadId]: emptySlice() } },
    ),

  applyEvent: (threadId, event) =>
    set((state) =>
      updateSlice(state, threadId, (slice) => {
        const applied = applyEvent(slice, event);
        // Track active turn id for the Stop button so we can thread it
        // to interrupt_turn if the provider wants a specific turn.
        let activeTurnId = slice.activeTurnId;
        let resumeCursor = slice.resumeCursor;
        if (event.type === "session_state_changed") {
          if (event.status.status === "running") {
            activeTurnId = event.status.active_turn;
          } else if (
            event.status.status === "ready" ||
            event.status.status === "closed" ||
            event.status.status === "error"
          ) {
            activeTurnId = null;
          }
        } else if (event.type === "turn_completed") {
          activeTurnId = null;
        } else if (event.type === "content_delta") {
          activeTurnId = event.turn_id;
        } else if (event.type === "resume_cursor_updated") {
          resumeCursor = event.resume_cursor;
        }
        if (
          applied === (slice as ChatThreadState) &&
          activeTurnId === slice.activeTurnId &&
          resumeCursor === slice.resumeCursor
        ) {
          return slice;
        }
        return { ...slice, ...applied, activeTurnId, resumeCursor };
      }),
    ),

  appendUserMessage: (threadId, text) =>
    set((state) =>
      updateSlice(state, threadId, (slice) => ({
        ...slice,
        ...appendUserMessage(slice, text),
        inputDraft: "",
      })),
    ),

  markRequestResponding: (threadId, requestId, decision) =>
    set((state) =>
      updateSlice(state, threadId, (slice) => ({
        ...slice,
        ...markRequestResponding(slice, requestId, decision),
      })),
    ),

  setInputDraft: (threadId, draft) =>
    set((state) =>
      updateSlice(state, threadId, (slice) =>
        slice.inputDraft === draft ? slice : { ...slice, inputDraft: draft },
      ),
    ),

  setModel: (threadId, model) =>
    set((state) =>
      updateSlice(state, threadId, (slice) =>
        slice.model === model ? slice : { ...slice, model },
      ),
    ),

  setPermissionMode: (threadId, mode) =>
    set((state) =>
      updateSlice(state, threadId, (slice) =>
        slice.permissionMode === mode
          ? slice
          : { ...slice, permissionMode: mode },
      ),
    ),

  setSessionLaunchMode: (threadId, mode) =>
    set((state) =>
      updateSlice(state, threadId, (slice) =>
        slice.sessionLaunchMode === mode
          ? slice
          : { ...slice, sessionLaunchMode: mode },
      ),
    ),

  migrateThreadId: (oldThreadId, newThreadId) =>
    set((state) => {
      if (oldThreadId === newThreadId) return {};
      const existing = state.threads[oldThreadId];
      if (!existing) return {};
      // On restart the old slice becomes the new one verbatim minus
      // the ephemeral turn-state fields. A caller-side step then
      // updates `sessionLaunchMode` (via `setSessionLaunchMode`) and
      // may clear the `resumeCursor` once a fresh one arrives on the
      // new thread's event stream.
      const migrated: ChatThreadSlice = {
        ...existing,
        streaming: false,
        activeTurnId: null,
        pendingRequestIds: [],
      };
      const nextThreads: Record<string, ChatThreadSlice> = {};
      for (const [key, value] of Object.entries(state.threads)) {
        if (key === oldThreadId) continue;
        nextThreads[key] = value;
      }
      nextThreads[newThreadId] = migrated;
      return { threads: nextThreads };
    }),

  resetThread: (threadId) =>
    set((state) => {
      if (!state.threads[threadId]) return {};
      const rest: Record<string, ChatThreadSlice> = {};
      for (const [key, value] of Object.entries(state.threads)) {
        if (key !== threadId) rest[key] = value;
      }
      return { threads: rest };
    }),

  setEffort: (threadId, effort) =>
    set((state) =>
      updateSlice(state, threadId, (slice) =>
        slice.effort === effort ? slice : { ...slice, effort },
      ),
    ),

  setContextWindow: (threadId, contextWindow) =>
    set((state) =>
      updateSlice(state, threadId, (slice) =>
        slice.contextWindow === contextWindow
          ? slice
          : { ...slice, contextWindow },
      ),
    ),
}));

export const selectThread =
  (threadId: string | null) =>
  (state: AgentChatStore): ChatThreadSlice | null =>
    threadId == null ? null : state.threads[threadId] ?? null;

export const selectMessages =
  (threadId: string | null) =>
  (state: AgentChatStore): ChatViewItem[] => {
    if (threadId == null) return EMPTY_ITEMS;
    return state.threads[threadId]?.messages ?? EMPTY_ITEMS;
  };

const EMPTY_ITEMS: ChatViewItem[] = [];
