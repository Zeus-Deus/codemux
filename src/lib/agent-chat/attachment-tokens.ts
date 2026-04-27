// Inline `@filename` token parser + segment composer for the
// composer's mirror overlay.
//
// Step 8 Stage 2.1 — chips moved INSIDE the textarea rather than
// living in a strip above it. The model mirrors how skills already
// work (`/skill-name` parsed at send time): the textarea text is the
// source of truth for which attachments are active; the slice's
// `stagedAttachments` is a resolved-content cache keyed by basename.
// Removing the token from the textarea excludes that attachment from
// the next send without any extra chip-removal affordance.

import type { Attachment } from "@/stores/agent-chat-store";
import type { Skill } from "@/tauri/commands";

import { parseSkillTokens } from "./skill-tokens";

export interface AttachmentTokenMatch {
  /** Inclusive start offset, points at the `@`. */
  start: number;
  /** Exclusive end offset (one past the last char of the basename). */
  end: number;
  /** Literal token text, e.g. `@README.md`. */
  token: string;
  /** Bare basename, e.g. `README.md`. */
  basename: string;
  /** Resolved attachment from the slice. */
  attachment: Attachment;
}

// `@` must be at start-of-text or after whitespace; the name accepts
// the characters typically found in filenames (letters, digits, dots,
// dashes, underscores). Matching ends at the first non-name character
// or end-of-text, so trailing prose doesn't bleed into the token.
const FILE_TOKEN_RE = /(?<=^|\s)@([A-Za-z0-9._-]+)(?=[^A-Za-z0-9._-]|$)/g;

// `@#<number>` for GitHub issue refs (Stage 4). Kept as a separate
// regex from FILE_TOKEN_RE because the leading `#` is significant —
// reusing the file regex would either mis-match `@#21` as a literal
// path or silently allow `@21` (no hash) to dereference an issue,
// which the user never types. Trailing assertion `\D|$` stops the
// match at whitespace / punctuation / EOL so prose like `@#21,` or
// `@#21.` doesn't sweep the comma into the token text.
const ISSUE_TOKEN_RE = /(?<=^|\s)@#(\d+)(?=\D|$)/g;

// `@!<number>` for GitHub PR refs (Stage 5). One-char distinct from
// the issue token (`#` vs `!`) so the segmenter, the user reading
// their own draft, and the eventual model agent all have a stable
// shape-level signal of "this is a PR, not an issue". The `!` sigil
// follows the GitLab MR convention; on GitHub it's just a Codemux
// convention — the resolved-content injection makes the PR-vs-issue
// distinction explicit anyway.
const PR_TOKEN_RE = /(?<=^|\s)@!(\d+)(?=\D|$)/g;

/**
 * Find every `@<basename>` token in `text` whose basename matches a
 * staged file/folder attachment's `metadata.label`. Match order
 * follows source position. Image attachments are skipped — those live
 * out-of-text per Stage 6.
 *
 * Lookup is by `metadata.label` (the basename) to keep tokens compact
 * and human-readable. Two staged attachments with the same basename
 * but different paths is a known V1 limitation: last-staged wins on
 * the lookup. The popup's collision strategy (Stage 7 polish) will
 * disambiguate before this layer ever sees a duplicate.
 */
export function parseFileTokens(
  text: string,
  attachments: Attachment[],
): AttachmentTokenMatch[] {
  if (!text || attachments.length === 0) return [];

  const byLabel = new Map<string, Attachment>();
  for (const a of attachments) {
    if (a.kind !== "file" && a.kind !== "folder") continue;
    byLabel.set(a.metadata.label, a);
  }
  if (byLabel.size === 0) return [];

  const matches: AttachmentTokenMatch[] = [];
  FILE_TOKEN_RE.lastIndex = 0;
  let m: RegExpExecArray | null;
  while ((m = FILE_TOKEN_RE.exec(text)) !== null) {
    const basename = m[1];
    if (!basename) continue;
    const attachment = byLabel.get(basename);
    if (!attachment) continue;
    matches.push({
      start: m.index,
      end: m.index + m[0].length,
      token: m[0],
      basename,
      attachment,
    });
  }
  return matches;
}

/** Stage 5 — find every `@!<number>` token in `text` whose number
 *  matches a staged PR attachment's `ref` (e.g. `!21`). PR refs are
 *  matched against the literal `ref` for the same reason issue refs
 *  are: the user-facing label includes the title and would never
 *  match a textarea token. */
export function parsePrTokens(
  text: string,
  attachments: Attachment[],
): AttachmentTokenMatch[] {
  if (!text || attachments.length === 0) return [];

  const byRef = new Map<string, Attachment>();
  for (const a of attachments) {
    if (a.kind !== "pr") continue;
    byRef.set(a.ref, a);
  }
  if (byRef.size === 0) return [];

  const matches: AttachmentTokenMatch[] = [];
  PR_TOKEN_RE.lastIndex = 0;
  let m: RegExpExecArray | null;
  while ((m = PR_TOKEN_RE.exec(text)) !== null) {
    const number = m[1];
    if (!number) continue;
    const attachment = byRef.get(`!${number}`);
    if (!attachment) continue;
    matches.push({
      start: m.index,
      end: m.index + m[0].length,
      token: m[0],
      basename: `!${number}`,
      attachment,
    });
  }
  return matches;
}

/** Find every `@#<number>` token in `text` whose number matches a
 *  staged issue attachment's `ref` (e.g. `#21`). Issue refs are
 *  matched against the literal `ref` rather than the label because
 *  the user-facing label includes the title (`#21 Fix login bug`)
 *  while the inline token only carries the number — the label would
 *  never match a textarea token. */
export function parseIssueTokens(
  text: string,
  attachments: Attachment[],
): AttachmentTokenMatch[] {
  if (!text || attachments.length === 0) return [];

  const byRef = new Map<string, Attachment>();
  for (const a of attachments) {
    if (a.kind !== "issue") continue;
    byRef.set(a.ref, a);
  }
  if (byRef.size === 0) return [];

  const matches: AttachmentTokenMatch[] = [];
  ISSUE_TOKEN_RE.lastIndex = 0;
  let m: RegExpExecArray | null;
  while ((m = ISSUE_TOKEN_RE.exec(text)) !== null) {
    const number = m[1];
    if (!number) continue;
    const attachment = byRef.get(`#${number}`);
    if (!attachment) continue;
    matches.push({
      start: m.index,
      end: m.index + m[0].length,
      token: m[0],
      // For issue refs, `basename` is the bare `#N` so consumers can
      // treat it the same shape-wise as file tokens.
      basename: `#${number}`,
      attachment,
    });
  }
  return matches;
}

/**
 * Filter slice attachments to the subset whose `@<basename>` token
 * still appears in `text`. Used at send time so deleting a token
 * excludes the file from the prompt without needing an explicit
 * chip-removal action. Dedupes by attachment id — mentioning the
 * same file twice injects content once.
 *
 * Image attachments pass through unchanged because they don't have a
 * text representation; out-of-text attachments are always "active"
 * until cleared on send.
 */
export function activeAttachments(
  text: string,
  attachments: Attachment[],
): Attachment[] {
  if (attachments.length === 0) return attachments;
  // File/folder tokens (@<basename>), issue tokens (@#<n>), and PR
  // tokens (@!<n>) are collected together so the same
  // delete-token-to-exclude rule applies uniformly. Order follows
  // source position across all kinds.
  const fileTokens = parseFileTokens(text, attachments);
  const issueTokens = parseIssueTokens(text, attachments);
  const prTokens = parsePrTokens(text, attachments);
  const tokens = [...fileTokens, ...issueTokens, ...prTokens].sort(
    (a, b) => a.start - b.start,
  );
  const seenIds = new Set<string>();
  const out: Attachment[] = [];
  for (const t of tokens) {
    if (seenIds.has(t.attachment.id)) continue;
    seenIds.add(t.attachment.id);
    out.push(t.attachment);
  }
  // Image attachments are out-of-text and stay active regardless of
  // textarea content. Append them at the end so the file-token order
  // (source-position) is preserved for prose-eligible attachments.
  for (const a of attachments) {
    if (a.kind === "image" && !seenIds.has(a.id)) {
      seenIds.add(a.id);
      out.push(a);
    }
  }
  return out;
}

/**
 * Combined plain / skill / attachment segments for the composer's
 * mirror overlay. Mirrors `segmentForHighlight` from skill-tokens but
 * folds in `@<basename>` matches. Plain runs preserve original
 * characters byte-for-byte so cursor positioning stays in lockstep
 * with the underlying textarea.
 */
export type DraftHighlightSegment =
  | { kind: "plain"; text: string }
  | { kind: "skill"; text: string; name: string }
  | {
      kind: "attachment";
      text: string;
      basename: string;
      isLoading: boolean;
      hasError: boolean;
    }
  | {
      // Stage 4 — separate kind so the renderer can apply a state-
      // coloured pill (open=warning amber, closed=muted) without the
      // file/folder branch having to learn about issue state.
      kind: "issue-attachment";
      text: string;
      ref: string;
      state: "open" | "closed";
      isLoading: boolean;
      hasError: boolean;
    }
  | {
      // Stage 5 — PR pill carries 4 visual states: open / merged /
      // closed / draft. Mirror the chip strip's colour rules so the
      // staged chip and the inline token share a visual language.
      kind: "pr-attachment";
      text: string;
      ref: string;
      state: "open" | "merged" | "closed" | "draft";
      isLoading: boolean;
      hasError: boolean;
    };

interface RangeAnnotation {
  start: number;
  end: number;
  build: (text: string) => DraftHighlightSegment;
}

export function segmentDraftHighlight(
  text: string,
  skills: Skill[],
  attachments: Attachment[],
): DraftHighlightSegment[] {
  if (!text) return [];

  const annotations: RangeAnnotation[] = [];

  for (const m of parseSkillTokens(text, skills)) {
    annotations.push({
      start: m.start,
      end: m.end,
      build: (slice) => ({ kind: "skill", text: slice, name: m.name }),
    });
  }
  for (const m of parseFileTokens(text, attachments)) {
    const isLoading = m.attachment.metadata.isLoading === true;
    const hasError = typeof m.attachment.metadata.error === "string";
    annotations.push({
      start: m.start,
      end: m.end,
      build: (slice) => ({
        kind: "attachment",
        text: slice,
        basename: m.basename,
        isLoading,
        hasError,
      }),
    });
  }
  for (const m of parseIssueTokens(text, attachments)) {
    const isLoading = m.attachment.metadata.isLoading === true;
    const hasError = typeof m.attachment.metadata.error === "string";
    // Default to "open" when state is unset — staged issues always
    // carry state from the picker / mention pick, but this guards
    // against a bare addStagedAttachment call that forgot to seed it.
    const state: "open" | "closed" =
      m.attachment.metadata.state === "closed" ? "closed" : "open";
    annotations.push({
      start: m.start,
      end: m.end,
      build: (slice) => ({
        kind: "issue-attachment",
        text: slice,
        ref: m.basename,
        state,
        isLoading,
        hasError,
      }),
    });
  }
  for (const m of parsePrTokens(text, attachments)) {
    const isLoading = m.attachment.metadata.isLoading === true;
    const hasError = typeof m.attachment.metadata.error === "string";
    // PR state widens the issue palette — `metadata.state` carries
    // one of "open"/"merged"/"closed"/"draft". Default to "open"
    // (most common; mirrors the issue branch's safety net).
    const raw = m.attachment.metadata.state;
    const state: "open" | "merged" | "closed" | "draft" =
      raw === "merged" || raw === "closed" || raw === "draft"
        ? raw
        : "open";
    annotations.push({
      start: m.start,
      end: m.end,
      build: (slice) => ({
        kind: "pr-attachment",
        text: slice,
        ref: m.basename,
        state,
        isLoading,
        hasError,
      }),
    });
  }

  if (annotations.length === 0) {
    return [{ kind: "plain", text }];
  }

  // Source-position order. Tokens are non-overlapping by construction
  // (skill names match `[A-Za-z0-9_-]+`; attachment names allow `.`
  // additionally — a skill regex can never match an attachment-style
  // token because skills don't allow `.`).
  annotations.sort((a, b) => a.start - b.start);

  const out: DraftHighlightSegment[] = [];
  let cursor = 0;
  for (const ann of annotations) {
    if (ann.start > cursor) {
      out.push({ kind: "plain", text: text.slice(cursor, ann.start) });
    }
    out.push(ann.build(text.slice(ann.start, ann.end)));
    cursor = ann.end;
  }
  if (cursor < text.length) {
    out.push({ kind: "plain", text: text.slice(cursor) });
  }
  return out;
}
