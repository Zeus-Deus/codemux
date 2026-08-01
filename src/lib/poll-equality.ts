/**
 * Equality gates for polled data.
 *
 * A panel that re-fetches the same git status every 5 or 10 seconds writes a
 * brand-new array into state each tick. The values are identical but the
 * identity is not, so every subscriber re-renders on a heartbeat — the
 * unchanged case, which is the common one, costs the same as a real change.
 *
 * These helpers keep the previous value when the payload is structurally
 * identical, which turns the steady state into a React bail-out. They are
 * for poll results: small, JSON-shaped, and already crossing an IPC boundary
 * as JSON, so a stringify compare is cheap next to the fetch that produced
 * the value.
 */

/** Structural equality via JSON. `undefined` compares equal to itself. */
export function sameJson(a: unknown, b: unknown): boolean {
  if (Object.is(a, b)) return true;
  try {
    return JSON.stringify(a) === JSON.stringify(b);
  } catch {
    // Cyclic or non-serialisable payloads: treat as changed rather than
    // silently pinning stale data on screen.
    return false;
  }
}

/**
 * `next` when it differs from `prev`, otherwise `prev` itself.
 *
 * Intended inside a functional setState so the comparison sees the state
 * React actually holds:
 * `setFiles((prev) => keepIfUnchanged(prev, fetched))`.
 */
export function keepIfUnchanged<T>(prev: T, next: T): T {
  return sameJson(prev, next) ? prev : next;
}
