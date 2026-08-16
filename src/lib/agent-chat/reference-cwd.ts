import type { ChatViewItem } from "./types";

const POSIX_ABSOLUTE = /^\//;
const WINDOWS_ABSOLUTE = /^[a-zA-Z]:[\\/]/;
const DIRECTORY_KEYS = new Set([
  "cwd",
  "workdir",
  "workingDirectory",
  "working_directory",
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

function directoryFromStructuredInput(input: unknown): string | null {
  if (typeof input === "string") return directoryFromFreeformInput(input);
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
  for (const value of Object.values(input).reverse()) {
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

export function toolCallReferenceCwd(input: unknown): string | null {
  return directoryFromStructuredInput(input) ?? directoryFromFreeformInput(input);
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

/**
 * Associate assistant prose with the most recent explicit tool working
 * directory in the same turn. A Home workspace can run tools in any project;
 * relative paths in the resulting summary belong to that project, not Home.
 */
export function assistantReferenceCwds(
  orderedItems: readonly ChatViewItem[],
): ReadonlyMap<string, string> {
  const byMessageId = new Map<string, string>();
  let turnCwd: string | null = null;

  for (const item of orderedItems) {
    if (item.kind === "user_message") {
      turnCwd = null;
      continue;
    }
    if (item.kind === "tool_call") {
      turnCwd = toolCallReferenceCwd(item.input) ?? turnCwd;
      continue;
    }
    if (item.kind === "assistant_message" && turnCwd) {
      byMessageId.set(item.id, turnCwd);
      continue;
    }
    if (item.kind === "turn_ended") turnCwd = null;
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
): ReadonlyMap<string, readonly string[]> {
  const byMessageId = new Map<string, readonly string[]>();
  let turnPaths: string[] = [];

  for (const item of orderedItems) {
    if (item.kind === "user_message" || item.kind === "turn_ended") {
      turnPaths = [];
      continue;
    }
    if (item.kind === "tool_call") {
      for (const path of toolCallReferencePaths(item.input)) {
        rememberPath(path, turnPaths);
      }
      continue;
    }
    if (item.kind === "assistant_message" && turnPaths.length > 0) {
      byMessageId.set(item.id, [...turnPaths]);
    }
  }

  return byMessageId;
}
