/**
 * Pure search/ranking model behind the command palette.
 *
 * Kept out of the component so the matching rules — which are the part that
 * actually decides whether the palette feels good — are unit-testable without
 * mounting cmdk, Radix, or the Tauri stores.
 */

import { fuzzyScore } from "@/lib/fuzzy";
import { statusRank } from "@/lib/pane-status";
import type { ActivePaneStatus } from "@/tauri/types";

/** Typing this as the first character narrows the palette to commands only. */
export const COMMAND_MODE_PREFIX = ">";

/**
 * Typing this narrows the palette to loaded pull requests.
 *
 * A word rather than a symbol because it is what you would type anyway:
 * `pr 6318` and `pr windows shutdown` are both things people already
 * write in chat. The trailing space is part of the prefix so a workspace
 * called `prototype` is still reachable by name.
 */
export const PR_MODE_PREFIX = "pr ";

export type PaletteMode = "all" | "commands" | "prs";

export interface PaletteQuery {
  /** Exactly what the user typed, prefix included. */
  raw: string;
  mode: PaletteMode;
  /** The search text with any mode prefix stripped and trimmed. */
  needle: string;
  /**
   * True when the query looks like a path. Mirrors the sidebar's Thread Scope
   * rule: a `/` means "I'm searching by location", which switches every
   * candidate from its name haystack to its path haystack.
   */
  pathMode: boolean;
}

export function parsePaletteQuery(raw: string): PaletteQuery {
  const leading = raw.trimStart();
  const commandMode = leading.startsWith(COMMAND_MODE_PREFIX);
  const prMode =
    !commandMode && leading.toLowerCase().startsWith(PR_MODE_PREFIX);
  const body = commandMode
    ? leading.slice(COMMAND_MODE_PREFIX.length)
    : prMode
      ? leading.slice(PR_MODE_PREFIX.length)
      : raw;
  const needle = body.trim();
  return {
    raw,
    mode: commandMode ? "commands" : prMode ? "prs" : "all",
    needle,
    // A pull-request query is never a path query: `owner/repo` is a
    // perfectly ordinary thing to type at it.
    pathMode: !prMode && needle.includes("/"),
  };
}

/** The shape `pr ` mode ranks. Structural, so the palette can hand it
 *  overview rows without the model importing them. */
export interface PalettePr {
  number: number;
  title: string;
  repo?: string | null;
  author?: string | null;
}

export function prSearchText(pr: PalettePr): string {
  return [`#${pr.number}`, `!${pr.number}`, pr.title, pr.repo ?? "", pr.author ?? ""]
    .join(" ")
    .trim();
}

/**
 * Rank pull requests for `pr <query>`.
 *
 * A number is treated as a number: `pr 285` puts #285 first even though
 * "285" also appears inside #12850 and inside three titles. Anything
 * else is ordinary fuzzy matching over the number, title, repository and
 * author.
 */
export function rankPalettePrs<T extends PalettePr>(
  items: readonly T[],
  needle: string,
): T[] {
  if (needle === "") return [...items];
  const digitsOnly = /^\d+$/.test(needle);
  const scored: { item: T; score: number }[] = [];
  for (const item of items) {
    let score: number | null = null;
    if (digitsOnly) {
      const number = String(item.number);
      if (number === needle) score = 1_000_000;
      else if (number.startsWith(needle)) score = 500_000;
      else if (number.includes(needle)) score = 250_000;
    }
    if (score === null) score = fuzzyScore(prSearchText(item), needle);
    if (score !== null) scored.push({ item, score });
  }
  return scored.sort((a, b) => b.score - a.score).map((entry) => entry.item);
}

/**
 * Filter + rank `items` best-first against the query.
 *
 * Deliberately scores ONE haystack per item (see `@/lib/fuzzy`): folding a
 * name and a full path into one string makes subsequence matching hit almost
 * everything, so the list stops narrowing. `pathMode` picks which haystack the
 * query is aimed at. An empty needle returns the caller's own ordering
 * untouched, which is how the resting palette keeps its status/recency sort.
 */
export function rankByQuery<T>(
  items: readonly T[],
  query: PaletteQuery,
  nameText: (item: T) => string,
  pathText: (item: T) => string,
): T[] {
  if (query.needle === "") return [...items];
  const haystack = query.pathMode ? pathText : nameText;
  const scored: { item: T; score: number }[] = [];
  for (const item of items) {
    const score = fuzzyScore(haystack(item), query.needle);
    if (score !== null) scored.push({ item, score });
  }
  // Array.prototype.sort is stable, so equal scores keep the caller's order.
  return scored.sort((a, b) => b.score - a.score).map((entry) => entry.item);
}

/** The search haystack for a workspace in name mode: what the row shows. */
export function workspaceSearchText(workspace: {
  title: string;
  git_branch: string | null;
}): string {
  return [workspace.title, workspace.git_branch].filter(Boolean).join(" ");
}

/**
 * The search haystack for a workspace in path mode: **locations only**.
 *
 * Folding the title and branch in here would reintroduce exactly the failure
 * this module warns about above — `projects/vexis` would then match
 * unrelated projects whose *names* happen to supply the missing characters,
 * and rank them above the real answer. Settled workspaces remain live
 * workspace records, so they are indexed here exactly like active ones.
 */
export function workspacePathText(workspace: {
  project_root: string | null;
  worktree_path?: string | null;
  cwd: string;
}): string {
  const paths = [workspace.project_root, workspace.worktree_path, workspace.cwd]
    .filter((p): p is string => !!p);
  // A worktree's cwd is usually its worktree_path; don't pay for it twice.
  return [...new Set(paths)].join(" ");
}

/**
 * The mono sub-label on a workspace row.
 *
 * The avatar carries a project letter but not which project, so the project
 * name is the default context. The branch is appended only when the title
 * doesn't already imply it — most Codemux workspaces are named after their
 * branch, and printing `workflow-approval  demo/workflow-approval` spends the
 * row's whole sub-label saying the same thing twice.
 */
export function workspaceRowSubtitle(
  workspace: { title: string; git_branch: string | null },
  projectName: string,
): string {
  const branch = workspace.git_branch;
  if (!branch) return projectName;
  const leaf = branch.slice(branch.lastIndexOf("/") + 1).toLowerCase();
  const title = workspace.title.toLowerCase();
  if (title === branch.toLowerCase() || title === leaf) return projectName;
  return `${projectName} · ${branch}`;
}

/** Ordering inputs for the resting (unsearched) workspace list. */
export interface WorkspaceOrder {
  /** Live agent status; null is genuinely idle. */
  status: ActivePaneStatus | null;
  /** Best-known "last thing happened here" stamp (ms epoch), if any. */
  activityAt: number | undefined;
  /** Parked onto the sidebar's Settled or Snoozed shelf. */
  parked: boolean;
}

/**
 * Resting order: whatever is asking for you first, then what you touched most
 * recently, with parked work last. The palette's default list is a
 * "where was I?" list, not an alphabetical index.
 */
export function compareWorkspaceOrder(a: WorkspaceOrder, b: WorkspaceOrder): number {
  if (a.parked !== b.parked) return a.parked ? 1 : -1;
  const rank = statusRank(b.status) - statusRank(a.status);
  if (rank !== 0) return rank;
  // An unknown stamp sorts last. This is a display tiebreak inside an already
  // status-ranked list, NOT the "idle since forever" reading that
  // `last_active_at`'s contract forbids — nothing here settles, sweeps, or
  // ages out a workspace, and a workspace with a live status has already been
  // ranked above this line.
  return (b.activityAt ?? 0) - (a.activityAt ?? 0);
}

/** The search haystack for a command row: its label plus hidden synonyms. */
export function commandSearchText(command: {
  label: string;
  keywords?: string;
}): string {
  return command.keywords ? `${command.label} ${command.keywords}` : command.label;
}

// ── Themes ───────────────────────────────────────────────────────────────

/**
 * The generic words the whole theme group answers to, alongside each row's
 * own name. The picker's advertised entry point is the word itself —
 * ⌘K → "theme" has to surface the set.
 */
export const THEME_KEYWORDS = "theme colors palette appearance";

/**
 * Rank the theme group: by name when the query names one, otherwise the whole
 * set in registry order.
 *
 * Folding the generic keywords into each row's haystack — the obvious first
 * try — makes "theme" match every row with a score that differs only by name
 * length, so the list comes back sorted by how short each theme is called.
 * The keywords are a group-level gate instead: they decide *whether* the
 * group shows, and registry order (built-ins as declared, then custom) decides
 * the order. `rankByQuery` still handles the case that matters, a real name.
 */
export function rankThemeGroup<T>(
  items: readonly T[],
  query: PaletteQuery,
  nameText: (item: T) => string,
): T[] {
  if (query.needle === "") return [];
  const byName = rankByQuery(items, query, nameText, nameText);
  if (byName.length > 0) return byName;
  return fuzzyScore(THEME_KEYWORDS, query.needle) === null ? [] : [...items];
}

/**
 * Theme rows are addressed by a prefixed cmdk value so the palette can tell,
 * from the selected value alone, whether the highlighted row is a theme — and
 * therefore whether the app should currently be wearing a preview.
 */
export const THEME_ROW_PREFIX = "theme:";

export function themeRowValue(themeId: string): string {
  return `${THEME_ROW_PREFIX}${themeId}`;
}

/** The theme id the palette should be previewing, or null for any other row. */
export function previewedThemeId(selectedValue: string): string | null {
  if (!selectedValue.startsWith(THEME_ROW_PREFIX)) return null;
  const id = selectedValue.slice(THEME_ROW_PREFIX.length);
  return id === "" ? null : id;
}

/**
 * Group-header count. When a group is capped, say so rather than printing a
 * number that silently under-reports what matched.
 */
export function groupCountLabel(shown: number, total: number): string {
  return shown < total ? `${shown} of ${total}` : `${total}`;
}

export function resultCountLabel(count: number): string {
  return count === 1 ? "1 result" : `${count} results`;
}
