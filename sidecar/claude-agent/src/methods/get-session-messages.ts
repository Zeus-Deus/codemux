// One page of a Claude Code conversation's history, read through the
// Agent SDK's supported transcript API. Like `list-sessions` this needs
// NO transient `query()` and NO `pathToClaudeCodeExecutable` — the SDK
// parses the transcript itself and hands back the raw API messages.
//
// The on-disk transcript format is documented as internal and unstable;
// it is never parsed here. Records are forwarded exactly as the SDK
// returns them so the client renders one message shape for both live
// and historical turns.
//
// Paging walks BACKWARDS from the newest message: the caller first asks
// for the last N (no `beforeOffset`), then for the N before whatever
// `offset` it was handed, until `offset` reaches 0. The SDK exposes no
// count endpoint and its own `offset` counts from the START, so the
// whole list is loaded once per call and sliced here — a 150 KB session
// parses in a few milliseconds, which is cheap enough for a scrollback.

import {
  getSessionMessages as sdkGetSessionMessages,
  type SessionMessage,
} from "@anthropic-ai/claude-agent-sdk";

export interface GetSessionMessagesInput {
  /** SDK session UUID whose transcript to read. */
  sessionId: string;
  /** Project directory the session lives in. Lets the SDK go straight
   *  to the right transcript folder; when omitted it searches every
   *  project it knows about. */
  dir?: string;
  /** Page size. Clamped to `[1, MAX_PAGE_SIZE]`. */
  limit: number;
  /** Return the messages strictly BEFORE this chronological index.
   *  Omitted => the newest `limit` messages. */
  beforeOffset?: number;
}

export interface GetSessionMessagesResult {
  /** Chronological slice, each record exactly as the SDK returned it. */
  messages: SessionMessage[];
  /** Number of user/assistant messages in the whole session. */
  total: number;
  /** Chronological index of `messages[0]`. `0` means the caller has
   *  reached the beginning of the conversation. */
  offset: number;
}

/** Largest page a single RPC frame will carry. Assistant messages carry
 *  full tool inputs and results, so a few hundred is already megabytes. */
export const MAX_PAGE_SIZE = 500;

// ---------------------------------------------------------------------------
// Dependency-injection seam for tests. Real RPCs read history through the
// SDK; tests hand in a canned list. Mirrors `setSessionListerForTests`.
// ---------------------------------------------------------------------------

/** Signature of the SDK's `getSessionMessages`, narrowed to the options
 *  this method actually sends. */
export type SessionMessageReader = (
  sessionId: string,
  options: { dir?: string; includeSystemMessages: boolean },
) => Promise<SessionMessage[]>;

let sessionMessageReader: SessionMessageReader = sdkGetSessionMessages;

/** Swap the reader so tests can supply a canned transcript. */
export function setSessionMessageReaderForTests(
  reader: SessionMessageReader,
): void {
  sessionMessageReader = reader;
}

/** Restore the SDK-backed reader. Call from test teardown. */
export function resetSessionMessageReaderForTests(): void {
  sessionMessageReader = sdkGetSessionMessages;
}

// ---------------------------------------------------------------------------
// Pure slicing
// ---------------------------------------------------------------------------

/** Bring a caller-supplied page size into `[1, MAX_PAGE_SIZE]`. A
 *  fractional or non-positive value degrades to a sane page rather than
 *  to an empty or unbounded one. */
export function clampPageSize(limit: number): number {
  if (!Number.isFinite(limit)) return 1;
  return Math.min(MAX_PAGE_SIZE, Math.max(1, Math.floor(limit)));
}

/** Pick the page ending just before `beforeOffset` (or the newest page
 *  when it is omitted) out of the full chronological list.
 *
 *  Returned indices are `[max(0, end - limit), end)` where `end` is
 *  `beforeOffset` clamped to `[0, all.length]`. Exported separately so
 *  the paging arithmetic is testable without the SDK. */
export function sliceHistory<T>(
  all: readonly T[],
  page: { limit: number; beforeOffset?: number },
): { messages: T[]; total: number; offset: number } {
  const total = all.length;
  const limit = clampPageSize(page.limit);
  const end =
    page.beforeOffset === undefined
      ? total
      : Math.min(total, Math.max(0, Math.floor(page.beforeOffset)));
  const offset = Math.max(0, end - limit);
  return { messages: all.slice(offset, end), total, offset };
}

// ---------------------------------------------------------------------------
// Method
// ---------------------------------------------------------------------------

export async function getSessionMessages(
  input: GetSessionMessagesInput,
): Promise<GetSessionMessagesResult> {
  const options: { dir?: string; includeSystemMessages: boolean } = {
    // Compact boundaries and other notices are not conversation turns;
    // the client draws its own divider above this history.
    includeSystemMessages: false,
  };
  if (input.dir !== undefined) options.dir = input.dir;

  // No try/catch on purpose: an unknown session must surface as an RPC
  // error, never as an empty page. An empty page means "no messages".
  const all = await sessionMessageReader(input.sessionId, options);

  const page: { limit: number; beforeOffset?: number } = { limit: input.limit };
  if (input.beforeOffset !== undefined) page.beforeOffset = input.beforeOffset;
  return sliceHistory(all, page);
}
