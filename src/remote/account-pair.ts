/**
 * Web-remote account-mode admission (Stage A), browser side.
 *
 * The alternative to pasting a pairing code: sign into the desktop's Codemux
 * account. The raw password is stretched to an AuthSecret locally
 * (`auth-derivation.ts`) and only `{email, auth_secret}` is POSTed to the
 * desktop's same-origin `POST /api/pair-account`; the desktop verifies it
 * resolves to its own signed-in account and mints a session identical to the
 * pairing path (`{session_id, session_token, approved}` + cookie).
 *
 * This module holds the network call plus two pieces of pure logic the pairing
 * bootstrap and its tests share: which auth methods a server offers
 * (`resolveAuthMethods` / `initialAuthMode`) and the server-code → friendly-copy
 * mapping (`mapAccountPairError`).
 */

import { deriveAuthSecret } from "./auth-derivation";
import { deriveDeviceName, storeSession, type RemoteSession } from "./session";

/** Result of a successful account sign-in. Same shape as the pairing path so
 *  the bootstrap treats both admission routes uniformly. */
export interface AccountPairResult {
  session: RemoteSession;
  /** `false` when the desktop requires approval for this device (the default
   *  for account sessions) — the bootstrap shows the waiting-for-approval
   *  screen while the transport polls for a ticket. */
  approved: boolean;
}

/** What the `/api/health` probe tells the bootstrap about admission methods. */
export interface ServerInfo {
  version: string | null;
  accountModeEnabled: boolean;
}

/** Which admission methods the pairing screen should offer. A pairing code is
 *  always accepted; account sign-in appears only when the server advertises it. */
export interface AuthMethods {
  account: boolean;
  pairingCode: true;
}

/** Read `/api/health` and report the server version + whether account mode is
 *  on. Never throws — a probe failure degrades to "no account mode, unknown
 *  version" so the pairing-code path always still works. */
export async function fetchServerInfo(
  baseUrl: string,
  fetchImpl: typeof fetch = fetch,
): Promise<ServerInfo> {
  try {
    const res = await fetchImpl(`${baseUrl}/api/health`, {
      credentials: "include",
    });
    if (!res.ok) return { version: null, accountModeEnabled: false };
    const body = (await res.json()) as {
      version?: unknown;
      account_mode_enabled?: unknown;
    };
    return {
      version: typeof body.version === "string" ? body.version : null,
      accountModeEnabled: body.account_mode_enabled === true,
    };
  } catch {
    return { version: null, accountModeEnabled: false };
  }
}

/** Decide which sign-in methods to surface from the server probe. */
export function resolveAuthMethods(info: ServerInfo | null): AuthMethods {
  return { account: info?.accountModeEnabled === true, pairingCode: true };
}

/** The method the pairing screen opens on: account sign-in when it's offered
 *  (the low-friction path account mode exists to provide), else the code box. */
export function initialAuthMode(methods: AuthMethods): "account" | "code" {
  return methods.account ? "account" : "code";
}

/**
 * Map a `/api/pair-account` failure to user-facing copy. The server answers
 * with machine codes (never prose), so the mapping lives here. Codes are
 * non-enumerating — a wrong password and an unknown email both read as the same
 * generic "email or password" message.
 */
export function mapAccountPairError(
  status: number,
  code: string | null,
): string {
  switch (code) {
    case "account_mode_disabled":
      return "Account sign-in isn't enabled on this desktop. Turn on Account access in Remote Access settings, or pair with a code.";
    case "account_signed_out":
      return "Sign in to your Codemux account on the desktop first, then try again.";
    case "account_mismatch":
      return "That account isn't the one signed in on this desktop. Sign in with the desktop's account.";
    case "account_auth_failed":
      return "That email or password is incorrect.";
    case "origin_mismatch":
      return "This device isn't allowed to sign in from here.";
    case "rate_limited":
      return "Too many attempts. Wait a minute and try again.";
    case "invalid_request":
      return "Enter your email and password.";
    default:
      break;
  }
  // Fall back to the status when the body carried no known code.
  if (status === 429) return "Too many attempts. Wait a minute and try again.";
  if (status === 401) return "That email or password is incorrect.";
  if (status === 403) return "This device can't sign in to that account here.";
  return `Sign-in failed (${status}).`;
}

/**
 * Sign into the desktop's Codemux account and mint a web-remote session.
 * Derives the AuthSecret locally (raw password never leaves the browser),
 * POSTs `{email, auth_secret}`, stores the returned session, and reports
 * whether it still needs desktop approval.
 */
export async function pairAccount(
  baseUrl: string,
  email: string,
  password: string,
  fetchImpl: typeof fetch = fetch,
): Promise<AccountPairResult> {
  const authSecret = await deriveAuthSecret(password, email);
  const res = await fetchImpl(`${baseUrl}/api/pair-account`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    credentials: "include",
    body: JSON.stringify({
      email: email.trim(),
      auth_secret: authSecret,
      device_name: deriveDeviceName(),
    }),
  });
  if (!res.ok) {
    let code: string | null = null;
    try {
      const body = (await res.json()) as { error?: unknown };
      if (typeof body.error === "string") code = body.error;
    } catch {
      /* non-JSON error body — fall back to the status-derived message */
    }
    throw new Error(mapAccountPairError(res.status, code));
  }
  const body = (await res.json()) as {
    session_id?: unknown;
    session_token?: unknown;
    approved?: unknown;
  };
  if (
    typeof body.session_id !== "string" ||
    typeof body.session_token !== "string"
  ) {
    throw new Error("The sign-in response was incomplete.");
  }
  const session: RemoteSession = {
    sessionId: body.session_id,
    sessionToken: body.session_token,
  };
  storeSession(session);
  // Missing/undefined `approved` means it's already approved; only an explicit
  // `false` is a pending approval.
  return { session, approved: body.approved !== false };
}
