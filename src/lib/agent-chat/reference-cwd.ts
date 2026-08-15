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
