import { describe, it, expect, vi } from "vitest";
import {
  OAUTH_STATE_KEY,
  clearOAuthParams,
  exchangeAuthCode,
  prepareGithubOAuth,
  readOAuthReturn,
  type OAuthReturnStore,
} from "./hosted-oauth";

/** In-memory store standing in for sessionStorage. */
function memStore(seed: Record<string, string> = {}): OAuthReturnStore & {
  data: Record<string, string>;
} {
  const data: Record<string, string> = { ...seed };
  return {
    data,
    get: (k) => (k in data ? data[k] : null),
    set: (k, v) => {
      data[k] = v;
    },
    remove: (k) => {
      delete data[k];
    },
  };
}

function jsonResponse(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

describe("prepareGithubOAuth", () => {
  it("stores the state and builds the connect URL with all params", () => {
    const store = memStore();
    const { url, state } = prepareGithubOAuth({
      apiBase: "https://api.codemux.org/",
      returnTo: "https://app.codemux.org",
      store,
      randomState: () => "STATE123",
    });
    expect(state).toBe("STATE123");
    expect(store.data[OAUTH_STATE_KEY]).toBe("STATE123");
    const u = new URL(url);
    expect(u.origin + u.pathname).toBe(
      "https://api.codemux.org/api/auth/web/connect",
    );
    expect(u.searchParams.get("provider")).toBe("github");
    expect(u.searchParams.get("state")).toBe("STATE123");
    expect(u.searchParams.get("returnTo")).toBe("https://app.codemux.org");
  });

  it("generates an opaque state matching the API's charset when not injected", () => {
    const store = memStore();
    const { state } = prepareGithubOAuth({
      apiBase: "https://api.codemux.org",
      returnTo: "https://app.codemux.org",
      store,
    });
    expect(state).toMatch(/^[A-Za-z0-9_-]{1,128}$/);
  });
});

describe("readOAuthReturn — CSRF state machine", () => {
  it("returns none when there are no callback params", () => {
    const store = memStore({ [OAUTH_STATE_KEY]: "s1" });
    expect(readOAuthReturn("", store)).toEqual({ kind: "none" });
    // A no-op read must NOT spend the pending state.
    expect(store.data[OAUTH_STATE_KEY]).toBe("s1");
  });

  it("yields the code when the returned state matches the stored one", () => {
    const store = memStore({ [OAUTH_STATE_KEY]: "s1" });
    const ret = readOAuthReturn("?authcode=abc&state=s1", store);
    expect(ret).toEqual({ kind: "code", code: "abc" });
    // State is consumed (one-shot).
    expect(store.get(OAUTH_STATE_KEY)).toBeNull();
  });

  it("rejects a mismatched state (CSRF) and consumes the stored state", () => {
    const store = memStore({ [OAUTH_STATE_KEY]: "s1" });
    const ret = readOAuthReturn("?authcode=abc&state=EVIL", store);
    expect(ret.kind).toBe("error");
    expect(store.get(OAUTH_STATE_KEY)).toBeNull();
  });

  it("rejects when no state was stored (fresh session / forged link)", () => {
    const store = memStore();
    const ret = readOAuthReturn("?authcode=abc&state=s1", store);
    expect(ret.kind).toBe("error");
  });

  it("rejects a return that carries a code but no state", () => {
    const store = memStore({ [OAUTH_STATE_KEY]: "s1" });
    const ret = readOAuthReturn("?authcode=abc", store);
    expect(ret.kind).toBe("error");
    expect(store.get(OAUTH_STATE_KEY)).toBeNull();
  });

  it("a replayed return can't be honored twice (state already spent)", () => {
    const store = memStore({ [OAUTH_STATE_KEY]: "s1" });
    const first = readOAuthReturn("?authcode=abc&state=s1", store);
    expect(first.kind).toBe("code");
    const second = readOAuthReturn("?authcode=abc&state=s1", store);
    expect(second.kind).toBe("error");
  });
});

describe("clearOAuthParams", () => {
  it("removes authcode + state but keeps other query, path, and hash", () => {
    const calls: string[] = [];
    const hist = {
      replaceState: (_d: unknown, _t: string, url: string) => calls.push(url),
    };
    clearOAuthParams(
      {
        href: "https://app.codemux.org/app?authcode=abc&state=s1&hosted=1#frag",
        pathname: "/app",
        hash: "#frag",
      },
      hist,
    );
    expect(calls).toHaveLength(1);
    expect(calls[0]).toBe("/app?hosted=1#frag");
  });

  it("collapses to the root path when only OAuth params were present", () => {
    const calls: string[] = [];
    const hist = {
      replaceState: (_d: unknown, _t: string, url: string) => calls.push(url),
    };
    clearOAuthParams(
      {
        href: "https://app.codemux.org/?authcode=abc&state=s1",
        pathname: "/",
        hash: "",
      },
      hist,
    );
    expect(calls[0]).toBe("/");
  });
});

describe("exchangeAuthCode", () => {
  it("POSTs the code and returns the account session on success", async () => {
    let captured: { url: string; init?: RequestInit } | null = null;
    const fetchImpl = vi.fn(async (url: string, init?: RequestInit) => {
      captured = { url: String(url), init };
      return jsonResponse(200, {
        token: "bearer-xyz",
        expiresAt: "2026-08-01T00:00:00.000Z",
        user: { id: "u1", name: "Me", email: "me@x.com", image: null },
      });
    }) as unknown as typeof fetch;

    const session = await exchangeAuthCode({
      apiBase: "https://api.codemux.org/",
      code: "abc",
      fetchImpl,
    });
    expect(fetchImpl).toHaveBeenCalledTimes(1);
    expect(captured!.url).toBe("https://api.codemux.org/api/auth/web/exchange");
    expect(captured!.init!.method).toBe("POST");
    expect(captured!.init!.credentials).toBe("include");
    expect(JSON.parse(captured!.init!.body as string)).toEqual({ code: "abc" });
    expect(session.token).toBe("bearer-xyz");
    expect(session.user.email).toBe("me@x.com");
  });

  it("throws a friendly error on a non-2xx (expired/used code)", async () => {
    const fetchImpl = vi.fn(async () =>
      jsonResponse(400, { error: "invalid_or_expired_code" }),
    ) as unknown as typeof fetch;
    await expect(
      exchangeAuthCode({ apiBase: "https://api.codemux.org", code: "x", fetchImpl }),
    ).rejects.toThrow(/expired|try again/i);
  });

  it("throws a friendly error on a network failure", async () => {
    const fetchImpl = vi.fn(async () => {
      throw new TypeError("Failed to fetch");
    }) as unknown as typeof fetch;
    await expect(
      exchangeAuthCode({ apiBase: "https://api.codemux.org", code: "x", fetchImpl }),
    ).rejects.toThrow(/connection|try again/i);
  });

  it("throws when the response body has no token", async () => {
    const fetchImpl = vi.fn(async () =>
      jsonResponse(200, { user: { id: "u1" } }),
    ) as unknown as typeof fetch;
    await expect(
      exchangeAuthCode({ apiBase: "https://api.codemux.org", code: "x", fetchImpl }),
    ).rejects.toThrow(/incomplete|try again/i);
  });
});
