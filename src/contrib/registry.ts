import type { Contribution } from "./types";

type Listener = () => void;

const EMPTY: readonly Contribution[] = Object.freeze([]);

/**
 * The one registry every plugin area reads from (port of Hermes Desktop's
 * contrib registry). Snapshots are cached per area and only invalidated on
 * mutation, so the value is referentially stable for `useSyncExternalStore`.
 *
 * Invalidation is area-scoped: registering a palette command never
 * re-renders the title bar. `registerMany`/the returned disposer collapse a
 * batch into one notification per touched area.
 */
class ContributionRegistry {
  private byArea = new Map<string, Contribution[]>();
  private snapshot = new Map<string, readonly Contribution[]>();
  private areaListeners = new Map<string, Set<Listener>>();
  private globalListeners = new Set<Listener>();
  private version = 0;

  /** Register one contribution. Returns a disposer that removes it. */
  register = (c: Contribution): (() => void) => this.registerMany([c]);

  /** Register several at once; the disposer removes all of them. */
  registerMany = (cs: Contribution[]): (() => void) => {
    cs.forEach((c) => this.put(c));
    this.invalidate(cs.map((c) => c.area));
    let disposed = false;
    return () => {
      if (disposed) return;
      disposed = true;
      this.removeMany(cs);
    };
  };

  /** Resolved, sorted, filtered entries for an area. Stable until mutated. */
  getArea = (area: string): readonly Contribution[] => {
    const cached = this.snapshot.get(area);
    if (cached) return cached;
    const raw = this.byArea.get(area);
    const resolved: readonly Contribution[] =
      !raw || raw.length === 0
        ? EMPTY
        : raw
            .filter((c) => {
              if (c.enabled === false) return false;
              if (!c.when) return true;
              try {
                return c.when();
              } catch (error) {
                console.error(`[plugins] ${c.id}: when() threw`, error);
                return false;
              }
            })
            .sort((a, b) => (a.order ?? 0) - (b.order ?? 0));
    const stable = resolved.length === 0 ? EMPTY : resolved;
    this.snapshot.set(area, stable);
    return stable;
  };

  /** Fires on every mutation; prefer `subscribeArea` in React. */
  subscribe = (fn: Listener): (() => void) => {
    this.globalListeners.add(fn);
    return () => {
      this.globalListeners.delete(fn);
    };
  };

  subscribeArea = (area: string, fn: Listener): (() => void) => {
    const set = this.areaListeners.get(area) ?? new Set<Listener>();
    set.add(fn);
    this.areaListeners.set(area, set);
    return () => {
      set.delete(fn);
      if (set.size === 0) this.areaListeners.delete(area);
    };
  };

  /** Monotonic mutation counter for non-React consumers. */
  getVersion = (): number => this.version;

  private removeMany(entries: readonly Contribution[]) {
    const changed: string[] = [];
    for (const entry of entries) {
      if (this.take(entry)) changed.push(entry.area);
    }
    if (changed.length) this.invalidate(changed);
  }

  private put(c: Contribution) {
    const list = this.byArea.get(c.area) ?? [];
    this.byArea.set(c.area, [...list.filter((e) => e.id !== c.id), c]);
  }

  /** Remove exactly this registration. A later same-id replacement survives
   *  the earlier registration's disposer. */
  private take(c: Contribution): boolean {
    const list = this.byArea.get(c.area);
    if (!list) return false;
    const next = list.filter((e) => e !== c);
    if (next.length === list.length) return false;
    if (next.length) this.byArea.set(c.area, next);
    else this.byArea.delete(c.area);
    return true;
  }

  private invalidate(areas: readonly string[]) {
    for (const area of new Set(areas)) {
      this.snapshot.delete(area);
      this.areaListeners.get(area)?.forEach((l) => l());
    }
    this.version += 1;
    this.globalListeners.forEach((l) => l());
  }
}

export const registry = new ContributionRegistry();
