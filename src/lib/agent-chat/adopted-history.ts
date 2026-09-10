import { useSyncExternalStore } from "react";

import { useAgentChatStore } from "@/stores/agent-chat-store";
import {
  agentChatLoadAdoptedHistory,
  type AgentChatMessageRow,
} from "@/tauri/commands";
import type { AgentChatProviderKind } from "@/tauri/types";

/**
 * The terminal side of an adopted conversation.
 *
 * An adopted thread's own rows start at the "resumed from the terminal"
 * divider; everything the user and the agent said in the CLI before that
 * lives in the provider's session file, which the backend pages out on
 * request. This module owns that import per thread: which page is
 * shown, whether more exists, and where in the agent-chat store the rows
 * were rendered.
 *
 * Rendering strategy: the imported rows are hydrated into a SHADOW thread
 * key (`${threadId}::terminal-history`) through the store's ordinary
 * `hydrateThread`, so the reducer, turn grouping and message components
 * render them exactly as they render a live transcript. `hydrateThread`
 * is a pure store write — the reload path — so an import never fires a
 * notification, never touches usage, and never reads as a new response.
 * Loading an earlier page re-hydrates the shadow key with the earlier
 * rows prepended; rows are cheap, so the concatenation is kept here.
 */

export const ADOPTED_HISTORY_PAGE_SIZE = 40;

export type AdoptedHistoryStatus = "idle" | "loading" | "ready" | "error";

export interface AdoptedHistoryState {
  status: AdoptedHistoryStatus;
  /** Index of the first imported row within the whole session; 0 means
   *  the beginning has been reached and there is nothing earlier. */
  offset: number;
  /** Messages in the whole terminal session. Known once `ready`. */
  total: number;
  /** Agent-chat store key the imported rows are hydrated under. */
  historyKey: string;
  /** Every imported row so far, oldest first. */
  rows: AgentChatMessageRow[];
  /** True while a "Show earlier" page is in flight. The first page uses
   *  `status: "loading"` instead, so consumers can tell "nothing shown
   *  yet" from "more arriving above what is shown". */
  loadingEarlier: boolean;
  /** Message of the last failed load (first page or an earlier page). */
  error: string | null;
}

/** Store key the imported rows of `threadId` are hydrated under. */
export function adoptedHistoryKey(threadId: string): string {
  return `${threadId}::terminal-history`;
}

function idleState(threadId: string): AdoptedHistoryState {
  return {
    status: "idle",
    offset: 0,
    total: 0,
    historyKey: adoptedHistoryKey(threadId),
    rows: [],
    loadingEarlier: false,
    error: null,
  };
}

const states = new Map<string, AdoptedHistoryState>();
// Stable idle objects per thread, so `useSyncExternalStore` sees the same
// snapshot for a thread that has never loaded instead of a fresh literal
// every render.
const idleStates = new Map<string, AdoptedHistoryState>();
const listeners = new Set<() => void>();
// Per-thread request generation: a reset or a competing load makes any
// response still in flight stale, and a stale response must not write.
const generations = new Map<string, number>();

function notify(): void {
  for (const listener of listeners) listener();
}

function setState(threadId: string, next: AdoptedHistoryState): void {
  states.set(threadId, next);
  notify();
}

/** Current import state for `threadId`; idle when nothing was loaded. */
export function getAdoptedHistory(threadId: string): AdoptedHistoryState {
  const known = states.get(threadId);
  if (known) return known;
  let idle = idleStates.get(threadId);
  if (!idle) {
    idle = idleState(threadId);
    idleStates.set(threadId, idle);
  }
  return idle;
}

export function subscribeAdoptedHistory(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

const NO_THREAD_STATE: AdoptedHistoryState = {
  status: "idle",
  offset: 0,
  total: 0,
  historyKey: "",
  rows: [],
  loadingEarlier: false,
  error: null,
};

/** Subscribe a component to one thread's import state. */
export function useAdoptedHistory(
  threadId: string | null | undefined,
): AdoptedHistoryState {
  return useSyncExternalStore(
    subscribeAdoptedHistory,
    () => (threadId ? getAdoptedHistory(threadId) : NO_THREAD_STATE),
    () => (threadId ? getAdoptedHistory(threadId) : NO_THREAD_STATE),
  );
}

function errorMessage(err: unknown): string {
  if (typeof err === "string") return err;
  if (err instanceof Error) return err.message;
  return String(err);
}

function bumpGeneration(threadId: string): number {
  const next = (generations.get(threadId) ?? 0) + 1;
  generations.set(threadId, next);
  return next;
}

function hydrateShadow(
  historyKey: string,
  rows: AgentChatMessageRow[],
  provider: AgentChatProviderKind | null | undefined,
): void {
  // `hydrateThread` replaces the shadow transcript wholesale from the
  // rows and nothing else: no notification, no usage, no active turn.
  useAgentChatStore
    .getState()
    .hydrateThread(historyKey, rows, provider ? { provider } : undefined);
}

/**
 * Load the most recent page of the terminal history behind `threadId`
 * and hydrate it into the shadow key. Idempotent: a thread that is
 * already loading or loaded is left alone (call
 * {@link loadEarlierAdoptedHistory} for more). Never throws — a failure
 * lands in `status: "error"` with its message, and the transcript keeps
 * saying the agent still has the history.
 */
export async function loadAdoptedHistory(
  threadId: string,
  provider?: AgentChatProviderKind | null,
): Promise<void> {
  const current = getAdoptedHistory(threadId);
  if (current.status === "loading" || current.status === "ready") return;
  const generation = bumpGeneration(threadId);
  setState(threadId, { ...current, status: "loading", error: null });
  try {
    const page = await agentChatLoadAdoptedHistory(
      threadId,
      null,
      ADOPTED_HISTORY_PAGE_SIZE,
    );
    if (generations.get(threadId) !== generation) return;
    const historyKey = adoptedHistoryKey(threadId);
    hydrateShadow(historyKey, page.rows, provider);
    setState(threadId, {
      status: "ready",
      offset: page.offset,
      total: page.total,
      historyKey,
      rows: page.rows,
      loadingEarlier: false,
      error: null,
    });
  } catch (err) {
    if (generations.get(threadId) !== generation) return;
    console.warn("[agent-chat] adopted history load failed:", err);
    setState(threadId, {
      ...getAdoptedHistory(threadId),
      status: "error",
      loadingEarlier: false,
      error: errorMessage(err),
    });
  }
}

/**
 * Load the page before what is shown and re-hydrate the shadow key with
 * it prepended. No-op unless the first page is in and the beginning has
 * not been reached. Never throws; a failure keeps the shown rows and
 * records the message so the control can say so.
 */
export async function loadEarlierAdoptedHistory(
  threadId: string,
  provider?: AgentChatProviderKind | null,
): Promise<void> {
  const current = getAdoptedHistory(threadId);
  if (current.status !== "ready" || current.loadingEarlier) return;
  if (current.offset <= 0) return;
  const generation = bumpGeneration(threadId);
  setState(threadId, { ...current, loadingEarlier: true, error: null });
  try {
    const page = await agentChatLoadAdoptedHistory(
      threadId,
      current.offset,
      ADOPTED_HISTORY_PAGE_SIZE,
    );
    if (generations.get(threadId) !== generation) return;
    const latest = getAdoptedHistory(threadId);
    const rows = [...page.rows, ...latest.rows];
    hydrateShadow(latest.historyKey, rows, provider);
    setState(threadId, {
      ...latest,
      offset: page.offset,
      total: page.total,
      rows,
      loadingEarlier: false,
      error: null,
    });
  } catch (err) {
    if (generations.get(threadId) !== generation) return;
    console.warn("[agent-chat] earlier adopted history load failed:", err);
    setState(threadId, {
      ...getAdoptedHistory(threadId),
      loadingEarlier: false,
      error: errorMessage(err),
    });
  }
}

/** Drop everything imported for `threadId` (state and shadow slice), so
 *  the next mount loads afresh. Also invalidates any response in flight. */
export function forgetAdoptedHistory(threadId: string): void {
  bumpGeneration(threadId);
  const known = states.get(threadId);
  states.delete(threadId);
  if (known) {
    useAgentChatStore.getState().resetThread(known.historyKey);
    notify();
  }
}

/** Test hook: clear every thread's import state. */
export function resetAdoptedHistoryForTests(): void {
  for (const threadId of [...states.keys()]) forgetAdoptedHistory(threadId);
  states.clear();
  idleStates.clear();
  generations.clear();
  notify();
}
