/**
 * GitHub OAuth sign-in for the hosted web client (framework-free, testable).
 *
 * The hosted client can't run the desktop deep-link callback, so it uses a
 * browser-return OAuth flow served by the control plane (api.codemux.org):
 *
 *   1. {@link prepareGithubOAuth} mints an opaque `state`, stores it, and builds
 *      the `GET /api/auth/web/connect` URL. The click handler navigates there.
 *   2. The API runs the GitHub OAuth dance and 302s the browser back to the
 *      hosted origin with `?authcode=<code>&state=<state>` — the long-lived
 *      bearer is deliberately NOT in the URL, only a 60s single-use code.
 *   3. On load, {@link readOAuthReturn} verifies the returned `state` matches the
 *      stored one (CSRF guard) and yields the `code`.
 *   4. {@link exchangeAuthCode} POSTs the code to `/api/auth/web/exchange` and
 *      receives the account bearer in the response BODY.
 *   5. {@link clearOAuthParams} strips `authcode`/`state` from the URL so a
 *      refresh or screenshot can't leak the code.
 *
 * Every side effect (state storage, navigation, `fetch`, `history`) is injected
 * so this module unit-tests with fakes and never touches the real network.
 */

/** sessionStorage key holding the pending OAuth `state` between redirect legs. */
export const OAUTH_STATE_KEY = "codemux.hosted.oauth.state";

const RETURN_ERROR =
  "We couldn't verify that GitHub sign-in. Please try again.";

/** Minimal storage seam (sessionStorage in the browser; a fake in tests). */
export interface OAuthReturnStore {
  get(key: string): string | null;
  set(key: string, value: string): void;
  remove(key: string): void;
}

/** The account bearer + identity returned by a successful code exchange. */
export interface ExchangedSession {
  token: string;
  expiresAt: string;
  user: { id: string; name: string; email: string; image: string | null };
}

/** Outcome of inspecting the return URL. */
export type OAuthReturn =
  | { kind: "none" }
  | { kind: "code"; code: string }
  | { kind: "error"; message: string };

/** A browser-backed {@link OAuthReturnStore}. Degrades to a no-op if
 *  sessionStorage is unavailable (private mode, sandboxed iframe). */
export function sessionOAuthStore(): OAuthReturnStore {
  return {
    get(key) {
      try {
        return window.sessionStorage.getItem(key);
      } catch {
        return null;
      }
    },
    set(key, value) {
      try {
        window.sessionStorage.setItem(key, value);
      } catch {
        /* ignore */
      }
    },
    remove(key) {
      try {
        window.sessionStorage.removeItem(key);
      } catch {
        /* ignore */
      }
    },
  };
}

/** Opaque `state` matching the API's `^[A-Za-z0-9_-]{1,128}$` contract. */
function defaultRandomState(): string {
  const bytes = new Uint8Array(16);
  (globalThis.crypto ?? window.crypto).getRandomValues(bytes);
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function trimBase(apiBase: string): string {
  return apiBase.replace(/\/+$/, "");
}

/**
 * Mint + persist a `state` and build the connect URL. The caller navigates the
 * browser to `url` (a full-page redirect); the persisted state is consumed by
 * {@link readOAuthReturn} on the way back.
 */
export function prepareGithubOAuth(opts: {
  apiBase: string;
  /** Where the API should return to — must be the hosted origin. */
  returnTo: string;
  store: OAuthReturnStore;
  randomState?: () => string;
}): { url: string; state: string } {
  const state = (opts.randomState ?? defaultRandomState)();
  opts.store.set(OAUTH_STATE_KEY, state);
  const url = new URL(`${trimBase(opts.apiBase)}/api/auth/web/connect`);
  url.searchParams.set("provider", "github");
  url.searchParams.set("state", state);
  url.searchParams.set("returnTo", opts.returnTo);
  return { url: url.toString(), state };
}

/**
 * Inspect the return URL's query for an OAuth callback. The stored `state` is
 * consumed (removed) on every call that sees callback params, so a stale state
 * can never satisfy a later attempt. A mismatched or missing state is an error,
 * not a silent pass — that's the CSRF guard.
 */
export function readOAuthReturn(
  search: string,
  store: OAuthReturnStore,
): OAuthReturn {
  const params = new URLSearchParams(search);
  const code = params.get("authcode");
  const state = params.get("state");
  if (code === null && state === null) return { kind: "none" };

  // One-shot: whatever the outcome, the pending state is spent.
  const expected = store.get(OAUTH_STATE_KEY);
  store.remove(OAUTH_STATE_KEY);

  if (code === null || state === null) {
    return { kind: "error", message: RETURN_ERROR };
  }
  if (!expected || state !== expected) {
    return { kind: "error", message: RETURN_ERROR };
  }
  return { kind: "code", code };
}

/**
 * Trade a single-use `authcode` for the account bearer. Never leaks a raw
 * network error — failures surface as friendly, retryable copy.
 */
export async function exchangeAuthCode(opts: {
  apiBase: string;
  code: string;
  fetchImpl?: typeof fetch;
}): Promise<ExchangedSession> {
  const f = opts.fetchImpl ?? ((...a: Parameters<typeof fetch>) => fetch(...a));
  let res: Response;
  try {
    res = await f(`${trimBase(opts.apiBase)}/api/auth/web/exchange`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      credentials: "include",
      body: JSON.stringify({ code: opts.code }),
    });
  } catch {
    throw new Error(
      "Couldn't reach the sign-in service. Check your connection and try again.",
    );
  }
  if (!res.ok) {
    throw new Error("That GitHub sign-in expired. Please sign in again.");
  }
  const body = (await res.json().catch(() => null)) as
    | Partial<ExchangedSession>
    | null;
  if (!body || typeof body.token !== "string") {
    throw new Error("The sign-in response was incomplete. Please try again.");
  }
  return {
    token: body.token,
    expiresAt: typeof body.expiresAt === "string" ? body.expiresAt : "",
    user: {
      id: String(body.user?.id ?? ""),
      name: String(body.user?.name ?? ""),
      email: String(body.user?.email ?? ""),
      image:
        typeof body.user?.image === "string" ? body.user.image : null,
    },
  };
}

/**
 * Strip `authcode` + `state` from the current URL (keeping any other query, the
 * path, and the hash) so the single-use code never survives in history.
 */
export function clearOAuthParams(
  loc: { href: string; pathname: string; hash: string },
  hist: { replaceState(data: unknown, unused: string, url: string): void },
): void {
  let url: URL;
  try {
    url = new URL(loc.href);
  } catch {
    hist.replaceState(null, "", loc.pathname || "/");
    return;
  }
  url.searchParams.delete("authcode");
  url.searchParams.delete("state");
  const query = url.searchParams.toString();
  const next = `${url.pathname}${query ? `?${query}` : ""}${url.hash}`;
  hist.replaceState(null, "", next || "/");
}
