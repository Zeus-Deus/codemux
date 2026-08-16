/**
 * The Pull Requests page's model, with no React in it.
 *
 * Grouping, filtering, the badge count and the ordering freeze are all
 * decisions about a list of rows, and all four are things the page must
 * be able to get provably right — so they live here as functions over
 * plain data rather than inside a component that would have to be
 * mounted to ask them a question.
 */

import type { PrOverviewItem } from "@/tauri/types";

/** One row: an overview item plus where it came from. */
export interface PrRow extends PrOverviewItem {
  /** Repository root this row was fetched for — half of its identity. */
  projectRoot: string;
  /** `owner/name`, when the URL could name it. */
  repo: string | null;
  /** Wire `provider_kind`; decides the `#`/`!` sigil and the host mark. */
  providerKind: string;
  /** Host state, for the rows the state dropdown pulls in. Absent means
   *  open — the overview only ever returns open pull requests. */
  state?: string;
}

export type PrGroupId = "review" | "yours" | "watching";

export const GROUP_ORDER: PrGroupId[] = ["review", "yours", "watching"];

export const GROUP_LABEL: Record<PrGroupId, string> = {
  review: "Needs your review",
  yours: "Yours",
  watching: "Watching",
};

/**
 * Stable identity for a row across polls: a number is only unique
 * within one repository.
 *
 * NUL joins the two halves because it is the one byte a path cannot
 * contain, so no two repositories can produce the same key. Every
 * caller uses this function rather than rebuilding the string: a
 * separator two call sites can disagree about is a bug that looks
 * like a missing row.
 */
export function rowKey(row: { projectRoot: string; number: number }): string {
  return `${row.projectRoot}\u0000${row.number}`;
}

function sameLogin(a: string | null | undefined, b: string | null | undefined): boolean {
  return !!a && !!b && a.toLowerCase() === b.toLowerCase();
}

/**
 * Which group a row belongs to.
 *
 * Review-requested wins over authorship: a pull request you opened and
 * were then asked to review is, in practice, waiting on you. Without a
 * viewer nothing can be attributed, and everything is Watching — an
 * honest answer rather than a guessed one.
 */
export function groupForRow(row: PrRow, viewer: string | null): PrGroupId {
  if (!viewer) return "watching";
  if (row.review_requested_from.some((login) => sameLogin(login, viewer))) {
    return "review";
  }
  if (sameLogin(row.author, viewer)) return "yours";
  return "watching";
}

/** Newest first. A row with no timestamp sorts last rather than first —
 *  unknown is not new. */
function byUpdatedDesc(a: PrRow, b: PrRow): number {
  const ta = a.updated_at ? Date.parse(a.updated_at) : 0;
  const tb = b.updated_at ? Date.parse(b.updated_at) : 0;
  return tb - ta;
}

export interface PrGroup {
  id: PrGroupId;
  rows: PrRow[];
}

/**
 * Rows in reading order: the two groups that want something from you,
 * then everything else.
 *
 * `viewerByRoot` is per repository root because the accounts differ —
 * one machine can be signed into a work GitLab and a personal GitHub at
 * once, and a single "viewer" would mis-file every row of one of them.
 */
export function groupRows(
  rows: PrRow[],
  viewerByRoot: Map<string, string | null>,
): PrGroup[] {
  const buckets: Record<PrGroupId, PrRow[]> = {
    review: [],
    yours: [],
    watching: [],
  };
  for (const row of rows) {
    buckets[groupForRow(row, viewerByRoot.get(row.projectRoot) ?? null)].push(row);
  }
  return GROUP_ORDER.map((id) => ({ id, rows: buckets[id].sort(byUpdatedDesc) }));
}

// ── Search ────────────────────────────────────────────────────────────

export interface PrSearch {
  /** Free text, lowercased. Empty when the query was only tokens. */
  text: string;
  isDraft: boolean | null;
  /** `passing` / `failing` / `pending` / `none`, when asked for. */
  ci: string | null;
  author: string | null;
}

const CI_ALIASES: Record<string, string> = {
  failing: "failing",
  failed: "failing",
  fail: "failing",
  red: "failing",
  passing: "passing",
  passed: "passing",
  pass: "passing",
  green: "passing",
  pending: "pending",
  running: "pending",
  none: "none",
};

/**
 * Split `is:draft ci:failing windows` into filters and words.
 *
 * Unknown tokens stay in the free text on purpose: `fix:` is a plausible
 * thing to type in a title search, and silently dropping the part of the
 * query the parser didn't recognise makes the result list a lie.
 */
export function parsePrSearch(query: string): PrSearch {
  const search: PrSearch = { text: "", isDraft: null, ci: null, author: null };
  const words: string[] = [];
  for (const part of query.trim().split(/\s+/).filter(Boolean)) {
    const [rawKey, ...rest] = part.split(":");
    const value = rest.join(":").toLowerCase();
    const key = rawKey.toLowerCase();
    if (key === "is" && (value === "draft" || value === "ready")) {
      search.isDraft = value === "draft";
      continue;
    }
    if (key === "ci" && CI_ALIASES[value]) {
      search.ci = CI_ALIASES[value];
      continue;
    }
    if (key === "author" && value) {
      search.author = value;
      continue;
    }
    words.push(part.toLowerCase());
  }
  search.text = words.join(" ");
  return search;
}

/** Every filter is an AND: each token the user typed narrows. */
export function matchesPrSearch(row: PrRow, search: PrSearch): boolean {
  if (search.isDraft != null && row.is_draft !== search.isDraft) return false;
  if (search.ci != null && row.checks !== search.ci) return false;
  if (search.author != null && !row.author.toLowerCase().includes(search.author)) {
    return false;
  }
  if (search.text) {
    const haystack = [
      row.title,
      row.author,
      row.repo ?? "",
      row.head_branch ?? "",
      `#${row.number}`,
      `!${row.number}`,
    ]
      .join(" ")
      .toLowerCase();
    return search.text.split(" ").every((word) => haystack.includes(word));
  }
  return true;
}

// ── Badge ─────────────────────────────────────────────────────────────

/**
 * What the sidebar badge counts: reviews waiting on you, and your own
 * pull requests that have gone red.
 *
 * Keys rather than a number so the same row counted two ways is counted
 * once, and so opening the page can mark exactly what was on it as seen
 * instead of resetting a counter that the next poll would refill.
 */
export function badgeKeys(
  rows: PrRow[],
  viewerByRoot: Map<string, string | null>,
): string[] {
  const keys = new Set<string>();
  for (const row of rows) {
    const viewer = viewerByRoot.get(row.projectRoot) ?? null;
    if (!viewer) continue;
    const group = groupForRow(row, viewer);
    if (group === "review") keys.add(rowKey(row));
    else if (group === "yours" && row.checks === "failing") keys.add(rowKey(row));
  }
  return [...keys];
}

/** The count, minus whatever the user has already looked at. */
export function badgeCount(keys: string[], seen: ReadonlySet<string>): number {
  return keys.filter((key) => !seen.has(key)).length;
}

/** Past nine the exact number stops being information. */
export function badgeLabel(count: number): string {
  return count > 9 ? "9+" : String(count);
}

// ── Rule 03: polling can update, but not rearrange ───────────────────

/**
 * A row's place in the list: which group, in what order.
 *
 * The freeze works on group *and* index because a row hopping from
 * Watching into Needs-your-review under the pointer is precisely the
 * rearrangement the rule exists to prevent — it moves every row below it
 * by one, mid-click.
 */
export interface PlanEntry {
  key: string;
  groupId: PrGroupId;
}

export interface FrozenPlan {
  entries: PlanEntry[];
  /** Rows the poll moved, held back until the next deliberate action. */
  moved: Set<string>;
}

/**
 * Hold `next` in `snapshot`'s order.
 *
 * Rows that were there keep the slot they had. Rows that are new go to
 * the end of the list rather than into the middle of it, and are marked.
 * Rows that vanished are dropped: a merged pull request that stays on
 * screen is a lie, where a row arriving a few seconds late is only late.
 */
export function freezePlan(
  next: PlanEntry[],
  snapshot: PlanEntry[] | null,
): FrozenPlan {
  if (!snapshot) return { entries: next, moved: new Set() };

  const nextByKey = new Map(next.map((entry) => [entry.key, entry]));
  const held: PlanEntry[] = [];
  const seen = new Set<string>();
  for (const entry of snapshot) {
    if (!nextByKey.has(entry.key)) continue;
    // The *old* slot, deliberately: the new group is what would move it.
    held.push(entry);
    seen.add(entry.key);
  }

  const moved = new Set<string>();
  const arrived = next.filter((entry) => !seen.has(entry.key));
  for (const entry of arrived) moved.add(entry.key);

  // Order among the rows that survived, before and after. A row whose
  // neighbours changed is one the poll would have moved.
  const heldOrder = held.map((entry) => entry.key);
  const nextOrder = next.filter((entry) => seen.has(entry.key)).map((e) => e.key);
  for (let i = 0; i < heldOrder.length; i++) {
    if (heldOrder[i] !== nextOrder[i]) moved.add(heldOrder[i]);
    const before = snapshot.find((e) => e.key === heldOrder[i]);
    const after = nextByKey.get(heldOrder[i]);
    if (before && after && before.groupId !== after.groupId) moved.add(heldOrder[i]);
  }

  return { entries: [...held, ...arrived], moved };
}

/** Re-apply a plan's order to the rows it describes. */
export function applyPlan(rows: PrRow[], entries: PlanEntry[]): PrGroup[] {
  const byKey = new Map(rows.map((row) => [rowKey(row), row]));
  const buckets: Record<PrGroupId, PrRow[]> = { review: [], yours: [], watching: [] };
  for (const entry of entries) {
    const row = byKey.get(entry.key);
    if (row) buckets[entry.groupId].push(row);
  }
  return GROUP_ORDER.map((id) => ({ id, rows: buckets[id] }));
}

/** The flat plan a set of groups describes. */
export function planOf(groups: PrGroup[]): PlanEntry[] {
  return groups.flatMap((group) =>
    group.rows.map((row) => ({ key: rowKey(row), groupId: group.id })),
  );
}
