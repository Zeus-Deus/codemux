/**
 * Scored subsequence ("fuzzy") matching for type-to-filter pickers.
 *
 * The rule every picker in the app shares: a query matches when its
 * characters appear in order somewhere in the candidate, so `cdx`
 * finds `codemux` and `vas` finds `vexis-agent-site`. On top of that
 * bare test, `fuzzyScore` ranks matches so the row a user *meant*
 * lands first:
 *
 *  - whole-query prefix beats whole-query substring beats a scattered
 *    subsequence (`co` → `codemux` before `openclaw`),
 *  - characters landing on a word boundary (`-`, `_`, `.`, `/`, space)
 *    or a camelCase hump score like initials, which is what makes
 *    typing the first letter of each word work,
 *  - adjacent matches beat scattered ones, and long gaps are
 *    penalized,
 *  - on an otherwise equal match, the shorter candidate wins.
 *
 * Scores are only comparable within one candidate list — treat them as
 * a sort key, never as a quality threshold.
 */

/** Characters that make the NEXT character read as the start of a word. */
const BOUNDARY_CHARS = new Set([" ", "-", "_", ".", "/", "\\", ":", "@", "~", "("]);

/** True when `text[index]` is a camelCase hump (`agentChat` → `C`). */
function isCamelHump(text: string, index: number): boolean {
  if (index === 0) return false;
  const ch = text[index];
  const prev = text[index - 1];
  return ch >= "A" && ch <= "Z" && prev >= "a" && prev <= "z";
}

/**
 * Score `query` against `text`, or `null` when `query` is not a
 * subsequence of `text`. An empty query scores `0` (matches
 * everything). Higher is better; see the module doc for the ranking
 * rules.
 */
export function fuzzyScore(text: string, query: string): number | null {
  if (query.length === 0) return 0;
  const haystack = text.toLowerCase();
  const needle = query.toLowerCase();
  if (needle.length > haystack.length) return null;

  let score = 0;
  let cursor = 0;
  let previous = -2;

  for (let qi = 0; qi < needle.length; qi++) {
    const at = haystack.indexOf(needle[qi], cursor);
    if (at === -1) return null;

    if (at === 0) score += 16;
    else if (BOUNDARY_CHARS.has(haystack[at - 1])) score += 12;
    else if (isCamelHump(text, at)) score += 8;

    if (at === previous + 1) score += 8;
    // Skipping characters to reach this match is weak evidence —
    // capped so one long gap doesn't sink an otherwise good row.
    score -= Math.min(at - cursor, 8);

    cursor = at + 1;
    previous = at;
  }

  if (haystack.startsWith(needle)) score += 40;
  else if (haystack.includes(needle)) score += 20;

  // Tie-break toward the tighter candidate: `co` should rank
  // `codemux` above `codemux-sitev2`.
  score -= Math.min(haystack.length - needle.length, 30) * 0.1;

  return score;
}

/** `true` when `query`'s characters appear in order inside `text`. */
export function fuzzyMatch(text: string, query: string): boolean {
  return fuzzyScore(text, query) !== null;
}

/**
 * Filter + rank `items` by `query`, best first, matching against the
 * string `toText` returns. Ties keep the input order
 * (`Array.prototype.sort` is stable), so a caller's own ordering —
 * recency, activity — survives an empty or ambiguous query.
 *
 * Match ONE haystack per item. Folding a second one in (a full path
 * next to a name) sounds helpful and isn't: subsequence matching over
 * a long path matches almost any short query, so the list stops
 * narrowing. Pick the haystack the query is aimed at instead — see
 * `ThreadScopeRow`'s `/`-switches-to-paths rule.
 */
export function fuzzyFilter<T>(
  items: readonly T[],
  query: string,
  toText: (item: T) => string,
): T[] {
  const trimmed = query.trim();
  if (trimmed === "") return [...items];

  const scored: { item: T; score: number }[] = [];
  for (const item of items) {
    const score = fuzzyScore(toText(item), trimmed);
    if (score !== null) scored.push({ item, score });
  }

  return scored.sort((a, b) => b.score - a.score).map((entry) => entry.item);
}
