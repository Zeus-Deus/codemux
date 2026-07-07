import { create } from "zustand";

import { replayPayloads } from "@/lib/agent-chat/hydrate";
import {
  applyEvent,
  appendUserMessage,
  createEmptyThreadState,
  markRequestResolved,
  markRequestResponding,
} from "@/lib/agent-chat/reducer";
import type { ChatThreadState, ChatViewItem } from "@/lib/agent-chat/types";
import type { AgentChatCheckpointRecord } from "@/tauri/commands";
import type {
  ApprovalDecision,
  ProviderRuntimeEvent,
} from "@/tauri/events";

/**
 * Default for new threads: launch in `bypassPermissions`
 * (Full access). Users who want stricter prompting pick it in the
 * footer picker; a mode change triggers a silent session restart.
 */
export const DEFAULT_THREAD_PERMISSION_MODE = "bypassPermissions";

/** Composer-level Cursor-style mode toggle. Orthogonal to the
 *  permission picker: Plan commandeers `permissionMode` under the
 *  hood, Ask/Debug layer a prompt-wrapper (Stages 4 & 6). `default`
 *  means "no pill — regular agent behavior". */
export type ChatMode = "default" | "plan" | "ask" | "debug";

/** Step 8 Stage 1 — attachment kinds drive icon + color in the chip,
 *  injection format at send time (Stage 2+), and UI affordances
 *  (e.g. images need paste/drop handlers, others don't). */
export type AttachmentKind = "file" | "folder" | "issue" | "pr" | "image";

export interface AttachmentMetadata {
  /** Chip display text. For files: filename; for issues/PRs:
   *  `#1234 · title`. Required so a chip always has something to render
   *  even before async fetches resolve. */
  label: string;
  /** File-only: line count for the chip's secondary metric ("421L"). */
  lineCount?: number;
  /** File / image: byte size for the chip tooltip and 5MB warning. */
  bytes?: number;
  /** Issue / PR state — drives chip color. PRs add `draft` (in-progress
   *  open PR) so the chip can render with a dashed-circle icon and
   *  muted tint; issues only ever take open/closed. */
  state?: "open" | "closed" | "merged" | "draft";
  /** When the resolved content was fetched, for the
   *  re-fetch-if-older-than-60s rule (Stage 4–5). Files re-read at
   *  send unconditionally so this is mainly for GitHub kinds. */
  fetchedAt?: number;
  /** While the chip is waiting on an async fetch (issues / PRs). */
  isLoading?: boolean;
  /** Display an error indicator on the chip (e.g. fetch failed). */
  error?: string;
  /** Step 8 Stage 7 — file-only: backend reported `truncated: true`
   *  (preview + outline rather than full content). Drives the
   *  "first 50/N L" indicator on the chip. */
  isTruncated?: boolean;
  /** Step 8 Stage 7 — pr-only: user clicked the expand affordance to
   *  swap from name-only diff to the full unified diff. Toggling this
   *  triggers a re-fetch + re-render of the resolved content. */
  expandFullDiff?: boolean;
}

export interface Attachment {
  /** Stable id for chip dedup + removal. Caller-generated (uuid). */
  id: string;
  kind: AttachmentKind;
  /** Discriminator-specific reference: path | "#1234" | "image:<id>". */
  ref: string;
  metadata: AttachmentMetadata;
  /** Resolved text payload for files / folders / issues / PRs. Populated
   *  at attach time for files, at fetch time for GitHub kinds. */
  resolvedContent?: string;
  /** Image-only: decoded bytes. Not persisted; re-attached if the user
   *  re-pastes after a session restart. */
  resolvedImage?: {
    mime: string;
    bytes: Uint8Array;
  };
}

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
  /** Composer-level Cursor-style mode pill. `default` means no pill. */
  mode: ChatMode;
  /** When Plan pill activates, we stash the prior `permissionMode`
   *  here so toggle-off can restore it. `null` at all other times. */
  modePriorPermissionMode: string | null;
  /** Seeded true by Stage 6's grep-on-open when `// CODEMUX_DEBUG`
   *  markers exist in the workspace, and flipped on by Claude's
   *  first debug-mode tool use. Drives the "Clean up debug logs"
   *  affordance. Defined on the slice from Stage 3 onward but
   *  unused until Stage 6. */
  hasDebugActivity: boolean;
  /** Set to `true` once the background grep-on-open has finished
   *  (success or failure). The Cleanup banner / exit dialog only
   *  trust `hasDebugActivity` once this flips, so a slow grep can't
   *  cause us to flash a stale "no markers" state. */
  debugActivityResolved: boolean;
  /** Step 8 Stage 1 — attachments staged for the next turn (files,
   *  folders, issues, PRs, images). Renders as a chip strip above the
   *  textarea. Cleared on send by the parent surface (Stage 2 wires
   *  this); Stage 1 only exposes the slice + actions. The slice itself
   *  imposes no cap — UI layer enforces 20 hard / 10 soft warn. */
  stagedAttachments: Attachment[];
  /** Run-start rollback checkpoint (issue #80). `null` until the
   *  background snapshot lands (event) or the on-mount fetch resolves.
   *  Drives the pane header's "Restore checkpoint" affordance. */
  checkpoint: AgentChatCheckpointRecord | null;
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
    mode: "default",
    modePriorPermissionMode: null,
    hasDebugActivity: false,
    debugActivityResolved: false,
    stagedAttachments: [],
    checkpoint: null,
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
  /** Locally mark a permission request resolved (no sidecar round-trip).
   *  Used for plan accept/reject — the sidecar denied+interrupted the
   *  ExitPlanMode tool, so no `request-resolved` notification will fire. */
  markRequestResolved: (
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
  /** Replace a thread's transcript (and seq counter) by replaying
   *  persisted message payloads through the pure reducer. Used on
   *  the session-history "Resume" path to restore the visible
   *  conversation before a fresh provider session boots. UI-only
   *  fields (model, permissionMode, mode, …) on the slice are
   *  preserved so resume keeps the user's current picker choices.
   *  Calling on an unknown thread creates the slice. */
  hydrateThread: (threadId: string, payloads: string[]) => void;
  /** Clear a thread entirely (e.g. on session stop). */
  resetThread: (threadId: string) => void;
  /** Seed / update the thread's resume cursor. Normally set by the
   *  `ResumeCursorUpdated` event, but the mount-seed effect also uses
   *  this to restore `{ resume: sdk_session_id }` from the persisted
   *  session row after an app restart so a picker-triggered silent
   *  restart resumes the SDK session instead of starting fresh. */
  setResumeCursor: (threadId: string, resumeCursor: unknown | null) => void;
  /** Set the thread's reasoning/effort level. `null` clears to default. */
  setEffort: (threadId: string, effort: string | null) => void;
  /** Set the thread's context-window selection. `null` clears. */
  setContextWindow: (threadId: string, contextWindow: string | null) => void;
  /** Set the composer-level Cursor-style mode. */
  setMode: (threadId: string, mode: ChatMode) => void;
  /** Stash / clear the permissionMode value to restore when the
   *  Plan pill is removed. Pass `null` to clear. */
  setModePriorPermissionMode: (
    threadId: string,
    priorMode: string | null,
  ) => void;
  /** Flip the `hasDebugActivity` flag (Stage 6). Defined on the
   *  store now so Stage 3 doesn't need another migration. */
  setHasDebugActivity: (threadId: string, value: boolean) => void;
  /** Mark the background debug-marker grep as finished. Stage 6. */
  setDebugActivityResolved: (threadId: string, value: boolean) => void;
  /** Step 8 Stage 1 — append a fresh attachment chip to the staging
   *  list. Caller is responsible for generating `id` and ensuring it
   *  doesn't collide. Cap (20) is enforced by the composer UI, not
   *  here, so tests can stage 21+ without the slice rejecting. */
  addStagedAttachment: (threadId: string, attachment: Attachment) => void;
  /** Patch metadata or resolved content on a staged attachment. Used
   *  by async fetches for issues/PRs to flip `isLoading` → resolved
   *  state once `gh` returns. Unknown id is a no-op. */
  updateStagedAttachment: (
    threadId: string,
    id: string,
    patch: Partial<Attachment>,
  ) => void;
  /** Remove a staged attachment by id (X button on the chip). Unknown
   *  id is a no-op. */
  removeStagedAttachment: (threadId: string, id: string) => void;
  /** Clear all staged attachments. Called by Stage 2's send-handler
   *  alongside the existing `inputDraft = ""` reset. */
  clearStagedAttachments: (threadId: string) => void;
  /** Record (or clear) the thread's run-start rollback checkpoint.
   *  Fed by the `agent_chat_checkpoint` event and the header's
   *  on-mount fetch. */
  setCheckpoint: (
    threadId: string,
    checkpoint: AgentChatCheckpointRecord | null,
  ) => void;
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

  markRequestResolved: (threadId, requestId, decision) =>
    set((state) =>
      updateSlice(state, threadId, (slice) => ({
        ...slice,
        ...markRequestResolved(slice, requestId, decision),
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
        // The old thread's checkpoint row dies with its session row
        // (FK cascade on the restart-dedup path); the new session
        // start takes a fresh checkpoint and its event re-populates
        // this. Carrying the stale record would render a restore
        // button pointing at a deleted row.
        checkpoint: null,
      };
      const nextThreads: Record<string, ChatThreadSlice> = {};
      for (const [key, value] of Object.entries(state.threads)) {
        if (key === oldThreadId) continue;
        nextThreads[key] = value;
      }
      nextThreads[newThreadId] = migrated;
      return { threads: nextThreads };
    }),

  hydrateThread: (threadId, payloads) =>
    set((state) => {
      const replayed = replayPayloads(payloads);
      const existing = state.threads[threadId] ?? emptySlice();
      // Spread `replayed` last so its `messages`, `nextSeq`,
      // `streaming`, `pendingRequestIds` overwrite the empty slice's
      // defaults. UI-only fields (model, permissionMode, mode, …)
      // come from `existing` because picker choices belong to the
      // user, not to the historical conversation.
      const next: ChatThreadSlice = {
        ...existing,
        ...replayed,
        // Hydration is a fresh-start scenario — drop ephemeral
        // turn-state fields the live event stream owns.
        activeTurnId: null,
      };
      return { threads: { ...state.threads, [threadId]: next } };
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

  setResumeCursor: (threadId, resumeCursor) =>
    set((state) =>
      updateSlice(state, threadId, (slice) =>
        slice.resumeCursor === resumeCursor
          ? slice
          : { ...slice, resumeCursor },
      ),
    ),

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

  setMode: (threadId, mode) =>
    set((state) =>
      updateSlice(state, threadId, (slice) =>
        slice.mode === mode ? slice : { ...slice, mode },
      ),
    ),

  setModePriorPermissionMode: (threadId, priorMode) =>
    set((state) =>
      updateSlice(state, threadId, (slice) =>
        slice.modePriorPermissionMode === priorMode
          ? slice
          : { ...slice, modePriorPermissionMode: priorMode },
      ),
    ),

  setHasDebugActivity: (threadId, value) =>
    set((state) =>
      updateSlice(state, threadId, (slice) =>
        slice.hasDebugActivity === value
          ? slice
          : { ...slice, hasDebugActivity: value },
      ),
    ),

  setDebugActivityResolved: (threadId, value) =>
    set((state) =>
      updateSlice(state, threadId, (slice) =>
        slice.debugActivityResolved === value
          ? slice
          : { ...slice, debugActivityResolved: value },
      ),
    ),

  addStagedAttachment: (threadId, attachment) =>
    set((state) =>
      updateSlice(state, threadId, (slice) => ({
        ...slice,
        stagedAttachments: [...slice.stagedAttachments, attachment],
      })),
    ),

  updateStagedAttachment: (threadId, id, patch) =>
    set((state) =>
      updateSlice(state, threadId, (slice) => {
        const idx = slice.stagedAttachments.findIndex((a) => a.id === id);
        if (idx < 0) return slice;
        const existing = slice.stagedAttachments[idx];
        const next: Attachment = {
          ...existing,
          ...patch,
          // Merge metadata at one level so callers can patch a single
          // metadata field without clobbering the whole object.
          metadata: { ...existing.metadata, ...(patch.metadata ?? {}) },
        };
        const nextList = [...slice.stagedAttachments];
        nextList[idx] = next;
        return { ...slice, stagedAttachments: nextList };
      }),
    ),

  removeStagedAttachment: (threadId, id) =>
    set((state) =>
      updateSlice(state, threadId, (slice) => {
        const next = slice.stagedAttachments.filter((a) => a.id !== id);
        if (next.length === slice.stagedAttachments.length) return slice;
        return { ...slice, stagedAttachments: next };
      }),
    ),

  clearStagedAttachments: (threadId) =>
    set((state) =>
      updateSlice(state, threadId, (slice) =>
        slice.stagedAttachments.length === 0
          ? slice
          : { ...slice, stagedAttachments: [] },
      ),
    ),

  setCheckpoint: (threadId, checkpoint) =>
    set((state) =>
      updateSlice(state, threadId, (slice) =>
        slice.checkpoint === checkpoint ? slice : { ...slice, checkpoint },
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
