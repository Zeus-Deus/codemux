import { afterEach, describe, expect, it, vi } from "vitest";

import {
  fetchServerInfo,
  initialAuthMode,
  mapAccountPairError,
  pairAccount,
  resolveAuthMethods,
  type ServerInfo,
} from "./account-pair";
import { clearSession, loadSession } from "./session";

// `pairAccount` runs the real (slow) Argon2id derivation; under parallel CPU
// contention a single derivation can exceed the 5s default. Widen the timeout.
vi.setConfig({ testTimeout: 30_000 });

afterEach(() => {
  clearSession();
  vi.restoreAllMocks();
});

// ── Login-screen decision logic ─────────────────────────────────────

describe("resolveAuthMethods / initialAuthMode", () => {
  it("offers account sign-in only when the server advertises account mode", () => {
    const on: ServerInfo = { version: "1.0.0", accountModeEnabled: true };
    const off: ServerInfo = { version: "1.0.0", accountModeEnabled: false };
    expect(resolveAuthMethods(on)).toEqual({ account: true, pairingCode: true });
    expect(resolveAuthMethods(off)).toEqual({
      account: false,
      pairingCode: true,
    });
  });

  it("always keeps the pairing-code path, even with no server info", () => {
    expect(resolveAuthMethods(null)).toEqual({
      account: false,
      pairingCode: true,
    });
  });

  it("opens on the account form when account mode is on, else the code box", () => {
    expect(initialAuthMode({ account: true, pairingCode: true })).toBe(
      "account",
    );
    expect(initialAuthMode({ account: false, pairingCode: true })).toBe("code");
  });
});

// ── /api/health probe ───────────────────────────────────────────────

describe("fetchServerInfo", () => {
  it("reads version + account_mode_enabled from the health payload", async () => {
    const fetchImpl = vi.fn(async () =>
      jsonResponse(200, { ok: true, version: "9.9.9", account_mode_enabled: true }),
    ) as unknown as typeof fetch;
    const info = await fetchServerInfo("http://host:4377", fetchImpl);
    expect(info).toEqual({ version: "9.9.9", accountModeEnabled: true });
  });

  it("degrades to no-account-mode on a probe failure", async () => {
    const fetchImpl = vi.fn(async () => {
      throw new Error("network down");
    }) as unknown as typeof fetch;
    const info = await fetchServerInfo("http://host:4377", fetchImpl);
    expect(info).toEqual({ version: null, accountModeEnabled: false });
  });
});

// ── Error-code → copy mapping (non-enumerating) ─────────────────────

describe("mapAccountPairError", () => {
  it("maps each server code to friendly, non-enumerating copy", () => {
    expect(mapAccountPairError(401, "account_auth_failed")).toMatch(
      /email or password is incorrect/i,
    );
    // A wrong password and an unknown email must read the same (no enumeration).
    expect(mapAccountPairError(403, "account_mismatch")).toMatch(
      /isn't the one signed in/i,
    );
    expect(mapAccountPairError(403, "account_mode_disabled")).toMatch(
      /isn't enabled/i,
    );
    expect(mapAccountPairError(403, "account_signed_out")).toMatch(
      /Sign in to your Codemux account on the desktop/i,
    );
    expect(mapAccountPairError(429, "rate_limited")).toMatch(/Too many/i);
  });

  it("falls back to the status when no known code is present", () => {
    expect(mapAccountPairError(401, null)).toMatch(/incorrect/i);
    expect(mapAccountPairError(500, null)).toMatch(/failed \(500\)/i);
  });
});

// ── pairAccount round-trip incl. pending-approval handling ──────────

describe("pairAccount", () => {
  it("stores the session and reports approved when the desktop trusts it", async () => {
    const fetchImpl = vi.fn(async () =>
      jsonResponse(200, {
        session_id: "sess-acct-1",
        session_token: "tok-acct-1",
        approved: true,
      }),
    ) as unknown as typeof fetch;

    const result = await pairAccount(
      "http://host:4377",
      "user@example.com",
      "hunter2",
      fetchImpl,
    );

    expect(result.approved).toBe(true);
    expect(result.session).toEqual({
      sessionId: "sess-acct-1",
      sessionToken: "tok-acct-1",
    });
    // Session persisted so a refresh doesn't re-prompt.
    expect(loadSession()).toEqual({
      sessionId: "sess-acct-1",
      sessionToken: "tok-acct-1",
    });

    // The wire body carried the derived AuthSecret, never the raw password.
    const init = (fetchImpl as unknown as { mock: { calls: unknown[][] } }).mock
      .calls[0][1] as RequestInit;
    const body = JSON.parse(init.body as string) as {
      auth_secret: string;
      email: string;
    };
    expect(body.email).toBe("user@example.com");
    expect(body.auth_secret).not.toContain("hunter2");
    expect(body.auth_secret.length).toBeGreaterThan(0);
  });

  it("reports pending approval (approved=false) and still stores the session", async () => {
    const fetchImpl = vi.fn(async () =>
      jsonResponse(200, {
        session_id: "sess-acct-2",
        session_token: "tok-acct-2",
        approved: false,
      }),
    ) as unknown as typeof fetch;

    const result = await pairAccount(
      "http://host:4377",
      "user@example.com",
      "hunter2",
      fetchImpl,
    );

    expect(result.approved).toBe(false);
    expect(loadSession()?.sessionId).toBe("sess-acct-2");
  });

  it("throws mapped copy on a rejected sign-in and stores nothing", async () => {
    const fetchImpl = vi.fn(async () =>
      jsonResponse(401, { error: "account_auth_failed" }),
    ) as unknown as typeof fetch;

    await expect(
      pairAccount("http://host:4377", "user@example.com", "wrong", fetchImpl),
    ).rejects.toThrow(/email or password is incorrect/i);
    expect(loadSession()).toBeNull();
  });
});

function jsonResponse(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}
