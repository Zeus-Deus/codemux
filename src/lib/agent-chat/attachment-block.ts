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
  GitHubIssue,
  PullRequestInfo,
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

/** Stage 4 — build the resolved-content body for an issue chip from
 *  the detail-fetch payload. Includes state, URL, body (already
 *  truncated to 50KB by the backend), and up to 20 comments with a
 *  trailing "N more not shown" note when the thread is longer. The
 *  agent gets enough context to triage without us blowing out the
 *  prompt budget on a 200-comment monster thread. */
export function buildIssueResolvedContent(detail: GitHubIssue): string {
  const parts: string[] = [];
  parts.push(`State: ${detail.state}`);
  parts.push(`URL: ${detail.url}`);
  if (detail.labels.length > 0) {
    parts.push(`Labels: ${detail.labels.join(", ")}`);
  }
  if (detail.assignees.length > 0) {
    parts.push(`Assignees: ${detail.assignees.join(", ")}`);
  }
  parts.push("");
  parts.push("### Body");
  parts.push(detail.body && detail.body.length > 0 ? detail.body : "(no body)");

  if (detail.comments.length > 0) {
    parts.push("");
    parts.push(
      `### Comments (${detail.totalComments} total, showing first ${detail.comments.length})`,
    );
    for (const comment of detail.comments) {
      parts.push("");
      parts.push(`**${comment.author}** (${comment.createdAt}):`);
      parts.push(comment.body);
    }
    if (detail.totalComments > detail.comments.length) {
      const remaining = detail.totalComments - detail.comments.length;
      parts.push("");
      parts.push(
        `_…${remaining} more comment${remaining === 1 ? "" : "s"} not shown. Use \`gh issue view ${detail.number} --comments\` for the full thread._`,
      );
    }
  }

  return parts.join("\n");
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

/** Stage 5 — build the resolved-content body for a PR chip from the
 *  detail-fetch payload + diff. Mirrors `buildIssueResolvedContent`'s
 *  shape (state, URL, body, comments) but layers in branch metadata
 *  (`base ← head`, additions/deletions, mergeable/review state) and
 *  the diff (name-only by default, full when the caller opts in via
 *  the second argument). The `diff` parameter is whatever the gh
 *  command returned — already truncated to 100 KB on the Rust side
 *  if the full unified diff would have blown the budget. */
export function buildPrResolvedContent(
  detail: PullRequestInfo,
  diff: string,
  opts: { fullDiff?: boolean } = {},
): string {
  const parts: string[] = [];
  parts.push(`State: ${detail.state}${detail.is_draft ? " (draft)" : ""}`);
  parts.push(`URL: ${detail.url}`);
  if (detail.author) parts.push(`Author: ${detail.author}`);
  if (detail.head_branch && detail.base_branch) {
    parts.push(`Branches: ${detail.base_branch} ← ${detail.head_branch}`);
  } else if (detail.head_branch) {
    parts.push(`Head branch: ${detail.head_branch}`);
  }
  const stats: string[] = [];
  if (typeof detail.additions === "number") {
    stats.push(`+${detail.additions}`);
  }
  if (typeof detail.deletions === "number") {
    stats.push(`-${detail.deletions}`);
  }
  if (stats.length > 0) parts.push(`Diff stat: ${stats.join(" / ")}`);
  if (detail.review_decision) {
    parts.push(`Review: ${detail.review_decision}`);
  }
  if (detail.mergeable) parts.push(`Mergeable: ${detail.mergeable}`);
  parts.push("");
  parts.push("### Body");
  parts.push(detail.body && detail.body.length > 0 ? detail.body : "(no body)");

  if (diff && diff.trim().length > 0) {
    parts.push("");
    if (opts.fullDiff) {
      parts.push("### Diff (full)");
      parts.push("```diff");
      parts.push(diff);
      parts.push("```");
    } else {
      // `--name-only` output is one path per line. Render as a
      // bulleted list so the agent reads it as files-changed rather
      // than a half-broken diff.
      const files = diff
        .split("\n")
        .map((s) => s.trim())
        .filter((s) => s.length > 0);
      parts.push(`### Files changed (${files.length})`);
      for (const f of files) parts.push(`- ${f}`);
      parts.push("");
      parts.push(
        `_Use \`gh pr diff ${detail.number}\` for the full unified diff._`,
      );
    }
  }

  if (detail.comments.length > 0) {
    parts.push("");
    parts.push(
      `### Comments (${detail.totalComments} total, showing first ${detail.comments.length})`,
    );
    for (const comment of detail.comments) {
      parts.push("");
      parts.push(`**${comment.author}** (${comment.createdAt}):`);
      parts.push(comment.body);
    }
    if (detail.totalComments > detail.comments.length) {
      const remaining = detail.totalComments - detail.comments.length;
      parts.push("");
      parts.push(
        `_…${remaining} more comment${remaining === 1 ? "" : "s"} not shown. Use \`gh pr view ${detail.number} --comments\` for the full thread._`,
      );
    }
  }

  return parts.join("\n");
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

/**
 * Stage 6 — extract resolved image attachments into the wire shape
 * `agent_chat_send_turn` expects (`{ data: number[], media_type:
 * string }`). Skips chips that are still loading or whose read failed
 * so a half-resolved image never lands on the SDK in a malformed
 * state. Returns `[]` when no images are staged so callers can pass
 * the result through without an `?? []`.
 */
export function buildImagePayloads(
  attachments: Attachment[],
): Array<{ data: number[]; media_type: string }> {
  return attachments
    .filter(
      (a): a is Attachment & { resolvedImage: { mime: string; bytes: Uint8Array } } =>
        a.kind === "image" && !!a.resolvedImage,
    )
    .map((a) => ({
      data: Array.from(a.resolvedImage.bytes),
      media_type: a.resolvedImage.mime,
    }));
}

/**
 * Base64-encode raw bytes for use in a `data:` URL.
 *
 * `btoa` needs a binary string, and the obvious
 * `String.fromCharCode(...bytes)` spread passes every byte as its own
 * argument — which overflows the call stack for anything but a tiny
 * image (JS engines cap argument counts in the tens of thousands). We
 * encode in fixed 32 KB windows so the per-call argument count stays
 * bounded regardless of image size.
 */
function bytesToBase64(bytes: Uint8Array): string {
  const CHUNK = 0x8000; // 32 KB — comfortably under engine arg limits.
  let binary = "";
  for (let i = 0; i < bytes.length; i += CHUNK) {
    const window = bytes.subarray(i, i + CHUNK);
    binary += String.fromCharCode(...window);
  }
  return btoa(binary);
}

/**
 * Sibling to `buildImagePayloads`: instead of the wire shape for the
 * SDK, produce the display shape the user-message bubble renders
 * (`{ src, mediaType }`). Each staged image becomes a self-contained
 * `data:` URL so the optimistically-appended bubble can show the
 * thumbnail immediately — before the turn round-trips and the backend
 * persists the bytes to disk.
 *
 * Filters to resolved images exactly like `buildImagePayloads` so a
 * still-loading chip never renders as a broken thumbnail, and returns
 * `[]` when nothing is staged so callers can pass it straight into the
 * optimistic append.
 */
export function buildImageDisplaySources(
  attachments: Attachment[],
): Array<{ src: string; mediaType: string }> {
  return attachments
    .filter(
      (a): a is Attachment & { resolvedImage: { mime: string; bytes: Uint8Array } } =>
        a.kind === "image" && !!a.resolvedImage,
    )
    .map((a) => ({
      src: `data:${a.resolvedImage.mime};base64,${bytesToBase64(a.resolvedImage.bytes)}`,
      mediaType: a.resolvedImage.mime,
    }));
}
