// One-shot list of the Claude Code conversations that already exist on
// this machine, read through the Agent SDK's supported session-history
// API. Unlike `list-models` / `list-commands` this needs NO transient
// `query()` and NO `pathToClaudeCodeExecutable` — `listSessions()` is a
// pure metadata read, so the probe is cheap enough to run every time the
// picker opens.
//
// The on-disk transcript format is documented as internal and unstable;
// it is never parsed here. Everything returned comes from SDKSessionInfo.

import {
  listSessions as sdkListSessions,
  type SDKSessionInfo,
} from "@anthropic-ai/claude-agent-sdk";

export interface ListSessionsInput {
  /** Absolute project directory to scope discovery to. Omitted =>
   *  every project the SDK knows about on this machine. */
  dir?: string;
  /** Include sibling git worktrees of `dir`. Ignored by the SDK when
   *  `dir` is absent. Always sent explicitly so the SDK-side default
   *  (true) can never drift underneath us. */
  includeWorktrees: boolean;
  /** Hard cap on rows returned. */
  limit: number;
}

/** Where the resolved `title` came from. `"fallback"` means the SDK
 *  reported no title of any kind — combined with a tiny `fileSize`
 *  that is the zero-message-stub signal Rust filters on. */
export type ExternalSessionTitleSource =
  | "custom"
  | "summary"
  | "prompt"
  | "fallback";

export interface ExternalSessionDescriptor {
  /** SDK session UUID. Becomes `agent_chat_sessions.sdk_session_id`. */
  sessionId: string;
  /** Resolved server-side from customTitle || summary || firstPrompt,
   *  trimmed and collapsed to one line. NEVER null and never empty —
   *  falls back to the cwd basename plus a short id when the SDK has
   *  nothing at all. */
  title: string;
  /** Absolute working directory. Required: entries the SDK reports
   *  without a `cwd` are dropped here (counted in `skippedWithoutCwd`)
   *  because adoption attaches to the folder the session lives in. */
  cwd: string;
  gitBranch: string | null;
  /** ISO-8601 UTC. The SDK reports epoch millis. */
  lastModified: string;
  /** ISO-8601 UTC, or null when the SDK omits `createdAt`. */
  createdAt: string | null;
  /** Transcript size in bytes. 0 when the SDK omits `fileSize`
   *  (non-local storage) — never null. */
  fileSize: number;
  titleSource: ExternalSessionTitleSource;
}

export interface ListSessionsResult {
  sessions: ExternalSessionDescriptor[];
  /** Rows the SDK returned with no usable `cwd`, dropped before Rust
   *  saw them. Diagnostic only — Rust logs it, never surfaces it. */
  skippedWithoutCwd: number;
}

// ---------------------------------------------------------------------------
// Dependency-injection seam for tests. Real RPCs read history through the
// SDK; tests hand in a canned list. Mirrors `setQueryFactoryForTests`.
// ---------------------------------------------------------------------------

/** Signature of the SDK's `listSessions`, narrowed to the options this
 *  method actually sends. */
export type SessionLister = (options: {
  dir?: string;
  includeWorktrees: boolean;
  limit: number;
}) => Promise<SDKSessionInfo[]>;

let sessionLister: SessionLister = sdkListSessions;

/** Swap the lister so tests can supply canned session metadata. */
export function setSessionListerForTests(lister: SessionLister): void {
  sessionLister = lister;
}

/** Restore the SDK-backed lister. Call from test teardown. */
export function resetSessionListerForTests(): void {
  sessionLister = sdkListSessions;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Longest title forwarded to Rust. A first-prompt fallback can be an
 *  entire pasted stack trace, and a few hundred of those would dominate
 *  the RPC frame for a picker row that renders one line anyway. */
const MAX_TITLE_LENGTH = 200;

/** Trim and collapse every whitespace run (including newlines) to a
 *  single space. Returns null when nothing survives, so the caller can
 *  fall through to the next candidate. */
function collapseToLine(raw: string | undefined): string | null {
  if (typeof raw !== "string") return null;
  const collapsed = raw.replace(/\s+/g, " ").trim();
  return collapsed === "" ? null : collapsed;
}

/** `collapseToLine` plus the picker-row length cap. */
function normalizeTitle(raw: string | undefined): string | null {
  const collapsed = collapseToLine(raw);
  if (collapsed === null) return null;
  if (collapsed.length <= MAX_TITLE_LENGTH) return collapsed;
  return `${collapsed.slice(0, MAX_TITLE_LENGTH - 1).trimEnd()}…`;
}

/** Last path segment of an absolute directory, tolerating trailing
 *  separators and both separator styles. */
function baseName(dir: string): string {
  const segments = dir.split(/[\\/]+/).filter((s) => s !== "");
  return segments.length > 0 ? (segments[segments.length - 1] as string) : dir;
}

/** Belt-and-braces last resort: the SDK reported no title of any kind.
 *  A live sweep found every session carried one, so this exists so the
 *  contract's "never empty" promise can't be broken by a future SDK. */
function fallbackTitle(cwd: string, sessionId: string): string {
  const shortId = sessionId.slice(0, 8);
  const folder = baseName(cwd);
  return folder === "" ? `Session ${shortId}` : `${folder} · ${shortId}`;
}

function resolveTitle(
  info: SDKSessionInfo,
  cwd: string,
): { title: string; titleSource: ExternalSessionTitleSource } {
  const custom = normalizeTitle(info.customTitle);
  if (custom !== null) return { title: custom, titleSource: "custom" };
  const summary = normalizeTitle(info.summary);
  if (summary !== null) return { title: summary, titleSource: "summary" };
  const prompt = normalizeTitle(info.firstPrompt);
  if (prompt !== null) return { title: prompt, titleSource: "prompt" };
  return {
    title: fallbackTitle(cwd, info.sessionId),
    titleSource: "fallback",
  };
}

/** Stand-in for a `lastModified` the SDK reported as unusable. */
const EPOCH_ISO = new Date(0).toISOString();

/** Epoch millis to ISO-8601 UTC. Returns null for anything the SDK
 *  reports that `Date` can't represent, so a single malformed row can't
 *  throw the whole listing away. */
function toIsoUtc(millis: unknown): string | null {
  if (typeof millis !== "number" || !Number.isFinite(millis)) return null;
  const date = new Date(millis);
  if (Number.isNaN(date.getTime())) return null;
  return date.toISOString();
}

/** Sessions created by throwaway fixtures live under the system temp
 *  directory and are pure noise in the picker. Matched on whole path
 *  segments so a real project named `/tmpfiles` survives.
 *
 *  Only the POSIX temp roots are recognised here — Rust re-applies the
 *  platform-aware check against its own temp directory, so this is a
 *  cheap early trim rather than the authoritative filter. */
const TEMP_ROOTS = ["/tmp", "/private/tmp", "/var/tmp"];

function isTempPath(cwd: string): boolean {
  const normalized = cwd.replace(/\\/g, "/");
  return TEMP_ROOTS.some(
    (root) => normalized === root || normalized.startsWith(`${root}/`),
  );
}

// ---------------------------------------------------------------------------
// Method
// ---------------------------------------------------------------------------

export async function listSessions(
  input: ListSessionsInput,
): Promise<ListSessionsResult> {
  const options: { dir?: string; includeWorktrees: boolean; limit: number } = {
    includeWorktrees: input.includeWorktrees,
    // A non-positive or fractional cap would reach the SDK verbatim;
    // clamp so a bad caller degrades to one row rather than to
    // undefined behaviour.
    limit: Math.max(1, Math.floor(input.limit)),
  };
  if (input.dir !== undefined) options.dir = input.dir;

  const raw = await sessionLister(options);

  const sessions: ExternalSessionDescriptor[] = [];
  let skippedWithoutCwd = 0;

  for (const info of raw) {
    const cwd = typeof info.cwd === "string" ? info.cwd.trim() : "";
    if (cwd === "") {
      skippedWithoutCwd += 1;
      continue;
    }
    if (isTempPath(cwd)) continue;

    const { title, titleSource } = resolveTitle(info, cwd);
    const gitBranch = collapseToLine(info.gitBranch);
    sessions.push({
      sessionId: info.sessionId,
      title,
      cwd,
      gitBranch,
      // A timestamp the SDK reported as unusable sorts to the bottom of
      // the picker rather than costing the user an adoptable session.
      lastModified: toIsoUtc(info.lastModified) ?? EPOCH_ISO,
      createdAt: toIsoUtc(info.createdAt),
      fileSize:
        typeof info.fileSize === "number" && Number.isFinite(info.fileSize)
          ? info.fileSize
          : 0,
      titleSource,
    });
  }

  return { sessions, skippedWithoutCwd };
}
