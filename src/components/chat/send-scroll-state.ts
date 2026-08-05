/**
 * Pure state + geometry for the transcript's **new-turn scroll contract**.
 *
 * A composer submission is treated as an explicit navigation intent, not as
 * a data update. Three states describe every position the transcript can be
 * in, and exactly one anchored message identity describes *which* turn is
 * being surfaced:
 *
 * - `following-end` — the default. A hydrated thread opens at the latest row
 *   and stays pinned there while the reader is at the edge.
 * - `anchoring-turn` — entered on send. The optimistic prompt is positioned
 *   `SEND_ANCHOR_OFFSET` below the transcript top and the response streams
 *   into the space reserved beneath it. While the turn fits the viewport the
 *   prompt stays put; once it overflows we advance only far enough to reveal
 *   the growing tail.
 * - `free-scrolling` — entered on a real reader gesture. Live follow is off
 *   and the "Jump to latest" pill is the way back.
 *
 * Everything here is a pure function of a LegendList measurement snapshot, so
 * the geometry is testable without mounting a virtualizer (jsdom has no
 * layout, so it could not be proven any other way).
 */

/** The three positions the transcript scroller can be in. */
export type SendScrollMode =
  | "following-end"
  | "anchoring-turn"
  | "free-scrolling";

/**
 * Distance from the transcript top at which a freshly sent prompt is parked.
 * Doubles as the reserved margin subtracted from the usable viewport, so the
 * anchored row never sits flush against the fade at the top edge.
 */
export const SEND_ANCHOR_OFFSET = 16;

/**
 * A composer submission's navigation intent.
 *
 * `clientNonce` is the same optimistic-send correlation token the reducer
 * already stamps onto the user bubble (`UserMessageItem.clientNonce`), so the
 * anchor resolves to the *exact* row that was just appended — never "the last
 * index", which queued or control rows can follow. `nonce` increments per
 * send so two consecutive sends of identical text are still distinguishable
 * as two separate navigation intents.
 */
export interface SendAnchorRequest {
  readonly clientNonce: string;
  readonly nonce: number;
}

/**
 * Payload LegendList hands `anchoredEndSpace.onReady` once it has measured
 * the anchored row and sized the reserved space. Declared here because the
 * package exports the config type but not this callback's argument type.
 */
export interface SendAnchorReadyInfo {
  readonly anchorIndex: number | undefined;
  readonly anchorKey: string | undefined;
  readonly size: number;
}

/** The subset of LegendList's `getState()` the geometry below reads. */
export interface TranscriptMeasurementState {
  readonly data: readonly unknown[];
  readonly scroll: number;
  readonly scrollLength: number;
  readonly positionAtIndex: (index: number) => number;
  readonly sizeAtIndex: (index: number) => number;
}

/** Measured geometry of the anchored turn against the current viewport. */
export interface AnchoredTurnMetrics {
  /** Content offset of the anchored prompt row. */
  readonly anchorTop: number;
  /** Content offset of the bottom edge of the last real row. */
  readonly lastBottom: number;
  /** Height of everything from the anchored prompt to the live tail. */
  readonly turnHeight: number;
  /** Viewport height minus the reserved top margin. */
  readonly usableViewportHeight: number;
  /** True once the turn no longer fits — the point where we start moving. */
  readonly overflowsUsableViewport: boolean;
  /** Absolute offset that would put the live tail at the viewport bottom. */
  readonly targetScrollToRevealEnd: number;
  /** How far to advance from here. Clamped at 0: this mechanism only ever
   *  scrolls *down*, so it can never drag a reader back up. */
  readonly scrollDeltaToRevealEnd: number;
}

/**
 * Bottom content offset of a measured row, or `null` when the virtualizer has
 * not measured it yet. A 1px floor keeps a not-yet-laid-out row from reading
 * as zero-height and collapsing the turn geometry.
 */
export function getRowBottom(
  state: TranscriptMeasurementState,
  index: number,
): number | null {
  const top = state.positionAtIndex(index);
  const height = state.sizeAtIndex(index);
  if (
    typeof top !== "number" ||
    typeof height !== "number" ||
    !Number.isFinite(top) ||
    !Number.isFinite(height)
  ) {
    return null;
  }
  return top + Math.max(1, height);
}

/**
 * Last-index-wins resolution of the anchored prompt row.
 *
 * Scanning from the tail matters: a rolled-back-then-resent turn, or a
 * hydrate replay that reintroduced an older bubble, can leave two rows
 * carrying the same nonce. The newest one is the one the reader just sent.
 * Returns `null` when the row is not (yet) in the list, which the caller
 * treats as "no reserved space" rather than guessing an index.
 */
export function resolveSendAnchorIndex<Item>(
  items: readonly Item[],
  clientNonce: string | null,
  getClientNonce: (item: Item) => string | null,
): number | null {
  if (!clientNonce) return null;
  for (let index = items.length - 1; index >= 0; index -= 1) {
    const item = items[index];
    if (item !== undefined && getClientNonce(item) === clientNonce) {
      return index;
    }
  }
  return null;
}

/**
 * Measure the anchored turn. `null` when the list is empty or the anchor row
 * has no usable measurement yet — the caller must then do nothing rather than
 * scroll against a guess.
 */
export function getAnchoredTurnMetrics({
  state,
  anchorIndex,
  anchorOffset = SEND_ANCHOR_OFFSET,
}: {
  readonly state: TranscriptMeasurementState;
  readonly anchorIndex: number;
  readonly anchorOffset?: number;
}): AnchoredTurnMetrics | null {
  if (state.data.length === 0) return null;

  const boundedAnchorIndex = Math.max(
    0,
    Math.min(anchorIndex, state.data.length - 1),
  );
  const anchorTop = state.positionAtIndex(boundedAnchorIndex);
  const lastBottom = getRowBottom(state, state.data.length - 1);
  if (
    typeof anchorTop !== "number" ||
    !Number.isFinite(anchorTop) ||
    lastBottom === null
  ) {
    return null;
  }

  const usableViewportHeight = Math.max(0, state.scrollLength - anchorOffset);
  const turnHeight = Math.max(0, lastBottom - anchorTop);
  const targetScrollToRevealEnd = Math.max(0, lastBottom - usableViewportHeight);

  return {
    anchorTop,
    lastBottom,
    turnHeight,
    usableViewportHeight,
    overflowsUsableViewport: turnHeight > usableViewportHeight,
    targetScrollToRevealEnd,
    scrollDeltaToRevealEnd: Math.max(
      0,
      targetScrollToRevealEnd - state.scroll,
    ),
  };
}

/**
 * True when the *real* rows overflow the usable viewport.
 *
 * The anchored end space is blank reserved area, so a plain "scroll to the
 * end" on a short thread would otherwise drive the reader into emptiness.
 * This gate keeps following-end a no-op until there is something below the
 * fold to follow.
 */
export function realContentOverflowsViewport(
  state: TranscriptMeasurementState,
  anchorOffset = SEND_ANCHOR_OFFSET,
): boolean {
  if (state.data.length === 0) return false;
  const lastBottom = getRowBottom(state, state.data.length - 1);
  if (lastBottom === null) return false;
  return lastBottom > Math.max(0, state.scrollLength - anchorOffset);
}
