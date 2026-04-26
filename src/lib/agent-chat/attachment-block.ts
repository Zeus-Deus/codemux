/**
 * Build the per-turn attachment block injected by Step 8 (Stage 2+).
 *
 * The block is plain text wedged into the prompt between the skill
 * body and the user's question (see `applyAllPrefixes`). For files,
 * the body is pre-rendered into `attachment.resolvedContent` at
 * attach-time (so we can also surface it in the chip preview); this
 * module wraps each resolved attachment with a short header so the
 * agent can see filename, path, and line count up front.
 *
 * Stage 2 implements `file` fully. `folder`, `issue`, `pr` stub here
 * for Stages 3–5 — they don't render until those stages light them
 * up. `image` is intentionally excluded (handled at the SDK content-
 * block layer in Stage 6, not as text).
 */

import type { Attachment } from "@/stores/agent-chat-store";
import type {
  FileAttachmentInfo,
  FolderAttachmentInfo,
} from "@/tauri/types";

/** Render a fenced full-content block for small files — agent sees
 *  the whole file. */
function renderFullFile(info: FileAttachmentInfo): string {
  const lang = info.language ?? "";
  return ["```" + lang, info.content, "```"].join("\n");
}

/** Render a truncated preview + outline for large files — the agent
 *  still gets a usable summary and an explicit pointer to the full
 *  path so it can `Read` more on demand. */
function renderTruncatedFile(info: FileAttachmentInfo): string {
  const lang = info.language ?? "";
  const previewLines = info.content.split("\n").slice(0, 50).join("\n");
  const lines: string[] = [
    `First 50 of ${info.lineCount} lines:`,
    "```" + lang,
    previewLines,
    "```",
  ];
  if (info.outline && info.outline.length > 0) {
    lines.push(`\nOutline (${info.outline.length} declarations):`);
    for (const entry of info.outline) {
      lines.push(`- ${entry.kind} ${entry.name} (line ${entry.line})`);
    }
  }
  lines.push(
    `\nUse the Read tool with path "${info.absolutePath}" to see more.`,
  );
  return lines.join("\n");
}

/**
 * Convert the backend-resolved file metadata into the body that lands
 * inside `attachment.resolvedContent`. The composer calls this after
 * `read_file_for_attachment` returns. Public so tests can hit it
 * directly without going through the React layer.
 */
export function buildFileResolvedContent(info: FileAttachmentInfo): string {
  if (!info.isText) {
    return "[binary file — use Read tool to access]";
  }
  return info.truncated ? renderTruncatedFile(info) : renderFullFile(info);
}

/**
 * Convert the backend-resolved folder metadata into the body that
 * lands inside `attachment.resolvedContent`. The composer calls this
 * after `read_folder_for_attachment` returns.
 *
 * Folder injection is intentionally tree-only: the agent gets the
 * directory shape and an explicit pointer to use Read/Grep for any
 * file content it actually needs. Avoids the "full folder content"
 * footgun that bloats prompts when a folder has dozens of files.
 */
export function buildFolderResolvedContent(info: FolderAttachmentInfo): string {
  return [
    `Tree (depth-bounded, ${info.itemCount} item${info.itemCount === 1 ? "" : "s"}):`,
    "```",
    info.tree,
    "```",
    "",
    `Use the Read or Grep tool with path "${info.absolutePath}" to explore further.`,
  ].join("\n");
}

function formatFileAttachment(att: Attachment): string {
  const header: string[] = [`## File: ${att.metadata.label}`];
  header.push(`Full path: ${att.ref}`);
  if (typeof att.metadata.lineCount === "number") {
    header.push(`Lines: ${att.metadata.lineCount}`);
  }
  return [...header, "", att.resolvedContent ?? ""].join("\n");
}

// Stub formatters for Stages 3–5. They're invoked when the matching
// kind has resolvedContent set; until each stage lights its kind up,
// nothing of that kind ever reaches this module so the stubs stay
// unreachable in practice.

function formatFolderAttachment(att: Attachment): string {
  return [`## Folder: ${att.metadata.label}`, `Path: ${att.ref}`, "", att.resolvedContent ?? ""].join(
    "\n",
  );
}

function formatIssueAttachment(att: Attachment): string {
  const stateLabel = att.metadata.state ? ` [${att.metadata.state}]` : "";
  return [
    `## Issue ${att.metadata.label}${stateLabel}`,
    "",
    att.resolvedContent ?? "",
  ].join("\n");
}

function formatPrAttachment(att: Attachment): string {
  const stateLabel = att.metadata.state ? ` [${att.metadata.state}]` : "";
  return [
    `## Pull Request ${att.metadata.label}${stateLabel}`,
    "",
    att.resolvedContent ?? "",
  ].join("\n");
}

function formatAttachment(att: Attachment): string {
  switch (att.kind) {
    case "file":
      return formatFileAttachment(att);
    case "folder":
      return formatFolderAttachment(att);
    case "issue":
      return formatIssueAttachment(att);
    case "pr":
      return formatPrAttachment(att);
    case "image":
      // Images are sent as content blocks at the SDK layer — never
      // formatted here, even if accidentally staged with a label.
      return "";
  }
}

/**
 * Wrap all resolved text attachments in a sentinel-fenced block so
 * the model can clearly identify "context the user attached" vs
 * "context the agent fetched on its own". Returns `null` when no
 * attachments are eligible — the send pipeline treats `null` as a
 * no-op, leaving the prompt unchanged.
 */
export function buildAttachmentBlock(
  attachments: Attachment[],
): string | null {
  const eligible = attachments.filter(
    (a) => a.kind !== "image" && a.resolvedContent && a.resolvedContent.length > 0,
  );
  if (eligible.length === 0) return null;
  const parts = eligible
    .map(formatAttachment)
    .filter((s) => s.length > 0);
  if (parts.length === 0) return null;
  return ["=== Attached context ===", ...parts, "=== End context ==="].join(
    "\n\n",
  );
}
