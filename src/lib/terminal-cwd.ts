/**
 * Terminal pane cwd labels.
 *
 * The terminal pane header used to render a bare `"Terminal"` — the same
 * word the tab directly above it already shows, so the bar carried zero
 * information. This module turns a session's live working directory into
 * the short hint that goes next to that title.
 *
 * The design rule is **signal only when it's surprising**:
 *
 * - Sitting at the workspace root (the overwhelmingly common case, and
 *   where every session starts) produces `null` — the header stays
 *   exactly as it is today. A header that changed on every pane would be
 *   visual churn, not information.
 * - Anywhere else, show the shortest label that identifies the location.
 *
 * Absolute paths are deliberately NOT rendered in full. The header is a
 * 28px strip that CSS-truncates on the *right*, so a long path degrades
 * to `/home/zeus/.codemux/worktr…` — it drops the only segment the user
 * cared about. Trimming to a leading `…/` plus the trailing segments
 * keeps the meaningful tail and fits a narrow split pane. The untrimmed
 * path is preserved separately for the hover tooltip.
 */

/** How many trailing path segments a trimmed label keeps. Two is enough
 *  to disambiguate the common collisions (`src/components` vs
 *  `docs/components`) without overflowing a split pane's header. */
const MAX_TAIL_SEGMENTS = 2;

export interface CwdHint {
  /** Short label for the header, e.g. `src-tauri` or `~/…/dotfiles`. */
  label: string;
  /** Untrimmed, human-form path for the `title=` hover tooltip. */
  full: string;
}

/** Normalize separators to `/` and drop any trailing separator so
 *  `/a/b` and `/a/b/` compare equal. The filesystem root stays `/`. */
function normalize(path: string): string {
  const slashed = path.replace(/\\/g, "/");
  const trimmed = slashed.replace(/\/+$/, "");
  return trimmed === "" ? "/" : trimmed;
}

/** True when `child` is the same path as `parent` or nested inside it.
 *  Compares segment-wise via the separator so `/foo/bar-baz` is not
 *  treated as living under `/foo/bar`. */
function isUnder(child: string, parent: string): boolean {
  if (child === parent) return true;
  const prefix = parent === "/" ? "/" : `${parent}/`;
  return child.startsWith(prefix);
}

/** Keep at most the last `MAX_TAIL_SEGMENTS` segments, marking elision
 *  with a leading `…/`. `prefix` is prepended verbatim (`""` or `"~"`). */
function trimTail(relative: string, prefix: string): string {
  const parts = relative.split("/").filter(Boolean);
  const elided = parts.length > MAX_TAIL_SEGMENTS;
  const tail = parts.slice(-MAX_TAIL_SEGMENTS).join("/");
  if (!prefix) return elided ? `…/${tail}` : tail;
  return elided ? `${prefix}/…/${tail}` : `${prefix}/${tail}`;
}

/**
 * Build the header hint for a terminal session.
 *
 * @param cwd            The session's live working directory, or null when
 *                       it isn't known yet (no OSC 7 emitted and the
 *                       `/proc` poll hasn't answered).
 * @param workspaceRoot  The owning workspace's cwd — the "home base" the
 *                       label is measured against.
 * @param homeDir        `$HOME`, used to contract out-of-workspace paths
 *                       to `~`. Null until the app hydrates it.
 * @returns The hint, or null when there is nothing worth showing.
 */
export function formatCwdHint(
  cwd: string | null | undefined,
  workspaceRoot: string | null | undefined,
  homeDir: string | null | undefined,
): CwdHint | null {
  if (!cwd) return null;

  const full = cwd.replace(/\\/g, "/");
  const path = normalize(cwd);
  const root = workspaceRoot ? normalize(workspaceRoot) : null;
  const home = homeDir ? normalize(homeDir) : null;

  // At (or effectively at) the workspace root: say nothing.
  if (root && path === root) return null;

  // Inside the workspace: the relative path is both the shortest and the
  // most meaningful label, since the workspace name is already on screen
  // in the sidebar and tab.
  if (root && isUnder(path, root)) {
    const relative = root === "/" ? path.slice(1) : path.slice(root.length + 1);
    return { label: trimTail(relative, ""), full };
  }

  // Outside the workspace but under $HOME: contract to `~` so the label
  // doesn't burn its budget on `/home/<user>`.
  if (home && path === home) return { label: "~", full };
  if (home && isUnder(path, home)) {
    const relative = path.slice(home.length + 1);
    return { label: trimTail(relative, "~"), full };
  }

  // Fully outside: an absolute path, trimmed to its tail. When the whole
  // path already fits the segment budget it is shown as-is — crucially
  // *keeping* its absolute form, because a trimmed `/var/log` rendered as
  // `var/log` would be indistinguishable from a workspace-relative path.
  const parts = path.split("/").filter(Boolean);
  if (parts.length <= MAX_TAIL_SEGMENTS) return { label: path, full };
  return { label: trimTail(path, ""), full };
}
