import { useEffect, useState } from "react";
import { getDefaultBranch } from "@/tauri/commands";

// Module-level cache of project_root → default branch name, so each project
// only resolves its default branch once per app session regardless of how
// many workspace rows render it. `null` means "we tried and failed" and is
// cached to avoid retry storms. `inFlight` deduplicates concurrent fetches
// across rows mounting at the same time.
//
// Exported under underscore-prefixed names so the colocated
// `sidebar-workspace-row.test-utils.ts` can reach in and clear them between
// test cases. App code should never touch these directly — use the
// `useDefaultBranch` hook.
export const _defaultBranchCache = new Map<string, string | null>();
export const _defaultBranchInFlight = new Map<string, Promise<string | null>>();

// Triggers a re-render on any subscribed row when `_defaultBranchCache` is
// updated for a new project_root. Keeps the cache logic decoupled from
// React rerender semantics without pulling in a zustand slice.
const defaultBranchListeners = new Set<() => void>();
function notifyDefaultBranchListeners() {
  for (const cb of defaultBranchListeners) cb();
}

/**
 * Resolve the default branch for a project path (shared across all workspace
 * rows that point at the same `project_root`). Returns the cached value
 * synchronously when available, otherwise kicks off a single fetch and
 * re-renders this row when it resolves. Never throws — a missing
 * `origin/HEAD` or detection failure caches `null`, which the caller treats
 * as "unknown" (action still renders but falls through the backend's
 * `Ok(None)` guard). Uses a mount flag so a late-resolving fetch can't
 * setState on an unmounted component (otherwise React logs a warning and
 * — under Vitest's jsdom teardown — crashes with "window is not defined").
 */
export function useDefaultBranch(
  projectRoot: string | null | undefined,
): string | null {
  const [, setTick] = useState(0);

  useEffect(() => {
    if (!projectRoot) return;
    let mounted = true;
    const cb = () => {
      if (mounted) setTick((n) => n + 1);
    };
    defaultBranchListeners.add(cb);

    if (
      !_defaultBranchCache.has(projectRoot) &&
      !_defaultBranchInFlight.has(projectRoot)
    ) {
      const promise = getDefaultBranch(projectRoot)
        .then((branch) => branch || null)
        .catch(() => null)
        .then((result) => {
          _defaultBranchCache.set(projectRoot, result);
          _defaultBranchInFlight.delete(projectRoot);
          notifyDefaultBranchListeners();
          return result;
        });
      _defaultBranchInFlight.set(projectRoot, promise);
    }

    return () => {
      mounted = false;
      defaultBranchListeners.delete(cb);
    };
  }, [projectRoot]);

  if (!projectRoot) return null;
  return _defaultBranchCache.get(projectRoot) ?? null;
}
