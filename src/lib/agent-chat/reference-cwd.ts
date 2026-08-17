import type { ChatViewItem, ToolCallItem } from "./types";

const POSIX_ABSOLUTE = /^\//;
const WINDOWS_ABSOLUTE = /^[a-zA-Z]:[\\/]/;
const DIRECTORY_KEYS = new Set([
  "cwd",
  "workdir",
  "workingDirectory",
  "working_directory",
]);

/**
 * Keys whose values are payloads the agent *authored* rather than context it
 * ran in: file contents, subagent prompts, patch bodies. A written
 * `launch.json` or a quoted prompt can contain the literal text
 * `"cwd": "/somewhere"`, and treating that as the turn's working directory
 * silently repoints every relative link in the answer at another tree.
 * Directory context is only read from the call's own fields.
 */
const CONTENT_KEYS = new Set([
  "content",
  "contents",
  "prompt",
  "text",
  "body",
  "message",
  "description",
  "instructions",
  "patch",
  "diff",
  "old_string",
  "new_string",
  "old_str",
  "new_str",
]);

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isAbsoluteDirectory(value: unknown): value is string {
  return (
    typeof value === "string" &&
    value.length > 0 &&
    (POSIX_ABSOLUTE.test(value) || WINDOWS_ABSOLUTE.test(value))
  );
}

/** Declared directory fields, at any nesting depth (`exec_command` nests the
 *  real call one level down), skipping authored payloads. */
function directoryFromStructuredInput(input: unknown): string | null {
  if (Array.isArray(input)) {
    for (let index = input.length - 1; index >= 0; index -= 1) {
      const found = directoryFromStructuredInput(input[index]);
      if (found) return found;
    }
    return null;
  }
  if (!isRecord(input)) return null;

  for (const key of DIRECTORY_KEYS) {
    const value = input[key];
    if (isAbsoluteDirectory(value)) return value;
  }
  for (const [key, value] of Object.entries(input).reverse()) {
    if (CONTENT_KEYS.has(key)) continue;
    const found = directoryFromStructuredInput(value);
    if (found) return found;
  }
  return null;
}

/**
 * Codex may persist a freeform orchestration call rather than the nested
 * `exec_command` object. Recognise the JSON/JavaScript property spelling in
 * that source too. This is intentionally narrow: only an explicitly absolute
 * directory is trusted as link context.
 */
function directoryFromFreeformInput(input: unknown): string | null {
  if (typeof input !== "string") return null;
  const matches = [
    ...input.matchAll(
      /["'](?:cwd|workdir|workingDirectory|working_directory)["']\s*:\s*["']([^"'\r\n]+)["']/g,
    ),
  ];
  for (let index = matches.length - 1; index >= 0; index -= 1) {
    const candidate = matches[index]?.[1];
    if (isAbsoluteDirectory(candidate)) return candidate;
  }
  return null;
}

/**
 * The freeform scan is deliberately shallow: the whole input when it is a
 * string, or a top-level field of the call (a command line, an argv array,
 * the serialized orchestration `source`). It never descends into nested
 * objects or authored payloads, so a Write whose `content` happens to spell
 * `"cwd": "/tmp/elsewhere"` cannot repoint link resolution.
 */
function directoryFromShallowFreeform(input: unknown): string | null {
  if (typeof input === "string") return directoryFromFreeformInput(input);
  if (Array.isArray(input)) return directoryFromFreeformList(input);
  if (!isRecord(input)) return null;
  for (const [key, value] of Object.entries(input).reverse()) {
    if (CONTENT_KEYS.has(key)) continue;
    const found =
      typeof value === "string"
        ? directoryFromFreeformInput(value)
        : Array.isArray(value)
          ? directoryFromFreeformList(value)
          : null;
    if (found) return found;
  }
  return null;
}

function directoryFromFreeformList(values: readonly unknown[]): string | null {
  for (let index = values.length - 1; index >= 0; index -= 1) {
    const found = directoryFromFreeformInput(values[index]);
    if (found) return found;
  }
  return null;
}

export function toolCallReferenceCwd(input: unknown): string | null {
  return directoryFromStructuredInput(input) ?? directoryFromShallowFreeform(input);
}

/** Newest-last, deduped, and bounded: fallback resolution walks from the
 *  end, so the most recent mention of a basename wins. */
const MAX_REFERENCE_PATHS = 32;

// Absolute file-path tokens embedded in freeform text (command lines,
// prompts). Segment charset mirrors `FILE_BASENAME` in `file-links.ts`
// minus whitespace — a spaced path inside a command line is ambiguous, and
// these tokens are only ever *candidates* that a stat must confirm. The
// lookbehind guards keep a mid-token slash from starting a match, so
// `notes/todo.txt` and `https://host/x.png` contribute nothing.
const POSIX_PATH_TOKEN =
  /(?<![\w@+~.:\\/-])\/(?:[\w@+~.-]+\/)*[\w@+~.-]+\.[a-zA-Z][a-zA-Z\d]{0,11}(?![\w.])/g;
const WINDOWS_PATH_TOKEN =
  /(?<![\w@+~.:\\/-])[a-zA-Z]:[\\/](?:[\w@+~.-]+[\\/])*[\w@+~.-]+\.[a-zA-Z][a-zA-Z\d]{0,11}(?![\w.])/g;

function rememberPath(path: string, into: string[]) {
  const existing = into.indexOf(path);
  if (existing !== -1) into.splice(existing, 1);
  into.push(path);
  if (into.length > MAX_REFERENCE_PATHS) into.shift();
}

function pathsFromString(value: string, into: string[]) {
  for (const match of value.matchAll(POSIX_PATH_TOKEN)) rememberPath(match[0], into);
  for (const match of value.matchAll(WINDOWS_PATH_TOKEN)) rememberPath(match[0], into);
}

function pathsFromStructuredInput(input: unknown, into: string[]) {
  if (typeof input === "string") {
    pathsFromString(input, into);
    return;
  }
  if (Array.isArray(input)) {
    for (const value of input) pathsFromStructuredInput(value, into);
    return;
  }
  if (!isRecord(input)) return;
  for (const value of Object.values(input)) pathsFromStructuredInput(value, into);
}

/** Every absolute file path a tool call's input mentions, newest last. */
export function toolCallReferencePaths(input: unknown): string[] {
  const paths: string[] = [];
  pathsFromStructuredInput(input, paths);
  return paths;
}

// Per-call memo. Both scans are regex-heavy and the transcript is rebuilt on
// every streaming delta, so re-deriving history each frame is pure waste; the
// reducer keeps unchanged items object-identical (the same property the
// transcript-slot reuse relies on), which makes the item a sound cache key.
const cwdByToolCall = new WeakMap<ToolCallItem, string | null>();
const pathsByToolCall = new WeakMap<ToolCallItem, readonly string[]>();

function cachedReferenceCwd(item: ToolCallItem): string | null {
  const cached = cwdByToolCall.get(item);
  if (cached !== undefined) return cached;
  const value = toolCallReferenceCwd(item.input);
  cwdByToolCall.set(item, value);
  return value;
}

function cachedReferencePaths(item: ToolCallItem): readonly string[] {
  const cached = pathsByToolCall.get(item);
  if (cached) return cached;
  const value = toolCallReferencePaths(item.input);
  pathsByToolCall.set(item, value);
  return value;
}

/**
 * A tool call gated behind an approval that was denied, cancelled, or is
 * still undecided never executed, so its working directory and the paths in
 * its arguments describe nothing on disk. A call that ran and *failed* stays
 * in: it may well have created the file before erroring, and every candidate
 * it contributes is stat-verified before use anyway.
 */
function neverRanRequestIds(
  orderedItems: readonly ChatViewItem[],
): ReadonlySet<string> {
  const ids = new Set<string>();
  for (const item of orderedItems) {
    if (item.kind !== "permission_request") continue;
    const { resolution } = item;
    if (resolution.state === "resolved") {
      const { decision } = resolution.decision;
      if (decision === "deny" || decision === "cancel") ids.add(item.request_id);
      continue;
    }
    // Pending / responding / failed: the call has not been let through.
    ids.add(item.request_id);
  }
  return ids;
}

function ranAsGated(item: ToolCallItem, blocked: ReadonlySet<string>): boolean {
  return !item.approval_request_id || !blocked.has(item.approval_request_id);
}

/**
 * Associate assistant prose with the most recent explicit tool working
 * directory in the same turn. A Home workspace can run tools in any project;
 * relative paths in the resulting summary belong to that project, not Home.
 *
 * `previous` is the last result: when nothing changed it is returned as-is so
 * the map keeps its object identity across a streaming delta and the
 * memoized transcript rows can skip (issue #129).
 */
export function assistantReferenceCwds(
  orderedItems: readonly ChatViewItem[],
  previous?: ReadonlyMap<string, string>,
): ReadonlyMap<string, string> {
  const byMessageId = new Map<string, string>();
  const blocked = neverRanRequestIds(orderedItems);
  let turnCwd: string | null = null;

  for (const item of orderedItems) {
    if (item.kind === "user_message") {
      turnCwd = null;
      continue;
    }
    if (item.kind === "tool_call") {
      if (ranAsGated(item, blocked)) {
        turnCwd = cachedReferenceCwd(item) ?? turnCwd;
      }
      continue;
    }
    if (item.kind === "assistant_message" && turnCwd) {
      byMessageId.set(item.id, turnCwd);
      continue;
    }
    if (item.kind === "turn_ended") turnCwd = null;
  }

  if (previous && previous.size === byMessageId.size) {
    let identical = true;
    for (const [id, cwd] of byMessageId) {
      if (previous.get(id) !== cwd) {
        identical = false;
        break;
      }
    }
    if (identical) return previous;
  }
  return byMessageId;
}

/**
 * Associate assistant prose with every absolute file path the same turn's
 * tool calls touched. When a chip's resolved path does not exist on disk
 * (the agent linked a bare filename for a file living elsewhere — commonly
 * a screenshot written to a temp directory), a basename match against these
 * is the fallback candidate set. Stat-verified before use, never trusted.
 */
export function assistantReferencePaths(
  orderedItems: readonly ChatViewItem[],
  previous?: ReadonlyMap<string, readonly string[]>,
): ReadonlyMap<string, readonly string[]> {
  const byMessageId = new Map<string, readonly string[]>();
  const blocked = neverRanRequestIds(orderedItems);
  let turnPaths: string[] = [];
  let allReused = true;

  for (const item of orderedItems) {
    if (item.kind === "user_message" || item.kind === "turn_ended") {
      turnPaths = [];
      continue;
    }
    if (item.kind === "tool_call") {
      if (!ranAsGated(item, blocked)) continue;
      for (const path of cachedReferencePaths(item)) {
        rememberPath(path, turnPaths);
      }
      continue;
    }
    if (item.kind === "assistant_message" && turnPaths.length > 0) {
      // Reuse the previous array whenever the contents match: the prop
      // identity is what lets the memoized row skip re-rendering.
      const prior = previous?.get(item.id);
      if (prior && sameStrings(prior, turnPaths)) {
        byMessageId.set(item.id, prior);
      } else {
        byMessageId.set(item.id, [...turnPaths]);
        allReused = false;
      }
    }
  }

  if (allReused && previous && previous.size === byMessageId.size) {
    return previous;
  }
  return byMessageId;
}

function sameStrings(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((value, index) => value === b[index]);
}
