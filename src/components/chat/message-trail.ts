import type { TranscriptSlot } from "./transcript-slots";

/**
 * Pure helpers for the transcript **navigation trail** (a.k.a. turn rail /
 * jump menu): a slim gutter of tick marks, one per user turn, that previews
 * a turn on hover and jumps to it on click. Kept side-effect-free and
 * exported so turn extraction and downsampling can be unit-tested directly.
 * Runtime visibility is read from LegendList's position model by the rail.
 */

export interface TrailEntry {
  /** Stable id of the user-turn row — used by labels and previews. */
  messageId: string;
  /** Index of this turn's slot in the full `slots` array. */
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
 * Evenly-spaced sample of `[0, count)` bounded to `maxTicks` marks.
 *
 * A very long thread would otherwise render (and try to lay out) hundreds
 * of ticks in a fixed-height gutter — overflowing or shrinking them to
 * invisibility. Downsampling keeps the rail readable at a bounded height;
 * the tradeoff is that not every turn gets its own tick past the cap.
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
 * Last trail entry whose virtual row begins at or above `offset`.
 *
 * LegendList positions are monotonic, so binary search keeps scroll work
 * logarithmic even for transcripts with thousands of turns. Returns `-1`
 * while the viewport is above the first user turn.
 */
export function findTrailEntryAtOffset(
  entries: TrailEntry[],
  offset: number,
  positionAtIndex: (slotIndex: number) => number,
): number {
  let low = 0;
  let high = entries.length - 1;
  let active = -1;

  while (low <= high) {
    const middle = Math.floor((low + high) / 2);
    const rowTop = positionAtIndex(entries[middle].slotIndex);
    if (Number.isFinite(rowTop) && rowTop <= offset) {
      active = middle;
      low = middle + 1;
    } else {
      high = middle - 1;
    }
  }

  return active;
}
