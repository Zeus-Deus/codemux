/**
 * Structural sharing for the `AppStateSnapshot` that the Rust backend emits
 * over Tauri IPC.
 *
 * WHY THIS EXISTS — re-render economics:
 *
 * The Rust backend rebuilds the ENTIRE `AppStateSnapshot` (workspaces,
 * surfaces, recursive pane trees, tabs, session lists, port lists, …) on
 * every `emit_app_state` and ships it to the renderer. Because it is
 * JSON-deserialized on arrival, EVERY object and array in the tree has a
 * fresh reference on every tick — even the parts that did not change. Agent
 * token streaming alone fires this up to ~60 times per second, on top of the
 * 5-second git poll, hook events, and PR/port refreshes.
 *
 * Zustand fans out to subscribers by reference identity: a selector like
 * `useActiveWorkspace()` returns the workspace OBJECT, and if that object is a
 * fresh ref every tick, every consumer re-renders every tick — cascading into
 * the whole pane tree, markdown re-parsing, tab bars, etc. Selecting a fresh
 * top-level snapshot ref with `set({ appState })` therefore invalidates
 * everything ~60x/sec even when nothing semantically changed.
 *
 * `shareStructural(prev, next)` reconciles the incoming `next` snapshot against
 * the `prev` one we already hold: wherever a subtree is deep-equal to what we
 * had, it REUSES the previous reference. Unchanged workspaces, surfaces, and
 * pane subtrees keep their identity, so selector subscribers see zero fan-out
 * on no-op ticks, and `React.memo` boundaries on the pane tree stay effective.
 *
 * WHAT THIS IS NOT — the removed stringify dedup:
 *
 * A previous attempt did a full-snapshot `JSON.stringify` dedup to catch the
 * identical-payload case. It was REMOVED for causing freezes: workspace state
 * grows with every opened surface/pane/session, so the serialize cost scaled
 * with usage and ran on every tick (see the comment in
 * `src/hooks/use-app-state.ts`). This walk is deliberately different: it
 * allocates NO strings, runs in a single O(n) pass over the tree, and reuses
 * refs at every level. It runs at most once per 16ms debounce window.
 *
 * CONTRACT:
 *   - The return value is ALWAYS deep-equal to `next` — data correctness is
 *     always `next`'s. We only ever swap in a `prev` reference when that ref is
 *     already deep-equal to the corresponding `next` value.
 *   - Inputs are never mutated.
 *   - `Object.is(prev, next)` returns `prev` immediately (also covers the dev
 *     mock, which mutates nested objects in place and re-emits the same refs).
 *   - Only plain objects (null-prototype or `Object.prototype`) and arrays are
 *     recursed into; everything else is compared with `Object.is`.
 */

function isPlainObject(value: unknown): value is Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return false;
  }
  const proto = Object.getPrototypeOf(value);
  return proto === Object.prototype || proto === null;
}

/**
 * Reconcile `next` against `prev`, reusing `prev`'s references wherever the
 * corresponding subtree is deep-equal. See the module doc comment for why.
 *
 * @returns a value deep-equal to `next`; reference-equal to `prev` when the two
 *          are deep-equal, and reference-equal to `next`'s children wherever
 *          they differ from `prev`.
 */
export function shareStructural<T>(prev: T, next: T): T {
  // Fast path: same reference (also the dev-mock in-place-mutation case).
  if (Object.is(prev, next)) return prev;

  const nextIsArray = Array.isArray(next);
  const prevIsArray = Array.isArray(prev);

  if (nextIsArray || prevIsArray) {
    // Type mismatch (array vs non-array) → can't share, take next.
    if (!nextIsArray || !prevIsArray) return next;

    const prevArr = prev as unknown[];
    const nextArr = next as unknown[];
    const len = nextArr.length;
    // Most backend ticks are unchanged. Allocate a replacement only once a
    // difference is found, retaining the already-reconciled prefix from prev.
    let reconciled: unknown[] | null = prevArr.length === len ? null : new Array(len);
    for (let i = 0; i < len; i++) {
      const child = shareStructural(prevArr[i], nextArr[i]);
      if (reconciled === null && !Object.is(child, prevArr[i])) {
        reconciled = prevArr.slice(0, i);
      }
      if (reconciled !== null) reconciled[i] = child;
    }
    return (reconciled ?? prev) as T;
  }

  if (isPlainObject(next) && isPlainObject(prev)) {
    const prevObj = prev as Record<string, unknown>;
    const nextObj = next as Record<string, unknown>;
    const nextKeys = Object.keys(nextObj);
    const prevKeys = Object.keys(prevObj);
    const prototype = Object.getPrototypeOf(nextObj);
    let reconciled: Record<string, unknown> | null =
      nextKeys.length === prevKeys.length && Object.getPrototypeOf(prevObj) === prototype
        ? null
        : { ...nextObj };
    for (let i = 0; i < nextKeys.length; i++) {
      const key = nextKeys[i];
      const hadKey = Object.prototype.hasOwnProperty.call(prevObj, key);
      const child = hadKey ? shareStructural(prevObj[key], nextObj[key]) : nextObj[key];
      if (reconciled === null && (!hadKey || !Object.is(child, prevObj[key]))) {
        // Spread defines own data properties, including a literal __proto__
        // key from JSON, rather than invoking Object.prototype's setter.
        reconciled = { ...nextObj };
        for (let j = 0; j < i; j++) {
          const sharedKey = nextKeys[j];
          reconciled[sharedKey] = prevObj[sharedKey];
        }
      }
      if (reconciled !== null) reconciled[key] = child;
    }
    if (reconciled !== null && prototype === null) Object.setPrototypeOf(reconciled, null);
    return (reconciled ?? prev) as T;
  }

  // Primitives, type mismatch, null, or non-plain objects → compare by
  // identity; since Object.is failed above, take next.
  return next;
}
