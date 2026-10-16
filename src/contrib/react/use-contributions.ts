import { useCallback, useSyncExternalStore } from "react";
import { registry } from "../registry";
import type { Contribution } from "../types";

/** Resolved contributions for one area. Re-renders only when THAT area
 *  mutates; with nothing registered it returns one frozen empty array. */
export function useContributions(area: string): readonly Contribution[] {
  const subscribe = useCallback(
    (onChange: () => void) => registry.subscribeArea(area, onChange),
    [area],
  );
  const getSnapshot = useCallback(() => registry.getArea(area), [area]);
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}

/** `data` payloads of an area, typed. Same stability rules as above. */
export function useContributionData<T>(area: string): readonly { c: Contribution; data: T }[] {
  const items = useContributions(area);
  return useMemoData<T>(items);
}

const dataCache = new WeakMap<readonly Contribution[], readonly { c: Contribution; data: unknown }[]>();
const EMPTY: readonly never[] = Object.freeze([]);

function useMemoData<T>(items: readonly Contribution[]): readonly { c: Contribution; data: T }[] {
  if (items.length === 0) return EMPTY;
  let cached = dataCache.get(items);
  if (!cached) {
    cached = items.filter((c) => c.data !== undefined).map((c) => ({ c, data: c.data }));
    dataCache.set(items, cached);
  }
  return cached as readonly { c: Contribution; data: T }[];
}
