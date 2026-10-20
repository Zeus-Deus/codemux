import { parseDiff } from "@/lib/diff-parser";

import type { ToolCallItem } from "./types";

/**
 * Presentation view of Codex's native tool items.
 *
 * Codex reports tool work as app-server item envelopes: the call's
 * `tool_name` is the item type (`commandExecution`, `fileChange`,
 * `mcpToolCall`, `dynamicToolCall`) and its `input` is the whole item, so
 * the command, the edited files or the real tool name sit one level down.
 * This module reads those envelopes for the labels, icons, activity rows and
 * bodies. It never rewrites the stored item: `tool_name`, `input` and
 * `result_content` stay the provider's own, so raw payload access, approvals
 * and the special cards that parse envelopes themselves are unaffected.
 *
 * Pure and side-effect-free.
 */

export type NativeFileChangeKind = "add" | "delete" | "update";

export interface NativeFileChange {
  path: string;
  kind: NativeFileChangeKind;
  /** Rename target of an update, when Codex moved the file. */
  movePath: string | null;
  /** Unified diff for an update; the whole file content for an add or
   *  delete. */
  diff: string;
  added: number;
  removed: number;
}

export type NativeToolCall =
  | {
      kind: "command";
      command: string;
      /** Captured output once settled: the plain stdout string, or the
       *  `aggregatedOutput` of a settled envelope. */
      output: string | null;
      /** Exit code, when a settled envelope reported one. */
      exitCode: number | null;
    }
  | { kind: "file_change"; changes: NativeFileChange[] }
  | {
      kind: "tool";
      /** The tool the model actually called, e.g.
       *  `codemux_mcp__codemux__browser_click` or `mcp__cua_repl__js`. */
      name: string;
      /** Parsed call arguments; `null` when absent or unparseable. */
      args: unknown;
      /** Result blocks once the call settled, else `null`. A string entry
       *  is preformatted text. */
      output: unknown[] | null;
      /** The provider's error message, when it reported one. */
      error: string | null;
    };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function nonEmptyString(value: unknown): string | null {
  return typeof value === "string" && value.length > 0 ? value : null;
}

// Tool-call items are copy-on-write, so identity caches the parse: activity
// rows re-describe every step on each streamed token.
const cache = new WeakMap<ToolCallItem, NativeToolCall | null>();

/** The native item behind a Codex tool call, or `null` for any other call
 *  (including an envelope too malformed to present faithfully). */
export function nativeToolCall(item: ToolCallItem): NativeToolCall | null {
  if (cache.has(item)) return cache.get(item) ?? null;
  const view = parseNativeToolCall(item);
  cache.set(item, view);
  return view;
}

function parseNativeToolCall(item: ToolCallItem): NativeToolCall | null {
  const input = item.input;
  // Only a genuine envelope: another provider's tool that happens to be
  // called `commandExecution` keeps its own rendering.
  if (!isRecord(input) || input.type !== item.tool_name) return null;
  switch (item.tool_name) {
    case "commandExecution": {
      const settled = envelopeOf(item.result_content, "commandExecution");
      const command = commandText(input.command) ?? commandText(settled?.command);
      if (!command) return null;
      const output = settled
        ? typeof settled.aggregatedOutput === "string"
          ? settled.aggregatedOutput
          : null
        : typeof item.result_content === "string"
          ? item.result_content
          : null;
      const exitCode = typeof settled?.exitCode === "number" ? settled.exitCode : null;
      return { kind: "command", command, output, exitCode };
    }
    case "fileChange": {
      if (!Array.isArray(input.changes)) return null;
      return { kind: "file_change", changes: nativeFileChanges(input.changes) };
    }
    case "mcpToolCall": {
      const tool = nonEmptyString(input.tool);
      if (!tool) return null;
      const server = nonEmptyString(input.server);
      const settled = envelopeOf(item.result_content, "mcpToolCall");
      const result = isRecord(settled?.result) ? settled.result : null;
      return {
        kind: "tool",
        // Codex names MCP tools for the model as `mcp__<server>__<tool>`.
        name: server ? `mcp__${server}__${tool}` : tool,
        args: parseArgs(input.arguments),
        output: settled ? mcpOutput(result) : passthroughOutput(item.result_content),
        error: errorMessage(settled?.error),
      };
    }
    case "dynamicToolCall": {
      const tool = nonEmptyString(input.tool);
      if (!tool) return null;
      const settled = envelopeOf(item.result_content, "dynamicToolCall");
      return {
        kind: "tool",
        name: tool,
        args: parseArgs(input.arguments),
        output: settled
          ? Array.isArray(settled.contentItems)
            ? settled.contentItems
            : null
          : passthroughOutput(item.result_content),
        error: null,
      };
    }
    default:
      return null;
  }
}

/**
 * The name and input the shared tool maps (verbs, icons, activity families)
 * should key on: a native command presents as `Bash`, a file change as
 * `Edit` (`Write` when every change creates a file), an MCP or dynamic call
 * as the tool the model called. Any other call passes through unchanged.
 */
export function toolPresentation(item: ToolCallItem): { name: string; input: unknown } {
  const native = nativeToolCall(item);
  if (!native) return { name: item.tool_name, input: item.input };
  switch (native.kind) {
    case "command":
      return { name: "Bash", input: { command: native.command } };
    case "file_change": {
      const first = native.changes[0];
      const allAdds = native.changes.length > 0 && native.changes.every((c) => c.kind === "add");
      return {
        name: allAdds ? "Write" : "Edit",
        input: first ? { file_path: first.path } : {},
      };
    }
    case "tool":
      return { name: native.name, input: native.args };
  }
}

/** Shorthand for callers that only key on the name. */
export function presentationToolName(item: ToolCallItem): string {
  return toolPresentation(item).name;
}

/** Normalise Codex `fileChange.changes` entries. An add or delete carries
 *  the whole file as `diff`, an update a unified diff. Entries without a
 *  path are skipped. */
export function nativeFileChanges(changes: readonly unknown[]): NativeFileChange[] {
  const out: NativeFileChange[] = [];
  for (const change of changes) {
    if (!isRecord(change)) continue;
    const path = nonEmptyString(change.path);
    if (!path) continue;
    const diff = typeof change.diff === "string" ? change.diff : "";
    const rawKind = isRecord(change.kind) ? change.kind.type : change.kind;
    const kind: NativeFileChangeKind =
      rawKind === "add" || rawKind === "delete" ? rawKind : "update";
    const movePath = isRecord(change.kind)
      ? nonEmptyString(change.kind.move_path) ?? nonEmptyString(change.kind.movePath)
      : null;
    let added = 0;
    let removed = 0;
    if (kind === "add") {
      added = lineCount(diff);
    } else if (kind === "delete") {
      removed = lineCount(diff);
    } else {
      // The shared parser only treats `---`/`+++` as file headers before
      // the first hunk; inside one they are content (a deleted `-- note`).
      for (const row of parseDiff(diff)) {
        if (row.type === "add") added += 1;
        else if (row.type === "del") removed += 1;
      }
    }
    out.push({ path, kind, movePath, diff, added, removed });
  }
  return out;
}

function lineCount(text: string): number {
  if (text.length === 0) return 0;
  const lines = text.split("\n");
  return lines[lines.length - 1] === "" ? lines.length - 1 : lines.length;
}

/** The settled item envelope Codex sends as a tool's result, if it is one. */
function envelopeOf(content: unknown, type: string): Record<string, unknown> | null {
  return isRecord(content) && content.type === type ? content : null;
}

/** A result that is not an envelope (a plain string, say) still renders. */
function passthroughOutput(content: unknown): unknown[] | null {
  if (content == null) return null;
  return Array.isArray(content) ? content : [content];
}

function mcpOutput(result: Record<string, unknown> | null): unknown[] | null {
  if (!result) return null;
  const content: unknown[] = Array.isArray(result.content) ? result.content : [];
  const structured = result.structuredContent;
  // MCP servers usually echo structured results as a JSON text block too;
  // show the structured value only when no block already carries it. It is
  // arbitrary data, not a content block, so it goes in as JSON text.
  if (structured != null && !content.some((block) => echoesJson(block, structured))) {
    return [...content, JSON.stringify(structured, null, 2)];
  }
  return Array.isArray(result.content) ? content : null;
}

function echoesJson(block: unknown, value: unknown): boolean {
  if (!isRecord(block) || typeof block.text !== "string") return false;
  try {
    return JSON.stringify(JSON.parse(block.text)) === JSON.stringify(value);
  } catch {
    return false;
  }
}

function errorMessage(error: unknown): string | null {
  if (typeof error === "string") return error || null;
  return isRecord(error) ? nonEmptyString(error.message) : null;
}

function parseArgs(args: unknown): unknown {
  if (typeof args !== "string") return args ?? null;
  try {
    return JSON.parse(args);
  } catch {
    return args;
  }
}

// `/usr/bin/bash -lc '<script>'`: Codex runs every command through a login
// shell and reports the joined argv. The script is what the model wrote.
const SHELL_WRAPPER = /^(?:\S*[\\/])?(?:bash|zsh|sh)(?:\.exe)? -l?c (.+)$/s;
const SHELLS = /^(?:\S*[\\/])?(?:bash|zsh|sh)(?:\.exe)?$/;

function commandText(command: unknown): string | null {
  if (Array.isArray(command)) {
    const argv = command.filter((part): part is string => typeof part === "string");
    if (argv.length === 3 && SHELLS.test(argv[0] ?? "") && /^-l?c$/.test(argv[1] ?? "")) {
      return nonEmptyString(argv[2]);
    }
    return nonEmptyString(argv.join(" "));
  }
  const text = nonEmptyString(command);
  if (!text) return null;
  const wrapped = SHELL_WRAPPER.exec(text);
  if (!wrapped) return text;
  return shellWord(wrapped[1] ?? "") ?? text;
}

/**
 * Decode one POSIX shell word (`'a'"'"'b'`, `"x \"y\""`, `a\ b`), or `null`
 * when the text is not exactly one word. A command we can't decode faithfully
 * keeps its wrapper rather than showing a mangled script.
 */
function shellWord(text: string): string | null {
  let out = "";
  let i = 0;
  while (i < text.length) {
    const ch = text[i];
    if (ch === "'") {
      const end = text.indexOf("'", i + 1);
      if (end < 0) return null;
      out += text.slice(i + 1, end);
      i = end + 1;
    } else if (ch === '"') {
      i += 1;
      let closed = false;
      while (i < text.length) {
        const c = text[i];
        if (c === '"') {
          closed = true;
          i += 1;
          break;
        }
        if (c === "\\" && i + 1 < text.length && '"\\$`\n'.includes(text[i + 1] ?? "")) {
          if (text[i + 1] !== "\n") out += text[i + 1];
          i += 2;
          continue;
        }
        out += c;
        i += 1;
      }
      if (!closed) return null;
    } else if (ch === "\\") {
      if (i + 1 >= text.length) return null;
      out += text[i + 1];
      i += 2;
    } else if (" \t\n;&|<>()`$".includes(ch ?? "")) {
      // A second word, an operator or an expansion: not one literal word.
      return null;
    } else {
      out += ch;
      i += 1;
    }
  }
  return out.length > 0 ? out : null;
}
