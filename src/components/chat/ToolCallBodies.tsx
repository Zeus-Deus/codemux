import { useState } from "react";

import {
  formatLazyBytes,
  lazyToolResultStub,
  toolResultContentFromPayload,
  type LazyToolResultStub,
} from "@/lib/agent-chat/lazy-tool-result";
import type { ToolCallItem } from "@/lib/agent-chat/types";
import { resolveChatFileLink } from "@/lib/agent-chat/file-links";
import {
  extractToolResultImages,
  isRenderableImageBlock,
} from "@/lib/agent-chat/tool-result-images";
import { cn } from "@/lib/utils";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import { agentChatGetToolResult } from "@/tauri/commands";

import { DiffView } from "./DiffView";
import { MessageCopyButton } from "./MessageCopyButton";
import { ToolCallBlock } from "./ToolCallBlock";
import { ToolResultImages } from "./ToolResultImages";
import { MarkdownFileLink } from "./MarkdownFileLink";
import { useChatFileLinkContext } from "./chat-file-link-context";

const BASH_TAIL_LINES = 10;
const READ_PREVIEW_LINES = 5;
const GREP_PREVIEW_MATCHES = 5;

/**
 * Dispatches per-tool body rendering. Falls back to the raw JSON dump
 * for unknown tools, matching pre-Stage-7 behavior — known tools get a
 * polished view that surfaces the most useful field at a glance.
 *
 * Bodies that trim their output (Bash tail, Read head, Grep matches) own a
 * local "show all" toggle in the trimmed block's footer, independent of the
 * card or row disclosure that hosts them.
 */
export function ToolCallBody({ item }: { item: ToolCallItem }) {
  const input = isRecord(item.input) ? item.input : null;
  // A hydrated oversized result ships as metadata; the body arrives when
  // the user actually asks for it.
  const stub = lazyToolResultStub(item.result_content);
  if (stub) return <LazyToolResultBody stub={stub} />;
  const body = renderToolBody(item, input);

  // Images returned in the tool result (a screenshot from `Read` on a
  // PNG, a browser/screenshot tool, …) render as thumbnails below the
  // per-tool body instead of being stringified into a base64 dump. The
  // per-tool `contentToString` skips image blocks, so the text body and
  // the image row never double-render the same payload.
  const images = extractToolResultImages(item.result_content);
  if (images.length === 0) return body;

  return (
    <div className="space-y-2.5">
      {body}
      <ToolResultImages images={images} />
    </div>
  );
}

function renderToolBody(
  item: ToolCallItem,
  input: Record<string, unknown> | null,
) {
  switch (item.tool_name) {
    case "Bash":
      return <BashToolBody item={item} input={input} />;
    case "Read":
      return <ReadToolBody item={item} input={input} />;
    case "Grep":
      return <GrepToolBody item={item} input={input} />;
    case "WebFetch":
      return <WebFetchToolBody item={item} input={input} />;
    case "Edit":
    case "MultiEdit":
    case "Write":
      return <EditToolBody item={item} input={input} />;
    default:
      return <GenericToolBody item={item} />;
  }
}

interface BodyProps {
  item: ToolCallItem;
  input: Record<string, unknown> | null;
}

export function BashToolBody({ item, input }: BodyProps) {
  const command = input ? stringField(input, "command") : null;
  const description = input ? stringField(input, "description") : null;
  const result = contentToString(item.result_content);
  const exitCode = parseExitCode(result, item.status);
  const [showAll, setShowAll] = useState(false);
  const totalLines = countLines(result);
  const tailHidden = totalLines - BASH_TAIL_LINES;

  return (
    <div className="space-y-2">
      {command && (
        <div className="group/copy code-surface relative select-text rounded-md bg-muted/40 px-3 py-2 text-foreground whitespace-pre-wrap break-words">
          <span className="text-muted-foreground/70">$ </span>
          {command}
          {description && (
            <div className="mt-1 font-sans text-label text-muted-foreground/80">
              {description}
            </div>
          )}
          <ToolCopyButton text={command} label="Copy command" />
        </div>
      )}
      {result && (
        <div className="group/copy relative rounded-md bg-muted/40">
          <pre
            className={cn(
              "code-surface whitespace-pre-wrap break-words px-3 py-2 text-foreground",
              showAll && FULL_OUTPUT_CLASS,
            )}
          >
            {showAll ? result : tailLines(result, BASH_TAIL_LINES)}
          </pre>
          <ToolCopyButton text={result} label="Copy output" />
          {tailHidden > 0 && (
            <ShowAllButton
              expanded={showAll}
              onToggle={() => setShowAll((v) => !v)}
              showLabel={`Show all ${totalLines} lines`}
              hideLabel={`Show last ${BASH_TAIL_LINES} lines`}
            />
          )}
        </div>
      )}
      {exitCode !== null && exitCode !== 0 && (
        // Per chat-ui skill: errors inside a tool result may use
        // --danger on the label itself, but the surrounding block
        // stays neutral. Drop the filled chip and keep just the
        // monospace text so the marker reads as inline metadata,
        // not a status billboard.
        <span
          aria-label={`Exit code ${exitCode}`}
          className="font-mono text-label text-destructive/80"
        >
          exit {exitCode}
        </span>
      )}
    </div>
  );
}

export function ReadToolBody({ item, input }: BodyProps) {
  const path = input
    ? stringField(input, "file_path") ?? stringField(input, "path")
    : null;
  const offset = input ? numberField(input, "offset") : null;
  const limit = input ? numberField(input, "limit") : null;
  const result = contentToString(item.result_content);
  const totalLines = countLines(result);
  const [showAll, setShowAll] = useState(false);
  const preview = result
    ? showAll
      ? result
      : headLines(result, READ_PREVIEW_LINES)
    : null;

  return (
    <div className="space-y-2">
      {path && (
        <div className="select-text font-mono text-body-sm text-foreground">
          <ToolSourcePath path={path} line={offset ?? undefined} />
          {(offset != null || limit != null) && (
            <span className="ml-2 text-muted-foreground/70">
              {rangeLabel(offset, limit)}
            </span>
          )}
        </div>
      )}
      {preview && (
        <div className="group/copy relative rounded-md bg-muted/40">
          <pre
            className={cn(
              "code-surface whitespace-pre-wrap break-words px-3 py-2 text-foreground",
              showAll && FULL_OUTPUT_CLASS,
            )}
          >
            {preview}
          </pre>
          <ToolCopyButton text={result} label="Copy output" />
          {totalLines > READ_PREVIEW_LINES && (
            <ShowAllButton
              expanded={showAll}
              onToggle={() => setShowAll((v) => !v)}
              showLabel={`Show all ${totalLines} lines`}
              hideLabel={`Show first ${READ_PREVIEW_LINES} lines`}
            />
          )}
        </div>
      )}
    </div>
  );
}

export function GrepToolBody({ item, input }: BodyProps) {
  const pattern = input ? stringField(input, "pattern") : null;
  const path = input ? stringField(input, "path") : null;
  const result = contentToString(item.result_content);
  const matches = parseGrepMatches(result);
  const [showAll, setShowAll] = useState(false);
  const hidden = Math.max(0, matches.length - GREP_PREVIEW_MATCHES);
  const visible = showAll ? matches : matches.slice(0, GREP_PREVIEW_MATCHES);

  return (
    <div className="space-y-2">
      {(pattern || path) && (
        <div className="select-text font-mono text-body-sm text-foreground">
          {pattern && <span>{pattern}</span>}
          {path && (
            <span className="ml-2 text-muted-foreground/70">
              in <ToolSourcePath path={path} />
            </span>
          )}
        </div>
      )}
      {matches.length > 0 ? (
        <div className="select-text rounded-md bg-muted/40">
          <div className={cn("space-y-1 px-3 py-2", showAll && FULL_OUTPUT_CLASS)}>
            <p className="text-label text-muted-foreground/80">
              {matches.length} match{matches.length === 1 ? "" : "es"}
            </p>
            <ul className="space-y-0.5">
              {visible.map((m, i) => (
                <li
                  key={i}
                  className="font-mono text-body-sm leading-5 text-foreground break-words"
                >
                  <span className="text-muted-foreground/70">
                    <ToolSourcePath path={m.location} />
                  </span>
                  {m.text && <span className="ml-2">{m.text}</span>}
                </li>
              ))}
            </ul>
          </div>
          {hidden > 0 && (
            <ShowAllButton
              expanded={showAll}
              onToggle={() => setShowAll((v) => !v)}
              showLabel={`Show ${hidden} more match${hidden === 1 ? "" : "es"}`}
              hideLabel={`Show first ${GREP_PREVIEW_MATCHES} matches`}
            />
          )}
        </div>
      ) : (
        result && <ToolCallBlock content={result} />
      )}
    </div>
  );
}

export function WebFetchToolBody({ item, input }: BodyProps) {
  const url = input ? stringField(input, "url") : null;
  const prompt = input ? stringField(input, "prompt") : null;
  const result = contentToString(item.result_content);
  const title = result ? extractTitle(result) : null;

  return (
    <div className="space-y-2">
      {url && (
        isSafeHttpUrl(url) ? (
          <a
            href={url}
            target="_blank"
            rel="noopener noreferrer"
            className="block break-all font-mono text-body-sm text-foreground underline-offset-4 hover:underline"
          >
            {url}
          </a>
        ) : (
          // Refuse to render non-http(s) URLs as links — `javascript:`,
          // `data:`, `file:`, etc. from a buggy or malicious tool result
          // are XSS vectors otherwise.
          <span className="block break-all font-mono text-body-sm text-muted-foreground">
            {url}
          </span>
        )
      )}
      {title && (
        <p className="select-text text-body-sm text-muted-foreground">{title}</p>
      )}
      {prompt && (
        <p className="select-text rounded-md bg-muted/40 px-3 py-2 text-body-sm text-foreground">
          {prompt}
        </p>
      )}
    </div>
  );
}

/**
 * Allowlist `http:` and `https:` only. The `URL` constructor itself is
 * lenient and parses `javascript:alert(1)` successfully, so we must
 * inspect `protocol` afterwards.
 */
function isSafeHttpUrl(raw: string): boolean {
  try {
    const u = new URL(raw);
    return u.protocol === "http:" || u.protocol === "https:";
  } catch {
    return false;
  }
}

export function EditToolBody({ item, input }: BodyProps) {
  const diff = editDiffInput(item, input);
  if (diff) {
    // Real diff surface computed from the tool input (design D7).
    return (
      <DiffView
        filename={diff.filename}
        oldText={diff.oldText}
        newText={diff.newText}
        copyText={diff.copyText}
      />
    );
  }

  // Fallback: no diff-able input yet (e.g. running with only a path, or a
  // result-only payload). Show the path and any raw result so nothing is
  // dropped.
  const path = input
    ? stringField(input, "file_path") ?? stringField(input, "path")
    : null;
  const result = contentToString(item.result_content);
  return (
    <div className="space-y-1">
      {path && (
        <div className="select-text font-mono text-body-sm text-foreground">
          <ToolSourcePath path={path} />
        </div>
      )}
      {result && <ToolCallBlock content={result} />}
    </div>
  );
}

interface EditDiffInput {
  filename: string | null;
  oldText: string;
  newText: string;
  copyText: string;
}

/** Extract the before/after text a diff card renders from an
 *  Edit/MultiEdit/Write tool input. Returns `null` when the input carries
 *  no diff-able fields (so the caller can fall back to path/result). */
function editDiffInput(
  item: ToolCallItem,
  input: Record<string, unknown> | null,
): EditDiffInput | null {
  if (!input) return null;
  const filename = stringField(input, "file_path") ?? stringField(input, "path");

  switch (item.tool_name) {
    case "Write": {
      const content = strOrNull(input.content) ?? strOrNull(input.contents);
      if (content == null) return null;
      return { filename, oldText: "", newText: content, copyText: content };
    }
    case "MultiEdit": {
      const edits = Array.isArray(input.edits) ? input.edits : [];
      const olds: string[] = [];
      const news: string[] = [];
      for (const edit of edits) {
        if (!isRecord(edit)) continue;
        const o = strOrNull(edit.old_string);
        const nw = strOrNull(edit.new_string);
        if (o != null) olds.push(o);
        if (nw != null) news.push(nw);
      }
      if (olds.length === 0 && news.length === 0) return null;
      const newText = news.join("\n");
      return { filename, oldText: olds.join("\n"), newText, copyText: newText };
    }
    default: {
      // Edit (and any single-hunk variant).
      const oldText = strOrNull(input.old_string);
      const newText = strOrNull(input.new_string);
      if (oldText == null && newText == null) return null;
      return {
        filename,
        oldText: oldText ?? "",
        newText: newText ?? "",
        copyText: newText ?? "",
      };
    }
  }
}

/**
 * Collapsed view of a tool result whose body stayed on disk. Renders the
 * preview the backend shipped with the stub — visually the same recessed
 * block a truncated body gets — and fetches the real content on demand.
 * A resolved fetch swaps the content into the store item, so the next
 * render takes the normal per-tool path.
 */
function LazyToolResultBody({ stub }: { stub: LazyToolResultStub }) {
  const [loading, setLoading] = useState(false);
  const [failed, setFailed] = useState(false);

  const load = () => {
    setLoading(true);
    setFailed(false);
    agentChatGetToolResult(stub.row_id)
      .then((payload) => {
        const content = toolResultContentFromPayload(payload);
        if (content == null) {
          setFailed(true);
          return;
        }
        useAgentChatStore.getState().resolveLazyToolResult(stub.row_id, content);
      })
      .catch((err) => {
        console.warn("[agent-chat] tool-result fetch failed:", err);
        setFailed(true);
      })
      .finally(() => setLoading(false));
  };

  const hidden = Math.max(stub.line_count - previewLineCount(stub.preview), 0);

  return (
    <div className="code-surface select-text rounded-md bg-muted/40 text-foreground whitespace-pre-wrap break-words">
      <div className="px-3 py-2">
        {stub.preview}
        <span className="text-muted-foreground/60">{"\n"}…</span>
      </div>
      <button
        type="button"
        onClick={load}
        disabled={loading}
        className="block w-full border-t border-border/40 px-3 py-1.5 text-left text-label text-muted-foreground hover:text-foreground disabled:opacity-60"
      >
        {loading
          ? "Loading…"
          : failed
            ? "Couldn't load the full output — retry"
            : `Show ${hidden} more line${hidden === 1 ? "" : "s"} (${formatLazyBytes(stub.bytes)})`}
      </button>
    </div>
  );
}

/** A trimmed block opened in full scrolls inside itself instead of
 *  stretching the transcript by thousands of lines. */
const FULL_OUTPUT_CLASS = "max-h-[60vh] overflow-auto thin-scrollbar";

/** Footer toggle for a trimmed output block, styled like the lazy-result
 *  "Show N more lines" footer. */
function ShowAllButton({
  expanded,
  onToggle,
  showLabel,
  hideLabel,
}: {
  expanded: boolean;
  onToggle: () => void;
  showLabel: string;
  hideLabel: string;
}) {
  return (
    <button
      type="button"
      aria-expanded={expanded}
      onClick={onToggle}
      className="block w-full border-t border-border/40 px-3 py-1.5 text-left text-label text-muted-foreground transition-colors duration-100 hover:text-foreground"
    >
      {expanded ? hideLabel : showLabel}
    </button>
  );
}

/** Copy chip pinned to the top-right of a command or output block,
 *  revealed when the pointer is over that block (`group/copy`). It is
 *  `select-none` so selecting the command by hand never picks up "Copy". */
function ToolCopyButton({ text, label }: { text: string; label: string }) {
  return (
    <MessageCopyButton
      text={text}
      label={label}
      className="absolute right-1.5 top-1.5 select-none bg-background/90 px-1 py-0.5 font-sans group-hover/copy:pointer-events-auto group-hover/copy:opacity-100"
    />
  );
}

function previewLineCount(preview: string): number {
  return preview ? preview.split("\n").length : 0;
}

function GenericToolBody({ item }: { item: ToolCallItem }) {
  const hasResult = item.result_content != null;
  if (hasResult) {
    return <ToolCallBlock content={item.result_content} error={item.status === "error"} />;
  }
  if (item.input != null) {
    return <ToolCallBlock content={null} text={safeStringify(item.input)} />;
  }
  return null;
}

function ToolSourcePath({ path, line }: { path: string; line?: number }) {
  const { cwd } = useChatFileLinkContext();
  const candidate = line != null ? `${path}:${line}` : path;
  const meta = resolveChatFileLink(candidate, cwd);
  if (!meta) return <>{path}</>;
  return (
    <MarkdownFileLink meta={meta}>
      {meta.basename}
      {meta.line ? ` · L${meta.line}` : ""}
    </MarkdownFileLink>
  );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function stringField(obj: Record<string, unknown>, key: string): string | null {
  const v = obj[key];
  return typeof v === "string" && v.length > 0 ? v : null;
}

/** Like `stringField` but preserves empty strings (a legitimate
 *  `old_string`/`new_string` for insert/delete-only diffs). */
function strOrNull(v: unknown): string | null {
  return typeof v === "string" ? v : null;
}

function numberField(obj: Record<string, unknown>, key: string): number | null {
  const v = obj[key];
  return typeof v === "number" ? v : null;
}

function rangeLabel(offset: number | null, limit: number | null): string {
  if (offset == null && limit == null) return "";
  const start = offset ?? 1;
  if (limit == null) return `L${start}+`;
  return `L${start}-${start + limit - 1}`;
}

function contentToString(content: unknown): string {
  if (content == null) return "";
  if (typeof content === "string") return content;
  if (Array.isArray(content)) {
    return content
      .filter((entry) => !isRenderableImageBlock(entry))
      .map((entry) => {
        if (entry == null) return "";
        if (typeof entry === "string") return entry;
        if (
          typeof entry === "object" &&
          "text" in entry &&
          typeof (entry as { text: unknown }).text === "string"
        ) {
          return (entry as { text: string }).text;
        }
        return JSON.stringify(entry);
      })
      .join("\n");
  }
  if (typeof content === "object") {
    const maybeText = (content as { text?: unknown }).text;
    if (typeof maybeText === "string") return maybeText;
    return JSON.stringify(content, null, 2);
  }
  return String(content);
}

function safeStringify(v: unknown): string {
  try {
    return JSON.stringify(v, null, 2);
  } catch {
    return String(v);
  }
}

function countLines(s: string): number {
  if (!s) return 0;
  return s.split("\n").length;
}

function tailLines(s: string, n: number): string {
  if (!s) return "";
  const lines = s.split("\n");
  if (lines.length <= n) return s;
  return lines.slice(lines.length - n).join("\n");
}

function headLines(s: string, n: number): string {
  if (!s) return "";
  const lines = s.split("\n");
  if (lines.length <= n) return s;
  return lines.slice(0, n).join("\n");
}

/**
 * Parse the trailing exit code that the SDK appends to Bash output
 * when the command failed. Format varies — current sidecar emits an
 * `exit code N` line near the end. Returns `null` when no signal is
 * present; callers decide whether to surface it.
 */
function parseExitCode(result: string, status: ToolCallItem["status"]): number | null {
  const match = result.match(/exit code[: ]+(\d+)/i);
  if (match) return Number.parseInt(match[1] ?? "", 10);
  if (status === "error") return -1;
  return null;
}

interface GrepMatch {
  location: string;
  text: string;
}

/**
 * The Grep tool surfaces matches as either lines of `path:line:content`
 * (when `output_mode: "content"`, the default) or path-only hits (when
 * `output_mode: "files_with_matches"`). We accept both — anything that
 * looks like `<path>:<lineno>:<rest>` becomes a structured match;
 * everything else is rendered as a path-only entry.
 */
function parseGrepMatches(result: string): GrepMatch[] {
  if (!result) return [];
  const lines = result.split("\n").filter((l) => l.trim().length > 0);
  const matches: GrepMatch[] = [];
  for (const line of lines) {
    const m = line.match(/^(.+?):(\d+):(.*)$/);
    if (m) {
      matches.push({ location: `${m[1]}:${m[2]}`, text: m[3]?.trim() ?? "" });
    } else if (!line.startsWith("Found")) {
      matches.push({ location: line, text: "" });
    }
  }
  return matches;
}

function extractTitle(result: string): string | null {
  const trimmed = result.trim();
  if (!trimmed) return null;
  const firstLine = trimmed.split("\n", 1)[0]?.trim() ?? "";
  if (!firstLine) return null;
  // Truncate hard so we don't render an entire fetched body when the
  // sidecar didn't surface a parsed title field.
  if (firstLine.length > 120) return firstLine.slice(0, 119) + "…";
  return firstLine;
}
