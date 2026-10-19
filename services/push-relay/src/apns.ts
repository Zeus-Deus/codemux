import type { ClientHttp2Session } from "node:http2";
import { importPrivateKey, signJwt } from "./jwt";
import type { Deliver } from "./types";

/** APNs requires HTTP/2, which fetch can't guarantee, so the transport is injectable. */
export type ApnsTransport = (
  url: string,
  headers: Record<string, string>,
  body: string,
) => Promise<{ status: number; body: string }>;

export interface ApnsOptions {
  teamId: string;
  keyId: string;
  /** Contents of the AuthKey_XXXX.p8 file (PKCS8, P-256). */
  privateKeyPem: string;
  send?: ApnsTransport;
}

const PRODUCTION_ORIGIN = "https://api.push.apple.com";
const SANDBOX_ORIGIN = "https://api.sandbox.push.apple.com";
// Apple rejects tokens older than 60 minutes and throttles refreshes faster than 20 minutes.
const PROVIDER_TOKEN_TTL_MS = 50 * 60 * 1000;
const GONE_REASONS = new Set(["BadDeviceToken", "Unregistered"]);

export function createApnsSender(options: ApnsOptions, now: () => number = Date.now): Deliver {
  const send = options.send ?? createHttp2Transport();
  let key: Promise<CryptoKey> | undefined;
  let cached: { jwt: string; issuedAt: number } | undefined;
  let signing: Promise<string> | undefined;

  async function signFresh(t: number): Promise<string> {
    key ??= importPrivateKey(options.privateKeyPem, "ES256");
    const jwt = await signJwt(
      await key,
      { alg: "ES256", kid: options.keyId },
      { iss: options.teamId, iat: Math.floor(t / 1000) },
    );
    cached = { jwt, issuedAt: t };
    return jwt;
  }

  // Concurrent sends share one signing: Apple throttles a connection that
  // presents more than one new provider token per 20 minutes.
  function providerToken(): Promise<string> {
    const t = now();
    if (cached && t - cached.issuedAt < PROVIDER_TOKEN_TTL_MS) return Promise.resolve(cached.jwt);
    signing ??= signFresh(t).finally(() => (signing = undefined));
    return signing;
  }

  return async (target, message) => {
    const origin = target.apnsEnvironment === "sandbox" ? SANDBOX_ORIGIN : PRODUCTION_ORIGIN;
    const headers: Record<string, string> = {
      authorization: `bearer ${await providerToken()}`,
      "apns-topic": target.appId,
      "apns-push-type": "alert",
      "apns-priority": "10",
      // 0 tells APNs to attempt delivery once and not store the notification.
      "apns-expiration": String(
        message.ttlSeconds === 0 ? 0 : Math.floor(now() / 1000) + message.ttlSeconds,
      ),
    };
    if (message.collapseKey) headers["apns-collapse-id"] = message.collapseKey;
    const payload = {
      aps: {
        alert: { title: message.title, body: message.body },
        sound: "default",
        "thread-id": message.data.workspaceId ?? message.category,
        category: message.category,
      },
      codemux: { ...message.data, category: message.category },
    };

    const res = await send(
      `${origin}/3/device/${encodeURIComponent(target.token)}`,
      headers,
      JSON.stringify(payload),
    );
    if (res.status === 200) return { ok: true };
    const reason = parseReason(res.body);
    if (reason === "ExpiredProviderToken") cached = undefined;
    return {
      ok: false,
      gone: res.status === 410 || (reason !== undefined && GONE_REASONS.has(reason)),
      status: res.status,
      reason,
    };
  };
}

function parseReason(body: string): string | undefined {
  try {
    const reason = (JSON.parse(body) as { reason?: unknown }).reason;
    return typeof reason === "string" ? reason : undefined;
  } catch {
    return undefined;
  }
}

/** Default transport: one reused HTTP/2 session per APNs origin. */
export function createHttp2Transport(timeoutMs = 10_000): ApnsTransport {
  const sessions = new Map<string, ClientHttp2Session>();
  return async (url, headers, body) => {
    const http2 = await import("node:http2");
    const { origin, pathname } = new URL(url);
    let session = sessions.get(origin);
    if (!session || session.closed || session.destroyed) {
      const created = http2.connect(origin);
      const forget = () => {
        if (sessions.get(origin) === created) sessions.delete(origin);
      };
      created.on("error", forget);
      created.on("close", forget);
      created.on("goaway", forget);
      sessions.set(origin, created);
      session = created;
    }
    const active = session;
    return new Promise((resolve, reject) => {
      const req = active.request({ ":method": "POST", ":path": pathname, ...headers });
      let status = 0;
      let text = "";
      req.setEncoding("utf8");
      req.setTimeout(timeoutMs, () => req.destroy(new Error("APNs request timed out")));
      req.on("response", (h) => (status = Number(h[":status"])));
      req.on("data", (chunk: string) => (text += chunk));
      req.on("end", () => resolve({ status, body: text }));
      req.on("error", reject);
      req.end(body);
    });
  };
}
