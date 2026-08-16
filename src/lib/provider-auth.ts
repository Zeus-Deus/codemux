/**
 * The one cached answer to "can Codemux act on this checkout right now?"
 *
 * Four surfaces ask it — the review panel, the composer's attach menu in
 * both the live and draft chat panes, and the new-workspace preflight —
 * and before this they each asked a *different* question: `gh`'s global
 * login, or whether `gh` was on PATH at all. On a GitLab checkout that
 * produced copy saying "run glab auth login" next to a boolean that was
 * gh's, and on a self-hosted deployment it could not tell one instance's
 * login from another's. `check_provider_auth` resolves the checkout's
 * provider first and probes that provider, scoped to that host.
 *
 * Caching mirrors the review panel's gh-status cache exactly: successes
 * only, one minute. A signed-out or unusable answer is never stored, so
 * the recovery — a `glab auth login`, a `git remote add origin …` — shows
 * up on the very next render rather than a minute later.
 */

import { checkProviderAuth } from "@/tauri/commands";
import type { ProviderAuthStatus, ProviderOperations } from "@/tauri/types";

/** Nothing declared — what an unverified or unreachable host gets.
 *
 *  Also the right answer for a probe that failed: we do not know what
 *  this host can do, and the backend refuses undeclared operations, so
 *  drawing controls on a guess would draw controls that fail. */
export const NO_OPERATIONS: ProviderOperations = {
  list_read: false,
  comment: false,
  approve: false,
  request_changes: false,
  line_comments: false,
  merge_with_strategies: false,
  draft_ready_close_reopen: false,
  checks_status: false,
  timeline: false,
};

/** Everything declared — what the GitHub adapter returns.
 *
 *  Exists for the dev mock and for tests, which stand in for the backend
 *  and therefore have to *be* the declaration. Product code must never
 *  reach for this: the declaration has one home, and it is the adapter. */
export const ALL_OPERATIONS: ProviderOperations = {
  list_read: true,
  comment: true,
  approve: true,
  request_changes: true,
  line_comments: true,
  merge_with_strategies: true,
  draft_ready_close_reopen: true,
  checks_status: true,
  timeline: true,
};

export const PROVIDER_AUTH_TTL_MS = 60_000;

interface CacheEntry {
  value: ProviderAuthStatus;
  ts: number;
}

const cache = new Map<string, CacheEntry>();

/** Keyed by path *and* expected product: a workspace that is re-detected
 *  as a different product must not be served the previous one's answer. */
function cacheKey(path: string, kind: string | null | undefined): string {
  return `${path}|${kind ?? ""}`;
}

/** What a failed probe looks like. Every caller renders this the same way
 *  it renders a genuinely unsupported checkout, so there is no error
 *  state to design. */
export function unusableProviderAuth(): ProviderAuthStatus {
  return {
    kind: "unknown",
    supported: false,
    installed: false,
    authenticated: false,
    username: null,
    operations: NO_OPERATIONS,
  };
}

export function getCachedProviderAuth(
  path: string,
  kind?: string | null,
): ProviderAuthStatus | null {
  const key = cacheKey(path, kind);
  const entry = cache.get(key);
  if (!entry || Date.now() - entry.ts > PROVIDER_AUTH_TTL_MS) {
    cache.delete(key);
    return null;
  }
  return entry.value;
}

/** Stores only a fully usable verdict — see the module docs. */
export function setCachedProviderAuth(
  path: string,
  value: ProviderAuthStatus,
  kind?: string | null,
): void {
  if (!value.authenticated) return;
  cache.set(cacheKey(path, kind), { value, ts: Date.now() });
}

/**
 * Cached host-scoped probe. `kind` is the product the caller already
 * believes serves this checkout (from the workspace snapshot); omit it
 * where nothing is known yet.
 *
 * Never rejects.
 */
export async function fetchProviderAuth(
  path: string,
  kind?: string | null,
): Promise<ProviderAuthStatus> {
  const cached = getCachedProviderAuth(path, kind);
  if (cached) return cached;
  try {
    const fresh = await checkProviderAuth(path);
    setCachedProviderAuth(path, fresh, kind);
    return fresh;
  } catch {
    return unusableProviderAuth();
  }
}

/** Reset the cache — for tests, and for the review panel's own reset. */
export function _resetProviderAuthCache(): void {
  cache.clear();
}
