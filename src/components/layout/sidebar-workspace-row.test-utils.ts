/**
 * Test-only helpers for `sidebar-workspace-row.tsx`.
 *
 * This file is imported exclusively from test files (`*.test.tsx`). Nothing
 * in the app's production entry points (components, hooks, stores, pages)
 * imports it, so Vite's tree-shaking drops the whole module from the prod
 * bundle. Keep it that way: any helper whose only reason to exist is
 * "tests need to reach into module state" belongs here, not in the
 * component file.
 */

import {
  _defaultBranchCache,
  _defaultBranchInFlight,
} from "./default-branch-cache";

/** Drop the module-level default-branch cache between test cases so a
 * prior test's mocked answer doesn't leak into the next one. */
export function __resetDefaultBranchCacheForTests() {
  _defaultBranchCache.clear();
  _defaultBranchInFlight.clear();
}
