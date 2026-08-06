import { useSyncExternalStore } from "react";

/**
 * A shared "a card is already up" phase for hover cards that live in a list.
 *
 * A hover card's open delay exists to stop a card flashing while the pointer
 * merely sweeps past a row. Once one card IS up, that reasoning is spent: the
 * user has clearly settled into reading cards, and every subsequent row paying
 * the delay again reads as the card lagging behind the cursor. So the first
 * card in a run costs the full delay and the rest open instantly, with the
 * phase surviving `GROUP_TIMEOUT_MS` past the last close so crossing a gap
 * between two rows does not reset the user back to square one.
 *
 * Radix's `HoverCard` has no provider to hang this on (only its `Tooltip`
 * does), hence a module-level store: every card on screen shares one phase,
 * which is exactly the grouping semantics wanted.
 */
const GROUP_TIMEOUT_MS = 400;

let active = false;
/** Cards currently open. The phase cannot expire while one is still up. */
let openCount = 0;
/** The most recently opened card's "close yourself" callback, or null. */
let supersede: (() => void) | null = null;
let expiryTimer = 0;
const listeners = new Set<() => void>();

function emit() {
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/**
 * Whether a hover card opened recently enough that the next one should skip
 * its delay. Read this at the moment a card opens — the `use*` hook below is
 * for rendering the delay prop, this is for the imperative decision.
 */
export function isHoverCardGroupActive(): boolean {
  return active;
}

/**
 * Mark a card as open for as long as the returned function is uncalled. Call
 * from an effect so an unmount mid-hover (a workspace archived out of the
 * sidebar under the pointer) cannot strand the counter above zero.
 *
 * `onSuperseded` closes this card the moment a different one opens. Cards keep
 * a close delay so the pointer can cross the offset gap into the card and
 * select text — but that delay must not apply when the pointer has moved to
 * ANOTHER row, because the new card now opens instantly and the two would
 * overlap on screen. Superseding collapses that window to nothing.
 */
export function registerOpenHoverCard(onSuperseded: () => void): () => void {
  const previous = supersede;
  supersede = onSuperseded;
  openCount += 1;
  clearTimeout(expiryTimer);
  if (!active) {
    active = true;
    emit();
  }
  // After this card is counted, so retiring the previous one cannot briefly
  // drop the group to zero and schedule an expiry mid-sweep.
  previous?.();

  let released = false;
  return () => {
    if (released) return;
    released = true;
    if (supersede === onSuperseded) supersede = null;
    openCount = Math.max(0, openCount - 1);
    if (openCount > 0) return;
    clearTimeout(expiryTimer);
    expiryTimer = window.setTimeout(() => {
      active = false;
      emit();
    }, GROUP_TIMEOUT_MS);
  };
}

/** Subscribing form, for deriving a card's `openDelay` prop. */
export function useHoverCardGroupActive(): boolean {
  return useSyncExternalStore(
    subscribe,
    isHoverCardGroupActive,
    isHoverCardGroupActive,
  );
}

export function __resetHoverCardGroupForTests(): void {
  clearTimeout(expiryTimer);
  expiryTimer = 0;
  openCount = 0;
  active = false;
  supersede = null;
  listeners.clear();
}
