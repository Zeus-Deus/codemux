import { create } from "zustand";

import {
  applyTimedReplayTail,
  parseTimedReplayPayloads,
  replayTimed,
  stampSearchSourceId,
} from "@/lib/agent-chat/hydrate";
import type { ReplayOptions } from "@/lib/agent-chat/hydrate";
import { isLazyToolResultStub } from "@/lib/agent-chat/lazy-tool-result";
import {
  applyEvent,
  appendUserMessage,
  createEmptyThreadState,
  markRequestPending,
  markRequestResolved,
  markRequestResponding,
  removeUserMessageByNonce,
} from "@/lib/agent-chat/reducer";
import type {
  ChatThreadState,
  ChatViewItem,
  LiveChatEvent,
  ToolCallItem,
  UserMessageImage,
} from "@/lib/agent-chat/types";
import type {
  AgentChatCheckpointRecord,
  AgentChatMessageRow,
} from "@/tauri/commands";
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
   *  re-pastes after a session restart. Kept in memory even after
   *  `stagedImage` lands because the optimistic bubble's `data:` URL is
   *  built from these bytes. */
  resolvedImage?: {
    mime: string;
    bytes: Uint8Array;
  };
  /** Image-only: reference to the backend staging file the bytes were
   *  written to at attach time (`agent_chat_stage_image`). Populated
   *  asynchronously after `resolvedImage`; the turn's `images` payload
   *  reads `path` from here so the raw bytes never cross IPC as JSON.
   *  Absent while staging is in flight or after a staging failure (the
   *  chip then carries a `metadata.error` and send is blocked). */
  stagedImage?: {
    path: string;
    mediaType: string;
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
  /** Premium inference speed for models that advertise fast-mode support. */
  fastMode: boolean;
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
  /** Durable resume cursor: the highest `agent_chat_messages.id` whose
   *  effect this slice already holds. A remount asks the backend for
   *  everything after it instead of replaying from row zero.
   *
   *  `null` means "never hydrated from disk" — the slice may hold live
   *  events whose position in the row space is unknown, so the next
   *  hydrate must be a cold full replay (which replaces the transcript
   *  wholesale and therefore cannot duplicate anything). A hydrate that
   *  found no rows sets `0`, not `null`: the thread IS synced, it just
   *  starts at the beginning of the id space.
   *
   *  Invariant the whole design rests on: every row with an id at or
   *  below this cursor is represented in `messages`. Live events may
   *  therefore only advance it when it is already non-null — otherwise a
   *  single live event would strand every unread row below it. */
  lastPersistedEventId: number | null;
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
    fastMode: false,
    mode: "default",
    modePriorPermissionMode: null,
    hasDebugActivity: false,
    debugActivityResolved: false,
    stagedAttachments: [],
    checkpoint: null,
    lastPersistedEventId: null,
  };
}

interface AgentChatStore {
  threads: Record<string, ChatThreadSlice>;

  /** Ensure a slice exists for a given thread id. */
  ensureThread: (threadId: string) => void;
  /** Apply a canonical provider event to the matching slice. */
  applyEvent: (threadId: string, event: ProviderRuntimeEvent) => void;
  /** Apply an ordered batch of provider events in a SINGLE `set()`.
   *  Identical to calling `applyEvent` once per element (same reducer,
   *  same order, same final slice) but it emits one store update, so a
   *  frame's worth of coalesced streaming deltas costs one React render
   *  instead of one per token. See `lib/agent-chat/event-batcher.ts`. */
  applyEvents: (threadId: string, events: ProviderRuntimeEvent[]) => void;
  /** Apply an ordered batch of LIVE events, each tagged with the durable
   *  row id it was persisted as. Identical to `applyEvents` except that
   *  an event whose `persistedId` is at or below the slice's cursor is
   *  dropped as a replay duplicate, and a higher one advances the cursor.
   *  This is the guard that makes "buffer live events across a hydrate,
   *  then drain" safe — the overlap it creates is deduped here rather
   *  than risking a gap. */
  applyLiveEvents: (threadId: string, items: LiveChatEvent[]) => void;
  /** Append a user message optimistically (no provider echo).
   *  `images` (when present) carries the turn's staged images as
   *  `data:` URLs so the bubble renders their thumbnails immediately. */
  appendUserMessage: (
    threadId: string,
    text: string,
    clientNonce?: string,
    images?: UserMessageImage[],
  ) => void;
  /** Roll back an optimistic user bubble by its client nonce (send RPC
   *  failed). No-op when not found. `restoreInterrupted` re-arms the
   *  interrupted flag when the failed send was a resume of an interrupted
   *  thread, so the "Run interrupted" divider + Continue chip survive a failed
   *  click. */
  removeUserMessageByNonce: (
    threadId: string,
    clientNonce: string,
    restoreInterrupted?: boolean,
  ) => void;
  /** Flag a permission request as in-flight while the invoke runs. */
  markRequestResponding: (
    threadId: string,
    requestId: string,
    decision: ApprovalDecision,
  ) => void;
  /** Restore an in-flight request after a retryable response failure. */
  markRequestPending: (threadId: string, requestId: string) => void;
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
  setSessionLaunchMode: (threadId: string, mode: string | null) => void;
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
   *  Calling on an unknown thread creates the slice.
   *
   *  `opts.runLive` is forwarded to `replayPayloads`: when the caller
   *  has confirmed the thread's turn is in flight (remount of a live
   *  run) it suppresses the Run-interrupted heuristic and marks the
   *  slice streaming. Omit for the plain resume path.
   *
   *  `opts.provider` is likewise forwarded: it decides whether an
   *  unresolved persisted request is expired on hydrate (Claude/Codex,
   *  whose callbacks die with the process) or kept actionable (OpenCode,
   *  whose permissions live in its own server). */
  hydrateThread: (
    threadId: string,
    rows: AgentChatMessageRow[],
    opts?: ReplayOptions,
  ) => void;
  /** Warm resume: fold a CURSOR TAIL (rows after `lastPersistedEventId`)
   *  into the existing slice in ONE ordered reduction, then advance the
   *  cursor to the tail's last row id. Unlike `hydrateThread` this never
   *  rebuilds the transcript prefix, and it leaves `activeTurnId` alone
   *  because a live run may own it. No-op for an empty tail — that is
   *  the "warm unchanged revisit does no work" path. */
  applyPayloadsTail: (
    threadId: string,
    rows: AgentChatMessageRow[],
    opts?: ReplayOptions,
  ) => void;
  /** Forget a thread's resume cursor, forcing the next hydrate down the
   *  cold path. Used when a tail read cannot be trusted (a failed
   *  hydrate whose buffered live events are about to land, a cursor
   *  ahead of the thread's own head after a merge). */
  invalidateThreadCursor: (threadId: string) => void;
  /** Swap a lazily-fetched tool-result body into the item that carries
   *  the matching stub. Row ids are table-wide unique, so the lookup
   *  needs no thread id — the card that renders the stub does not know
   *  which slice it belongs to. */
  resolveLazyToolResult: (rowId: number, content: unknown) => void;
  /** Drop warm slices until the estimated total payload is back under
   *  budget, oldest-touched first. `keep` is the caller's protected set
   *  (the mounted panes' threads); running / pending threads are never
   *  evicted regardless. An evicted thread simply cold-hydrates on its
   *  next visit. */
  evictColdThreads: (keep: string[]) => void;
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
  /** Enable or disable the provider's premium speed tier. */
  setFastMode: (threadId: string, fastMode: boolean) => void;
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
  touchThread(threadId);
  return { threads: { ...state.threads, [threadId]: next } };
}

// ── Warm-slice retention ─────────────────────────────────────────────
//
// Visited threads stay warm so a revisit costs a cursor tail read instead
// of a full replay — but "every thread ever opened, forever" is how a
// long session ends up holding the whole 165 MB transcript corpus in the
// renderer. Last-touch times and weights live OUTSIDE the slice so
// bookkeeping never changes a slice's identity (which would defeat the
// field-level subscriptions and the transcript's slot reuse).

/** Heuristic byte budget for all warm transcripts combined. The unit is
 *  "estimated payload characters", not real heap bytes. */
export const WARM_SLICE_BUDGET_UNITS = 128 * 1024 * 1024;

const lastTouchedAt = new Map<string, number>();
const itemWeights = new WeakMap<object, number>();

function touchThread(threadId: string): void {
  lastTouchedAt.set(threadId, Date.now());
}

/** Threads with a live pane, by mount count. Split panes and a dev
 *  double-mount can both hold the same thread, so this refcounts rather
 *  than toggling a flag. Lives outside the slice for the same reason the
 *  touch times do: bookkeeping must never change a slice's identity. */
const mountedThreads = new Map<string, number>();

/** Mark a thread as displayed. Returns the matching unregister so a caller
 *  cannot leak a mount by forgetting the thread id. */
export function registerMountedThread(threadId: string): () => void {
  mountedThreads.set(threadId, (mountedThreads.get(threadId) ?? 0) + 1);
  let released = false;
  return () => {
    if (released) return;
    released = true;
    const next = (mountedThreads.get(threadId) ?? 1) - 1;
    if (next <= 0) mountedThreads.delete(threadId);
    else mountedThreads.set(threadId, next);
  };
}

/** Whether a pane is currently displaying this thread — read by eviction. */
export function isThreadMounted(threadId: string): boolean {
  return mountedThreads.has(threadId);
}

/** Cheap size proxy for one transcript item, memoized on the item object.
 *  Items are structurally shared across renders, so a re-estimate after a
 *  streaming delta only walks the handful of items that actually changed. */
function estimateItemWeight(item: ChatViewItem): number {
  const cached = itemWeights.get(item);
  if (cached !== undefined) return cached;
  const weight = estimateValueWeight(item, 0);
  itemWeights.set(item, weight);
  return weight;
}

function estimateValueWeight(value: unknown, depth: number): number {
  if (value == null) return 0;
  if (typeof value === "string") return value.length;
  if (typeof value === "number" || typeof value === "boolean") return 8;
  if (typeof value !== "object") return 8;
  // Depth cap: tool payloads can nest arbitrarily and this is a budget
  // heuristic, not an allocator.
  if (depth >= 6) return 64;
  if (Array.isArray(value)) {
    let weight = 16;
    for (const entry of value) weight += estimateValueWeight(entry, depth + 1);
    return weight;
  }
  let weight = 32;
  for (const [key, entry] of Object.entries(value)) {
    weight += key.length + estimateValueWeight(entry, depth + 1);
  }
  return weight;
}

function estimateSliceWeight(slice: ChatThreadSlice): number {
  let weight = slice.inputDraft.length;
  for (const item of slice.messages) weight += estimateItemWeight(item);
  return weight;
}

/** A thread the user could still be watching — or has unsent work in —
 *  must never be evicted, no matter how cold its last touch looks.
 *
 *  Unsent composer state is the important half: eviction drops the whole
 *  slice, and a typed-but-unsent message or a staged attachment exists
 *  NOWHERE else. Run state is recoverable from disk; a draft is not. */
function isSliceBusy(slice: ChatThreadSlice): boolean {
  return (
    slice.streaming ||
    slice.activeTurnId !== null ||
    slice.pendingRequestIds.length > 0 ||
    slice.inputDraft.trim().length > 0 ||
    slice.stagedAttachments.length > 0
  );
}

/**
 * One provider event folded into one slice. Returns the SAME slice object
 * when nothing changed, which is what keeps `updateSlice` from cloning the
 * threads map (and, in a batch, what lets a no-op event drop out for free).
 */
function reduceThreadEvent(
  slice: ChatThreadSlice,
  event: ProviderRuntimeEvent,
): ChatThreadSlice {
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
      updateSlice(state, threadId, (slice) => reduceThreadEvent(slice, event)),
    ),

  applyEvents: (threadId, events) =>
    set((state) => {
      if (events.length === 0) return {};
      return updateSlice(state, threadId, (slice) => {
        let next = slice;
        for (const event of events) next = reduceThreadEvent(next, event);
        return next;
      });
    }),

  applyLiveEvents: (threadId, items) =>
    set((state) => {
      if (items.length === 0) return {};
      return updateSlice(state, threadId, (slice) => {
        let next = slice;
        let cursor = slice.lastPersistedEventId;
        for (const { event, persistedId } of items) {
          // Already applied through a tail read — the overlap the
          // buffer-then-drain hydrate deliberately creates.
          if (persistedId != null && cursor != null && persistedId <= cursor) {
            continue;
          }
          const before = next;
          next = reduceThreadEvent(next, event);
          if (persistedId != null) {
            next = stampSearchSourceId(before, next, event, persistedId);
          }
          // Only a synced slice may advance: from `null` we have no idea
          // which rows below this id we are missing.
          if (persistedId != null && cursor != null && persistedId > cursor) {
            cursor = persistedId;
          }
        }
        if (cursor === slice.lastPersistedEventId) return next;
        return { ...next, lastPersistedEventId: cursor };
      });
    }),

  appendUserMessage: (threadId, text, clientNonce, images) =>
    set((state) =>
      updateSlice(state, threadId, (slice) => ({
        ...slice,
        ...appendUserMessage(slice, text, undefined, clientNonce, images),
        inputDraft: "",
      })),
    ),

  removeUserMessageByNonce: (threadId, clientNonce, restoreInterrupted) =>
    set((state) =>
      updateSlice(state, threadId, (slice) => ({
        ...slice,
        ...removeUserMessageByNonce(slice, clientNonce, restoreInterrupted),
      })),
    ),

  markRequestResponding: (threadId, requestId, decision) =>
    set((state) =>
      updateSlice(state, threadId, (slice) => ({
        ...slice,
        ...markRequestResponding(slice, requestId, decision),
      })),
    ),

  markRequestPending: (threadId, requestId) =>
    set((state) =>
      updateSlice(state, threadId, (slice) => ({
        ...slice,
        ...markRequestPending(slice, requestId),
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
        // The cursor belongs to the OLD thread's rows. The restart /
        // collapse paths re-home rows between threads keeping their
        // original ids, so the old head can sit above the new thread's
        // head — a cursor that would make every tail read look empty.
        // Cold-hydrate the new id instead.
        lastPersistedEventId: null,
      };
      const nextThreads: Record<string, ChatThreadSlice> = {};
      for (const [key, value] of Object.entries(state.threads)) {
        if (key === oldThreadId) continue;
        nextThreads[key] = value;
      }
      nextThreads[newThreadId] = migrated;
      return { threads: nextThreads };
    }),

  hydrateThread: (threadId, rows, opts) =>
    set((state) => {
      const replayed = replayTimed(parseTimedReplayPayloads(rows), opts);
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
        // Rows arrive ascending, so the last one is the head. An empty
        // hydrate still syncs the thread (cursor 0, not null): its
        // history simply starts at the beginning of the id space.
        lastPersistedEventId: rows.length > 0 ? rows[rows.length - 1].id : 0,
      };
      touchThread(threadId);
      return { threads: { ...state.threads, [threadId]: next } };
    }),

  applyPayloadsTail: (threadId, rows, opts) =>
    set((state) => {
      if (rows.length === 0) return {};
      return updateSlice(state, threadId, (slice) => {
        // Two hydrates can overlap (remount inside the IPC window) and
        // read the same tail from the same cursor. The cursor invariant —
        // every row at or below it is already represented — makes that
        // decidable per row, so a re-applied tail folds nothing twice.
        const cursor = slice.lastPersistedEventId;
        const fresh =
          cursor == null ? rows : rows.filter((row) => row.id > cursor);
        if (fresh.length === 0) return slice;
        const timed = parseTimedReplayPayloads(fresh);
        const merged = applyTimedReplayTail(
          slice,
          timed,
          {
            ...opts,
            // Summarizes the prefix for the unsettled-tail scan: a slice
            // that is streaming or already flagged interrupted ends on an
            // unsettled user turn.
            previousUnsettled: slice.interrupted || slice.streaming,
          },
        );
        return {
          ...slice,
          ...merged,
          // `activeTurnId` is deliberately preserved: a tail merge can
          // happen mid-run and the Stop button needs its turn id.
          lastPersistedEventId: fresh[fresh.length - 1].id,
        };
      });
    }),

  invalidateThreadCursor: (threadId) =>
    set((state) =>
      updateSlice(state, threadId, (slice) =>
        slice.lastPersistedEventId === null
          ? slice
          : { ...slice, lastPersistedEventId: null },
      ),
    ),

  resolveLazyToolResult: (rowId, content) =>
    set((state) => {
      for (const [threadId, slice] of Object.entries(state.threads)) {
        const index = slice.messages.findIndex(
          (item) =>
            item.kind === "tool_call" &&
            isLazyToolResultStub(item.result_content) &&
            item.result_content.__codemux_lazy_tool_result.row_id === rowId,
        );
        if (index < 0) continue;
        const item = slice.messages[index] as ToolCallItem;
        const messages = [...slice.messages];
        // A new item object is exactly right: the transcript's slot reuse
        // keys off item identity, so precisely this row re-renders.
        messages[index] = { ...item, result_content: content };
        return {
          threads: {
            ...state.threads,
            [threadId]: { ...slice, messages },
          },
        };
      }
      return {};
    }),

  evictColdThreads: (keep) =>
    set((state) => {
      const entries = Object.entries(state.threads);
      if (entries.length === 0) return {};
      let total = 0;
      const weights = new Map<string, number>();
      for (const [threadId, slice] of entries) {
        const weight = estimateSliceWeight(slice);
        weights.set(threadId, weight);
        total += weight;
      }
      if (total <= WARM_SLICE_BUDGET_UNITS) return {};
      const protectedIds = new Set(keep);
      const candidates = entries
        .filter(
          ([threadId, slice]) =>
            !protectedIds.has(threadId) &&
            // A mounted pane is rendering this slice right now; dropping
            // it blanks the transcript in place. `keep` only names the
            // thread that just hydrated, which says nothing about the
            // other panes on screen.
            !isThreadMounted(threadId) &&
            !isSliceBusy(slice),
        )
        .sort(
          (a, b) => (lastTouchedAt.get(a[0]) ?? 0) - (lastTouchedAt.get(b[0]) ?? 0),
        );
      const evicted = new Set<string>();
      for (const [threadId] of candidates) {
        if (total <= WARM_SLICE_BUDGET_UNITS) break;
        evicted.add(threadId);
        lastTouchedAt.delete(threadId);
        total -= weights.get(threadId) ?? 0;
      }
      if (evicted.size === 0) return {};
      const threads: Record<string, ChatThreadSlice> = {};
      for (const [threadId, slice] of entries) {
        if (!evicted.has(threadId)) threads[threadId] = slice;
      }
      return { threads };
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

  setFastMode: (threadId, fastMode) =>
    set((state) =>
      updateSlice(state, threadId, (slice) =>
        slice.fastMode === fastMode ? slice : { ...slice, fastMode },
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

// Frozen so a null-thread read is reference-stable AND provably read-only:
// every `?? EMPTY_ITEMS` fallback has to hand back the same array or the
// identity checks the memoized transcript rows rely on are defeated.
const EMPTY_ITEMS = Object.freeze([]) as unknown as ChatViewItem[];
