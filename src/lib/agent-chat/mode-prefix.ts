/**
 * Per-turn prompt wrappers for Cursor-style mode pills.
 *
 * Mirrors `ultrathink.ts`: each mode contributes a prefix that the
 * send-path composes onto the user's raw text. Stage 4 ships ASK;
 * Stage 6 will fill DEBUG. Composition order with ultrathink is
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

export const DEBUG_WRAPPER = "";

/**
 * Apply the per-mode wrapper to `text`. `default` and (for now)
 * `debug` pass through unchanged. Empty / whitespace-only input
 * returns unchanged so the call site doesn't fire a wrapper-only
 * payload at the SDK.
 */
export function applyModePrefix(text: string, mode: ChatMode): string {
  const trimmed = text.trim();
  if (!trimmed) return trimmed;
  if (mode === "ask") {
    if (trimmed.startsWith(ASK_WRAPPER)) return trimmed;
    return `${ASK_WRAPPER}\n\n${trimmed}`;
  }
  return trimmed;
}

/**
 * Compose mode wrapper + ultrathink prefix in the locked order:
 *   `Ultrathink:\n${MODE_WRAPPER}\n\n${user text}`
 *
 * Ultrathink is applied LAST so it lands at the very top of the
 * payload (matches the per-turn-prepend pattern Anthropic recommends
 * for high-level directives). When `effort === "ultrathink"` we strip
 * any leading Ultrathink prefix on the input first so the re-add
 * lands above the mode wrapper instead of getting buried inside it
 * — this keeps the helper idempotent under repeated application.
 */
export function applyAllPrefixes(
  text: string,
  mode: ChatMode,
  effort: string | null | undefined,
): string {
  const base = effort === "ultrathink" ? stripClaudeUltrathinkPrefix(text) : text;
  const withMode = applyModePrefix(base, mode);
  return applyClaudePromptEffortPrefix(withMode, effort);
}
