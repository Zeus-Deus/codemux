/**
 * Per-turn prompt wrappers for Cursor-style mode pills.
 *
 * Mirrors `ultrathink.ts`: each mode contributes a prefix that the
 * send-path composes onto the user's raw text. Stage 4 ships ASK;
 * Stage 6 fills DEBUG. Composition order with ultrathink is
 * locked: `Ultrathink:` lives at the very top, then the mode wrapper,
 * then the user's text — so the model sees the high-level "think
 * hard" directive first, the framing second, and the question last.
 *
 * `applyModePrefix` is idempotent: re-applying the same wrapper to a
 * string that already starts with it is a no-op, which keeps the
 * transcript / send-path safe under repeated calls (mirrors the
 * idempotency guarantees in `applyClaudePromptEffortPrefix`).
 */

import type { ChatMode } from "@/stores/agent-chat-store";

import {
  applyClaudePromptEffortPrefix,
  stripClaudeUltrathinkPrefix,
} from "./ultrathink";

export const ASK_WRAPPER =
  "You are in ASK mode. Answer questions directly and concisely about the code, files, or concepts in question. Do not make any changes to code or files. Do not call ExitPlanMode — this is a conversational question, not a planning session.";

export const DEBUG_WRAPPER = `You are in DEBUG mode. To help diagnose this issue:
1. Add diagnostic log statements tagged with \`CODEMUX_DEBUG\` using the appropriate comment syntax for each language.
2. Make logs descriptive — include relevant variable values and execution context.
3. When the bug is resolved, the user will trigger cleanup. You don't need to remove logs manually until then.`;

/**
 * Apply the per-mode wrapper to `text`. `default` and `plan` pass
 * through unchanged (Plan uses SDK enforcement, not a wrapper).
 * Empty / whitespace-only input returns unchanged so the call site
 * doesn't fire a wrapper-only payload at the SDK.
 */
export function applyModePrefix(text: string, mode: ChatMode): string {
  const trimmed = text.trim();
  if (!trimmed) return trimmed;
  if (mode === "ask") {
    if (trimmed.startsWith(ASK_WRAPPER)) return trimmed;
    return `${ASK_WRAPPER}\n\n${trimmed}`;
  }
  if (mode === "debug") {
    if (trimmed.startsWith(DEBUG_WRAPPER)) return trimmed;
    return `${DEBUG_WRAPPER}\n\n${trimmed}`;
  }
  return trimmed;
}

/**
 * Prepend a staged-skill body to `text`, framed by a markdown rule so
 * the model can clearly distinguish the procedural skill content from
 * the user's actual request.
 *
 * Composition order, top-to-bottom (when chained with mode + ultrathink):
 *   1. `Ultrathink:` directive
 *   2. Mode wrapper (ASK / DEBUG)
 *   3. Skill body (this fn)
 *   4. Attachment block (Step 8 Stage 2)
 *   5. `---` separator
 *   6. User text
 *
 * This places the skill *between* high-level framing and the user's
 * literal ask, matching how Claude itself injects skill content when it
 * auto-triggers a skill. Idempotent: re-applying the same skill body to
 * an already-prefixed string is a no-op.
 */
export function applySkillPrefix(text: string, skillBody: string | null | undefined): string {
  if (!skillBody) return text;
  const trimmedBody = skillBody.trim();
  if (!trimmedBody) return text;
  if (text.startsWith(trimmedBody)) return text;
  return `${trimmedBody}\n\n---\n\n${text}`;
}

/**
 * Prepend a staged-attachment block to `text`, framed by the same
 * markdown rule used for skills. Sits *between* the skill body and
 * the user's literal ask in the final composition (Step 8 Stage 2),
 * so the model reads behavior framing → skill → context → question.
 *
 * Idempotent: re-applying the same block to an already-prefixed
 * string is a no-op (mirrors `applySkillPrefix`).
 */
export function applyAttachmentPrefix(
  text: string,
  attachmentBlock: string | null | undefined,
): string {
  if (!attachmentBlock) return text;
  const trimmedBlock = attachmentBlock.trim();
  if (!trimmedBlock) return text;
  if (text.startsWith(trimmedBlock)) return text;
  return `${trimmedBlock}\n\n---\n\n${text}`;
}

/**
 * Compose mode wrapper + ultrathink prefix in the locked order:
 *   `Ultrathink:\n${MODE_WRAPPER}\n\n${SKILL}\n\n---\n\n${ATTACHMENTS}\n\n---\n\n${user text}`
 *
 * Ultrathink is applied LAST so it lands at the very top of the
 * payload (matches the per-turn-prepend pattern Anthropic recommends
 * for high-level directives). When `effort === "ultrathink"` we strip
 * any leading Ultrathink prefix on the input first so the re-add
 * lands above the mode wrapper instead of getting buried inside it
 * — this keeps the helper idempotent under repeated application.
 *
 * The attachment block (Step 8 Stage 2) is applied BEFORE the skill
 * body so that, after wrapping, the skill is OUTSIDE the attachments —
 * matching the behavior-then-context reading order the agent expects.
 */
export function applyAllPrefixes(
  text: string,
  mode: ChatMode,
  effort: string | null | undefined,
  stagedSkillBody?: string | null,
  attachmentBlock?: string | null,
): string {
  const base = effort === "ultrathink" ? stripClaudeUltrathinkPrefix(text) : text;
  const withAttachments = applyAttachmentPrefix(base, attachmentBlock);
  const withSkill = applySkillPrefix(withAttachments, stagedSkillBody);
  const withMode = applyModePrefix(withSkill, mode);
  return applyClaudePromptEffortPrefix(withMode, effort);
}
