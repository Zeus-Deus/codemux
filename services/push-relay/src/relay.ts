import { createApnsSender, type ApnsOptions } from "./apns";
import { createFcmSender, type FcmOptions } from "./fcm";
import { constantTimeEqual, randomBase64url, sha256Hex } from "./jwt";
import {
  createMemoryRateLimiter,
  createMemoryStore,
  type DeviceRecord,
  type PushRelayStore,
  type RateLimiter,
} from "./store";
import {
  CATEGORIES,
  type ApnsEnvironment,
  type Category,
  type Deliver,
  type Platform,
  type PushMessage,
} from "./types";

export interface PushRelayLimits {
  /** Device registrations per client IP per hour. Default 20. */
  registrationsPerHour?: number;
  /** Notifications per device per hour. Default 120. */
  sendsPerHour?: number;
}

export interface PushRelayOptions {
  /** Mount prefix stripped before routing, e.g. "/api/push". */
  basePath?: string;
  store?: PushRelayStore;
  rateLimiter?: RateLimiter;
  apns?: ApnsOptions;
  fcm?: FcmOptions;
  /** APNs topics (bundle ids) and Android package names accepted at registration. */
  allowedApps: { ios: string[]; android: string[] };
  now?: () => number;
  /** Only trust x-forwarded-for when the relay sits behind a proxy that sets it. */
  getClientIp?: (request: Request) => string;
  limits?: PushRelayLimits;
}

export interface PushRelay {
  handle(request: Request): Promise<Response>;
}

const MAX_BODY_BYTES = 16 * 1024;
// Keeps the final APNs/FCM payload (with envelope overhead) under the 4 KiB provider limit.
const MAX_CONTENT_BYTES = 3072;
const HOUR_MS = 60 * 60 * 1000;
const DEVICE_ID_RE = /^[A-Za-z0-9_-]{1,64}$/;
const IOS_TOKEN_RE = /^[0-9a-fA-F]{64,200}$/;
const COLLAPSE_KEY_RE = /^[A-Za-z0-9._:-]{1,64}$/;
const DATA_KEY_RE = /^[A-Za-z][A-Za-z0-9_]{0,31}$/;
// FCM rejects these data keys; "category" is set by the relay itself.
const RESERVED_DATA_KEYS = new Set(["from", "notification", "message_type", "collapse_key", "category"]);

class HttpError extends Error {
  constructor(
    readonly status: number,
    readonly body: Record<string, unknown>,
    readonly headers: Record<string, string> = {},
  ) {
    super(String(body.error));
  }
}

const invalid = (detail: string) => new HttpError(400, { error: "invalid_request", detail });
const notFound = () => new HttpError(404, { error: "not_found" });
const unauthorized = () => new HttpError(401, { error: "unauthorized" });

function json(status: number, body: unknown, headers: Record<string, string> = {}): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", "cache-control": "no-store", ...headers },
  });
}

const noContent = () => new Response(null, { status: 204, headers: { "cache-control": "no-store" } });

export function defaultClientIp(request: Request): string {
  return request.headers.get("x-forwarded-for")?.split(",")[0]?.trim() || "unknown";
}

export function createPushRelay(options: PushRelayOptions): PushRelay {
  const now = options.now ?? Date.now;
  const basePath = (options.basePath ?? "").replace(/\/+$/, "");
  const store = options.store ?? createMemoryStore({ now });
  const rateLimiter = options.rateLimiter ?? createMemoryRateLimiter({ now });
  const getClientIp = options.getClientIp ?? defaultClientIp;
  const registrationsPerHour = options.limits?.registrationsPerHour ?? 20;
  const sendsPerHour = options.limits?.sendsPerHour ?? 120;
  const senders: Record<Platform, Deliver | undefined> = {
    ios: options.apns && createApnsSender(options.apns, now),
    android: options.fcm && createFcmSender(options.fcm, now),
  };

  async function enforce(key: string, limit: number): Promise<void> {
    const result = await rateLimiter.hit(key, limit, HOUR_MS);
    if (!result.allowed) {
      throw new HttpError(429, { error: "rate_limited" }, { "retry-after": String(result.retryAfterSeconds) });
    }
  }

  /** `send` is the key desktops hold; `manage` is the phone's own. A
   *  desktop must never be able to redirect the device to another token. */
  async function authorize(request: Request, deviceId: string, key: "send" | "manage"): Promise<DeviceRecord> {
    const match = /^Bearer\s+(\S+)\s*$/i.exec(request.headers.get("authorization") ?? "");
    if (!match) throw unauthorized();
    if (!DEVICE_ID_RE.test(deviceId)) throw notFound();
    const device = await store.get(deviceId);
    if (!device) throw notFound();
    const expected = key === "send" ? device.sendKeyHash : device.manageKeyHash;
    if (!constantTimeEqual(await sha256Hex(match[1]), expected)) throw unauthorized();
    return device;
  }

  async function register(request: Request): Promise<Response> {
    await enforce(`register:${getClientIp(request)}`, registrationsPerHour);
    const body = asObject(await readJson(request));
    const platform = body.platform;
    if (platform !== "ios" && platform !== "android") {
      throw invalid('platform must be "ios" or "android"');
    }
    const token = parseToken(platform, body.token);
    if (typeof body.appId !== "string" || body.appId === "") throw invalid("appId is required");
    if (!options.allowedApps[platform].includes(body.appId)) {
      throw new HttpError(400, { error: "unsupported_app" });
    }
    const apnsEnvironment =
      parseApnsEnvironment(platform, body.apnsEnvironment) ?? (platform === "ios" ? "production" : null);

    const deviceId = randomBase64url(16);
    const sendKey = randomBase64url(32);
    const manageKey = randomBase64url(32);
    const at = new Date(now());
    await store.create({
      deviceId,
      platform,
      token,
      appId: body.appId,
      apnsEnvironment,
      sendKeyHash: await sha256Hex(sendKey),
      manageKeyHash: await sha256Hex(manageKey),
      mutedCategories: body.mutedCategories === undefined ? [] : parseMuted(body.mutedCategories),
      createdAt: at,
      updatedAt: at,
    });
    return json(201, { deviceId, sendKey, manageKey });
  }

  /** Token rotation and/or muted categories; either may be omitted. */
  async function rotate(request: Request, deviceId: string): Promise<Response> {
    const device = await authorize(request, deviceId, "manage");
    const body = asObject(await readJson(request));
    if (body.token === undefined && body.mutedCategories === undefined) {
      throw invalid("token or mutedCategories is required");
    }
    const muted = body.mutedCategories === undefined ? undefined : parseMuted(body.mutedCategories);
    if (body.token !== undefined) {
      const token = parseToken(device.platform, body.token);
      const apnsEnvironment = parseApnsEnvironment(device.platform, body.apnsEnvironment);
      await store.updateToken(device.deviceId, token, apnsEnvironment);
    }
    if (muted) await store.setMutedCategories(device.deviceId, muted);
    return noContent();
  }

  async function remove(request: Request, deviceId: string): Promise<Response> {
    const device = await authorize(request, deviceId, "manage");
    await store.delete(device.deviceId);
    return noContent();
  }

  async function notify(request: Request, deviceId: string): Promise<Response> {
    const device = await authorize(request, deviceId, "send");
    const deliver = senders[device.platform];
    if (!deliver) throw new HttpError(503, { error: "platform_unavailable" });
    await enforce(`send:${device.deviceId}`, sendsPerHour);
    const message = parseNotification(await readJson(request));
    // Muted by the phone: accepted, not delivered. A test always goes out.
    if (message.category !== "test" && device.mutedCategories.includes(message.category)) {
      return json(202, { delivered: false });
    }

    let result;
    try {
      result = await deliver(device, message);
    } catch {
      throw new HttpError(502, { error: "upstream_failed" });
    }
    if (result.ok) return json(202, { delivered: true });
    if (result.gone) {
      // The phone may have rotated its token while this send was in flight;
      // the fresh token stays, and the desktop keeps its handle.
      if (await store.deleteIfToken(device.deviceId, device.token)) {
        throw new HttpError(410, { error: "gone" });
      }
      throw new HttpError(502, { error: "upstream_failed" });
    }
    throw new HttpError(502, { error: "upstream_failed" });
  }

  function health(): Response {
    return json(200, { ok: true, platforms: { ios: !!senders.ios, android: !!senders.android } });
  }

  type Handler = () => Response | Promise<Response>;
  function byMethod(request: Request, handlers: Record<string, Handler>): Response | Promise<Response> {
    const handler = handlers[request.method];
    if (!handler) {
      throw new HttpError(405, { error: "method_not_allowed" }, { allow: Object.keys(handlers).join(", ") });
    }
    return handler();
  }

  function route(request: Request): Response | Promise<Response> {
    let path = new URL(request.url).pathname;
    if (basePath) {
      if (path !== basePath && !path.startsWith(`${basePath}/`)) throw notFound();
      path = path.slice(basePath.length);
    }
    const segments = path.split("/").filter(Boolean);
    const [version, resource, deviceId, sub] = segments;
    if (version !== "v1") throw notFound();
    if (resource === "health" && segments.length === 2) return byMethod(request, { GET: health });
    if (resource === "devices") {
      if (segments.length === 2) return byMethod(request, { POST: () => register(request) });
      if (segments.length === 3) {
        return byMethod(request, {
          PUT: () => rotate(request, deviceId),
          DELETE: () => remove(request, deviceId),
        });
      }
      if (segments.length === 4 && sub === "notifications") {
        return byMethod(request, { POST: () => notify(request, deviceId) });
      }
    }
    throw notFound();
  }

  return {
    async handle(request) {
      try {
        return await route(request);
      } catch (error) {
        if (error instanceof HttpError) return json(error.status, error.body, error.headers);
        console.error("push-relay: unexpected error", error);
        return json(500, { error: "internal" });
      }
    },
  };
}

async function readJson(request: Request): Promise<unknown> {
  const tooLarge = () => new HttpError(413, { error: "payload_too_large" });
  if (Number(request.headers.get("content-length")) > MAX_BODY_BYTES) throw tooLarge();
  const chunks: Uint8Array[] = [];
  let size = 0;
  if (request.body) {
    const reader = request.body.getReader();
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.byteLength;
      if (size > MAX_BODY_BYTES) {
        await reader.cancel();
        throw tooLarge();
      }
      chunks.push(value);
    }
  }
  const bytes = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  try {
    return JSON.parse(new TextDecoder().decode(bytes));
  } catch {
    throw invalid("body must be valid JSON");
  }
}

function asObject(value: unknown): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw invalid("body must be a JSON object");
  }
  return value as Record<string, unknown>;
}

function parseToken(platform: Platform, token: unknown): string {
  if (typeof token !== "string" || token === "" || token.length > 4096) {
    throw invalid("token must be a non-empty string of at most 4096 characters");
  }
  if (platform === "ios" && !IOS_TOKEN_RE.test(token)) {
    throw invalid("iOS token must be 64-200 hex characters");
  }
  if (platform === "android" && /\s/.test(token)) throw invalid("token must not contain whitespace");
  return token;
}

function parseApnsEnvironment(platform: Platform, value: unknown): ApnsEnvironment | undefined {
  if (value === undefined) return undefined;
  if (platform !== "ios") throw invalid("apnsEnvironment is only valid for ios");
  if (value !== "sandbox" && value !== "production") {
    throw invalid('apnsEnvironment must be "sandbox" or "production"');
  }
  return value;
}

function parseMuted(value: unknown): Category[] {
  if (!Array.isArray(value) || !value.every((c) => CATEGORIES.includes(c as Category) && c !== "test")) {
    throw invalid("mutedCategories must list notification categories other than test");
  }
  return [...new Set(value as Category[])];
}

function parseNotification(value: unknown): PushMessage {
  const body = asObject(value);
  if (typeof body.title !== "string" || body.title === "" || body.title.length > 120) {
    throw invalid("title must be a non-empty string of at most 120 characters");
  }
  if (typeof body.body !== "string" || body.body.length > 400) {
    throw invalid("body must be a string of at most 400 characters");
  }
  if (!CATEGORIES.includes(body.category as Category)) {
    throw invalid(`category must be one of ${CATEGORIES.join(", ")}`);
  }
  if (body.collapseKey !== undefined && (typeof body.collapseKey !== "string" || !COLLAPSE_KEY_RE.test(body.collapseKey))) {
    throw invalid("collapseKey must be 1-64 characters of [A-Za-z0-9._:-]");
  }
  const data: Record<string, string> = {};
  if (body.data !== undefined) {
    if (typeof body.data !== "object" || body.data === null || Array.isArray(body.data)) {
      throw invalid("data must be an object of string values");
    }
    const entries = Object.entries(body.data);
    if (entries.length > 10) throw invalid("data must have at most 10 keys");
    for (const [key, val] of entries) {
      if (!DATA_KEY_RE.test(key) || RESERVED_DATA_KEYS.has(key) || /^(google|gcm)/i.test(key)) {
        throw invalid(`data key "${key.slice(0, 32)}" is invalid or reserved`);
      }
      if (typeof val !== "string" || val.length > 256) {
        throw invalid(`data.${key} must be a string of at most 256 characters`);
      }
      data[key] = val;
    }
  }
  let ttlSeconds = 3600;
  if (body.ttlSeconds !== undefined) {
    if (typeof body.ttlSeconds !== "number" || !Number.isInteger(body.ttlSeconds) || body.ttlSeconds < 0 || body.ttlSeconds > 86_400) {
      throw invalid("ttlSeconds must be an integer between 0 and 86400");
    }
    ttlSeconds = body.ttlSeconds;
  }
  const message: PushMessage = {
    title: body.title,
    body: body.body,
    category: body.category as Category,
    data,
    ttlSeconds,
  };
  if (body.collapseKey !== undefined) message.collapseKey = body.collapseKey as string;
  const contentBytes = new TextEncoder().encode(JSON.stringify([message.title, message.body, data])).byteLength;
  if (contentBytes > MAX_CONTENT_BYTES) {
    throw invalid(`title, body and data together exceed ${MAX_CONTENT_BYTES} bytes`);
  }
  return message;
}
