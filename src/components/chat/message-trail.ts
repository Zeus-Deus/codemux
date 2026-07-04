import type { TranscriptSlot } from "./transcript-slots";

/**
 * Pure helpers for the transcript **navigation trail** (a.k.a. turn rail /
 * jump menu): a slim gutter of tick marks, one per user turn, that previews
 * a turn on hover and jumps to it on click. Kept side-effect-free and
 * exported so the turn extraction + active-turn derivation can be
 * unit-tested directly (jsdom can't exercise real scrolling / an
 * IntersectionObserver).
 *
 * The trail derives its active turn from the scroller's `visibleMessageIds`
 * (document-order list of rows intersecting the viewport), NOT from
 * `currentAnchorId`: this transcript sets `scrollAnchor={false}` on every
 * row (the engine's anchor handling breaks the #77 stick-to-bottom pin on
 * hydrated transcripts), so `currentAnchorId` is always `null` here.
 */

export interface TrailEntry {
  /** MessageScroller `messageId` of the user-turn row — the jump target. */
  messageId: string;
  /** Index of this turn's slot in the full `slots` array. Active-turn
   *  tracking compares this against the first visible row's slot index. */
  slotIndex: number;
  /** 0-based ordinal of this user turn (drives labels / aria). */
  turnIndex: number;
  /** Trimmed user prompt text. */
  userText: string;
  /** Trimmed text of the first assistant reply after this turn (before the
   *  next user turn); `""` when the turn has no assistant message yet. */
  replySnippet: string;
}

/**
 * One `TrailEntry` per user-message slot, in document order. The reply
 * snippet is the first `assistant_message` slot that follows the user turn
 * and precedes the next user turn — tool-group / reasoning / tool-call
 * slots in between are skipped so the preview shows prose, not a tool run.
 */
export function buildTrailEntries(slots: TranscriptSlot[]): TrailEntry[] {
  const entries: TrailEntry[] = [];
  let turnIndex = 0;
  for (let i = 0; i < slots.length; i++) {
    const body = slots[i].body;
    if (body.kind !== "item" || body.item.kind !== "user_message") continue;

    let replySnippet = "";
    for (let j = i + 1; j < slots.length; j++) {
      const next = slots[j].body;
      if (next.kind !== "item") continue;
      if (next.item.kind === "user_message") break;
      if (next.item.kind === "assistant_message") {
        replySnippet = next.item.text.trim();
        break;
      }
    }

    entries.push({
      messageId: slots[i].messageId,
      slotIndex: i,
      turnIndex,
      userText: body.item.text.trim(),
      replySnippet,
    });
    turnIndex += 1;
  }
  return entries;
}

/**
 * Index (into `entries`) of the turn currently "in view": the last entry
 * whose slot begins at or above the FIRST visible row (document order).
 *
 * - `-1` when nothing is visible yet, or the viewport sits above the first
 *   user turn (e.g. only the session-start marker / a leading assistant row
 *   is on screen).
 * - Clamps to the last entry once the viewport scrolls past the final turn
 *   (the last entry's slot index stays `<=` any later row's index).
 *
 * `visibleMessageIds` includes non-user rows (assistant rows, folded tool
 * groups keyed `run:<id>`), which is exactly why the lookup routes through
 * `slotIndexById` (messageId → slot index) rather than matching entries by
 * id: the first visible row is usually NOT a user turn.
 */
export function deriveActiveTrailIndex(
  entries: TrailEntry[],
  visibleMessageIds: string[],
  slotIndexById: Map<string, number>,
): number {
  if (entries.length === 0 || visibleMessageIds.length === 0) return -1;

  const firstVisibleSlotIndex = slotIndexById.get(visibleMessageIds[0]);
  if (firstVisibleSlotIndex == null) return -1;

  let active = -1;
  for (let i = 0; i < entries.length; i++) {
    if (entries[i].slotIndex <= firstVisibleSlotIndex) active = i;
    else break;
  }
  return active;
}

/**
 * Evenly-spaced sample of `[0, count)` bounded to `maxTicks` marks.
 *
 * A very long thread would otherwise render (and try to lay out) hundreds
 * of ticks in a fixed-height gutter — overflowing or shrinking them to
 * invisibility. Downsampling keeps the rail readable at a bounded height;
 * the tradeoff is that not every turn gets its own tick past the cap, so
 * the caller re-injects the ACTIVE turn's index (see `withActiveIndex`) to
 * guarantee the in-view turn is always represented.
 *
 * Endpoints are always included (first + last turn), and rounding
 * collisions are de-duplicated so no two ticks map to the same turn.
 */
export function sampleTrailIndices(count: number, maxTicks: number): number[] {
  if (count <= 0) return [];
  if (count <= maxTicks) {
    return Array.from({ length: count }, (_, i) => i);
  }
  const out: number[] = [];
  for (let k = 0; k < maxTicks; k++) {
    const idx = Math.round((k * (count - 1)) / (maxTicks - 1));
    if (out.length === 0 || idx !== out[out.length - 1]) out.push(idx);
  }
  return out;
}

/**
 * Ensure `active` appears in a sampled index list without growing it: if
 * the active turn was dropped by downsampling, swap the nearest sampled
 * index for it, then re-sort. Keeps the tick count bounded while the
 * in-view turn always has a visible (and highlightable) tick.
 */
export function withActiveIndex(sample: number[], active: number): number[] {
  if (active < 0 || sample.length === 0 || sample.includes(active)) {
    return sample;
  }
  let nearest = 0;
  let best = Infinity;
  for (let i = 0; i < sample.length; i++) {
    const d = Math.abs(sample[i] - active);
    if (d < best) {
      best = d;
      nearest = i;
    }
  }
  const copy = sample.slice();
  copy[nearest] = active;
  copy.sort((a, b) => a - b);
  return copy;
}
