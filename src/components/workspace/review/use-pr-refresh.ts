import { useCallback, useLayoutEffect, useMemo, useRef } from "react";
import { refreshGithubReadCache } from "@/tauri/commands";
import { toast } from "@/lib/toast";

/** Manual refresh must forget native successes before asking React Query.
 * Mutations already invalidate native reads and keep their existing path. */
export function usePrRefresh(path: string, scope: string, invalidate: () => Promise<unknown>) {
  // A visit, not just a PR number: A → B → A must not adopt A's old reply.
  const visit = useMemo(() => ({ pending: null as Promise<void> | null }), [path, scope]);
  const currentVisit = useRef<typeof visit | null>(visit);
  useLayoutEffect(() => {
    currentVisit.current = visit;
    return () => { currentVisit.current = null; };
  }, [visit]);

  return useCallback(() => {
    if (currentVisit.current !== visit) return Promise.resolve();
    if (visit.pending) return visit.pending;
    visit.pending = (async () => {
      try {
        await refreshGithubReadCache(path);
        if (currentVisit.current !== visit) return;
        await invalidate();
      } catch (error) {
        if (currentVisit.current === visit) {
          toast.error(`Couldn't refresh pull request: ${String(error)}`);
        }
      }
    })().finally(() => { visit.pending = null; });
    return visit.pending;
  }, [path, visit, invalidate]);
}