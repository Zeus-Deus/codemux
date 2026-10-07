import { useEffect, useState } from "react";

/** Ids whose entry animation has already played. Composer panels remount
 *  when the user switches tabs or sessions and back, and the entry should
 *  only play when an item first arrives, not on every return to it.
 *  Insertion-ordered and capped so a long-lived window does not grow it
 *  without bound; the oldest ids belong to long-resolved items. */
const easedIds = new Set<string>();
export const EASED_ID_LIMIT = 500;

export function markEased(id: string): void {
  // Re-inserting moves the id to the newest end.
  easedIds.delete(id);
  easedIds.add(id);
  if (easedIds.size > EASED_ID_LIMIT) {
    const oldest = easedIds.values().next();
    if (!oldest.done) easedIds.delete(oldest.value);
  }
}

/** Whether this mount should play its entry animation: true only the first
 *  time `id` is shown. Decided once per mount; later ids are only recorded. */
export function useEaseInOnce(id: string): boolean {
  const [ease] = useState(() => !easedIds.has(id));
  useEffect(() => {
    markEased(id);
  }, [id]);
  return ease;
}
