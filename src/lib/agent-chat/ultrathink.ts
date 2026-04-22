/**
 * Ultrathink is a Claude-only "6th effort level" implemented as a
 * prompt prefix rather than an SDK parameter. T3Code's mechanism,
 * ported verbatim — see their `shared/src/model.ts:173-300`.
 *
 * Three layers of defense (client picker, client send-path, sidecar
 * on-turn) all call `applyClaudePromptEffortPrefix`. Idempotency makes
 * the double-fire safe.
 */

export const ULTRATHINK_PROMPT_PREFIX = "Ultrathink:\n";

const ULTRATHINK_DETECTOR = /\bultrathink\b/i;

/**
 * Prepend `Ultrathink:\n` to `text` when `effort === "ultrathink"`,
 * idempotently. Any other effort value (including null/undefined) is
 * a no-op. Empty prompts are returned unchanged (no spurious prefix).
 */
export function applyClaudePromptEffortPrefix(
  text: string,
  effort: string | null | undefined,
): string {
  const trimmed = text.trim();
  if (!trimmed) return trimmed;
  if (effort !== "ultrathink") return trimmed;
  if (trimmed.startsWith("Ultrathink:")) return trimmed;
  return `${ULTRATHINK_PROMPT_PREFIX}${trimmed}`;
}

/** Case-insensitive word-boundary check for `ultrathink` in `text`. */
export function isClaudeUltrathinkPrompt(
  text: string | null | undefined,
): boolean {
  return typeof text === "string" && ULTRATHINK_DETECTOR.test(text);
}

/**
 * Strip the leading `"Ultrathink:\s*"` (case-insensitive) prefix, used
 * when the user deselects ultrathink — the UI removes the prefix from
 * the draft so effort state and prompt stay consistent.
 */
export function stripClaudeUltrathinkPrefix(text: string): string {
  return text.replace(/^Ultrathink:\s*/i, "");
}

/**
 * Detect the "ultrathink in body text" state: the user typed the word
 * ultrathink outside the prefix itself. While in this state the effort
 * picker locks — T3Code shows a "Remove it to change effort" hint.
 */
export function hasUltrathinkInBodyText(
  text: string | null | undefined,
): boolean {
  if (typeof text !== "string") return false;
  return isClaudeUltrathinkPrompt(stripClaudeUltrathinkPrefix(text));
}
