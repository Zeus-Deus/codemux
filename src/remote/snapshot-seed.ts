/**
 * HTTP bulk-snapshot prefetch for the web-remote client.
 *
 * `GET /api/snapshot` returns the same `AppStateSnapshot` the `get_app_state`
 * command returns, wrapped in a small versioned envelope
 * (`{ api_version, app_state, status }`). The bootstrap fires this
 * **concurrently** with the WS ticket + upgrade so the shim can answer the
 * app's first `get_app_state` from the prefetched snapshot instead of waiting
 * for a socket round-trip after React mounts — the UI paints real state as
 * early as possible.
 *
 * The endpoint is deliberately shaped as the first piece of a versioned API
 * for future non-web (native mobile) clients; see the backend module
 * `src-tauri/src/web_remote/snapshot.rs`.
 *
 * Seeding is a pure optimization: any failure resolves to `null`, and the shim
 * silently falls back to the existing WS `get_app_state` path, which remains
 * the source of truth.
 */

/** Injectable seams for the snapshot fetch (defaults use browser globals). */
export interface SnapshotFetchDeps {
  fetchImpl?: typeof fetch;
  /** Abort the request after this many ms. Kept bounded so a hung fetch never
   *  stalls the seed path — the WS invoke self-heals regardless. */
  timeoutMs?: number;
}

/** The `app_state` value carried by the snapshot envelope. Opaque here — it is
 *  handed back verbatim as the `get_app_state` result, so the shim needs no
 *  knowledge of its shape. */
export type SnapshotSeed = unknown;

const DEFAULT_TIMEOUT_MS = 8000;

/**
 * Fetch `/api/snapshot` and return its `app_state` payload, or `null` on any
 * failure (non-2xx, network error, timeout, malformed/empty body, wrong
 * `api_version`). The returned value is used verbatim as the seed for the
 * first `get_app_state`.
 */
export async function fetchSnapshot(
  baseUrl: string,
  getToken: () => string | null,
  deps: SnapshotFetchDeps = {},
): Promise<SnapshotSeed | null> {
  const fetchImpl = deps.fetchImpl ?? ((...a: Parameters<typeof fetch>) => fetch(...a));
  const base = baseUrl.replace(/\/+$/, "");
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), deps.timeoutMs ?? DEFAULT_TIMEOUT_MS);
  try {
    const token = getToken();
    const headers: Record<string, string> = {};
    if (token) headers.Authorization = `Bearer ${token}`;
    const res = await fetchImpl(`${base}/api/snapshot`, {
      method: "GET",
      headers,
      credentials: "include",
      // Live state — never serve a cached copy.
      cache: "no-store",
      signal: controller.signal,
    });
    if (!res.ok) return null;
    const body = (await res.json()) as {
      api_version?: unknown;
      app_state?: unknown;
    };
    // Only a well-formed, version-matched envelope with an object `app_state`
    // is allowed to seed the store; anything else falls back to the WS path.
    if (
      body == null ||
      typeof body !== "object" ||
      body.api_version !== 1 ||
      body.app_state == null ||
      typeof body.app_state !== "object"
    ) {
      return null;
    }
    return body.app_state;
  } catch {
    // Network error, abort/timeout, or non-JSON body — harmless, fall back.
    return null;
  } finally {
    clearTimeout(timer);
  }
}
