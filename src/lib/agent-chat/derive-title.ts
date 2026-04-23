/**
 * Turn a user's first chat message into a workspace title.
 *
 * Used by `materializeAndSend` and `materializeWithPreset` when they
 * promote a Home-target draft — the new regular workspace needs a
 * human-friendly sidebar label instead of the $HOME basename.
 *
 * Rules:
 *  - Trim leading / trailing whitespace.
 *  - Empty → `null` (caller falls back to the path-derived default).
 *  - ≤ `MAX_TITLE` chars → return as-is.
 *  - Longer than `MAX_TITLE`: hard-truncate to `MAX_TITLE`, then try to
 *    pull back to the last space within `WORD_BOUNDARY_LOOKBACK` chars.
 *    If a space exists in that window, cut there (no ellipsis). If
 *    not, keep the hard cut and append `…` so the user can tell it's
 *    truncated.
 */

const MAX_TITLE = 40;
const WORD_BOUNDARY_LOOKBACK = 15;

export function deriveTitleFromFirstMessage(text: string): string | null {
  const trimmed = text.trim();
  if (trimmed.length === 0) return null;

  if (trimmed.length <= MAX_TITLE) return trimmed;

  const hardCut = trimmed.slice(0, MAX_TITLE);
  // Try to trim back to a word boundary within the last
  // WORD_BOUNDARY_LOOKBACK chars.
  const lastSpace = hardCut.lastIndexOf(" ");
  if (lastSpace >= MAX_TITLE - WORD_BOUNDARY_LOOKBACK) {
    return hardCut.slice(0, lastSpace).trimEnd();
  }

  // No good word boundary — hard truncate with ellipsis.
  return hardCut.trimEnd() + "…";
}
