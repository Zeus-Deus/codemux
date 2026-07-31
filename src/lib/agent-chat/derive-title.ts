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

import { basename } from "@/lib/path";

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

/** Whether a workspace is still wearing a title the BACKEND assigned,
 *  rather than one a human (or a branch) chose.
 *
 *  Used to decide whether an already-existing workspace may be
 *  auto-renamed from its first chat message. A user-chosen name — or one
 *  inherited from a worktree branch — must never be silently
 *  overwritten, so anything not recognised here is left alone.
 *
 *  Two shapes count as default, both from `state_impl.rs`'s
 *  `default_workspace_title`:
 *   - the directory's own name (`~/projects/codemux` → `codemux`), the
 *     current default; and
 *   - `Workspace {n}`, the legacy default, still worn by every workspace
 *     created before that change — they must stay upgradeable.
 *
 *  `dirPath` is the workspace's cwd/project root. Omit it and only the
 *  legacy shape is recognised, which is the safe direction: the worst
 *  case is declining to rename.
 *
 *  Note the deliberate consequence: if a user manually renames a
 *  workspace to exactly its directory name, a first prompt will still
 *  rename it. That collision is indistinguishable from the default by
 *  construction, and re-titling a workspace the user never really named
 *  is the cheaper error. */
export function isDefaultWorkspaceTitle(
  title: string | null | undefined,
  dirPath?: string | null,
): boolean {
  // No title at all is the clearest case of "not user-chosen" — there is
  // nothing here to protect, so it is always safe to name.
  const trimmed = title?.trim();
  if (!trimmed) return true;
  if (/^Workspace \d+$/.test(trimmed)) return true;
  if (!dirPath) return false;
  return trimmed === basename(dirPath);
}
