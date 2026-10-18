import { importPrivateKey, signJwt } from "./jwt";
import type { Deliver } from "./types";

export interface FcmOptions {
  projectId: string;
  /** `client_email` from the Firebase service-account JSON. */
  clientEmail: string;
  /** `private_key` from the service-account JSON (PKCS8 RSA PEM). */
  privateKeyPem: string;
  fetch?: typeof fetch;
}

const TOKEN_URL = "https://oauth2.googleapis.com/token";
const SCOPE = "https://www.googleapis.com/auth/firebase.messaging";
const REFRESH_MARGIN_MS = 60_000;

export function createFcmSender(options: FcmOptions, now: () => number = Date.now): Deliver {
  const fetchImpl = options.fetch ?? ((input, init) => fetch(input, init));
  const sendUrl = `https://fcm.googleapis.com/v1/projects/${encodeURIComponent(options.projectId)}/messages:send`;
  let key: Promise<CryptoKey> | undefined;
  let cached: { token: string; expiresAt: number } | undefined;
  let inflight: Promise<string> | undefined;

  async function fetchAccessToken(): Promise<string> {
    key ??= importPrivateKey(options.privateKeyPem, "RS256");
    const iat = Math.floor(now() / 1000);
    const assertion = await signJwt(
      await key,
      { alg: "RS256", typ: "JWT" },
      { iss: options.clientEmail, scope: SCOPE, aud: TOKEN_URL, iat, exp: iat + 3600 },
    );
    const res = await fetchImpl(TOKEN_URL, {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({
        grant_type: "urn:ietf:params:oauth:grant-type:jwt-bearer",
        assertion,
      }).toString(),
    });
    if (!res.ok) throw new Error(`FCM OAuth token request failed with ${res.status}`);
    const json = (await res.json()) as { access_token?: unknown; expires_in?: unknown };
    if (typeof json.access_token !== "string") throw new Error("FCM OAuth response lacks access_token");
    const expiresIn = typeof json.expires_in === "number" ? json.expires_in : 3600;
    cached = { token: json.access_token, expiresAt: now() + expiresIn * 1000 };
    return cached.token;
  }

  function accessToken(): Promise<string> {
    if (cached && now() < cached.expiresAt - REFRESH_MARGIN_MS) return Promise.resolve(cached.token);
    inflight ??= fetchAccessToken().finally(() => (inflight = undefined));
    return inflight;
  }

  return async (target, message) => {
    const android: Record<string, unknown> = {
      // Only the allowlisted package may receive it, whatever the token.
      restricted_package_name: target.appId,
      priority: "HIGH",
      ttl: `${message.ttlSeconds}s`,
      notification: {
        channel_id:
          message.category === "approval" || message.category === "input" ? "attention" : "activity",
        ...(message.collapseKey ? { tag: message.collapseKey } : {}),
      },
    };
    if (message.collapseKey) android.collapse_key = message.collapseKey;
    const res = await fetchImpl(sendUrl, {
      method: "POST",
      headers: {
        authorization: `Bearer ${await accessToken()}`,
        "content-type": "application/json",
      },
      body: JSON.stringify({
        message: {
          token: target.token,
          notification: { title: message.title, body: message.body },
          data: { ...message.data, category: message.category },
          android,
        },
      }),
    });
    if (res.ok) return { ok: true };
    const errorCode = parseErrorCode(await res.text());
    if (res.status === 401) cached = undefined;
    return {
      ok: false,
      gone: res.status === 404 || errorCode === "UNREGISTERED",
      status: res.status,
      reason: errorCode,
    };
  };
}

function parseErrorCode(body: string): string | undefined {
  try {
    const details = (JSON.parse(body) as { error?: { details?: unknown } }).error?.details;
    if (!Array.isArray(details)) return undefined;
    for (const detail of details) {
      const code = (detail as { errorCode?: unknown } | null)?.errorCode;
      if (typeof code === "string") return code;
    }
  } catch {
    // Non-JSON error body; fall through.
  }
  return undefined;
}
