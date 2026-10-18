// @vitest-environment node
import { beforeAll, describe, expect, it } from "vitest";
import type { ApnsTransport } from "./apns";
import { sha256Hex } from "./jwt";
import { createPushRelay, type PushRelayOptions } from "./relay";
import { createMemoryStore } from "./store";
import { decodeJwt, generateEcKeyPair, generateRsaKeyPair, toPkcs8Pem, verifyJwt } from "./test-helpers";

const APP_ID = "org.codemux.app";
const IOS_TOKEN = "ab".repeat(32);
const FCM_TOKEN = "fcm:registration-token_123";
const TOKEN_URL = "https://oauth2.googleapis.com/token";
const FCM_SEND_URL = "https://fcm.googleapis.com/v1/projects/codemux-test/messages:send";
const START = Date.UTC(2026, 9, 18, 12);

let ec: CryptoKeyPair;
let rsa: CryptoKeyPair;
let ecPem: string;
let rsaPem: string;

beforeAll(async () => {
  [ec, rsa] = await Promise.all([generateEcKeyPair(), generateRsaKeyPair()]);
  [ecPem, rsaPem] = await Promise.all([toPkcs8Pem(ec.privateKey), toPkcs8Pem(rsa.privateKey)]);
});

type ApnsCall = { url: string; headers: Record<string, string>; body: any };
type FetchCall = { url: string; init: RequestInit };

function setup(overrides: Partial<PushRelayOptions> = {}) {
  let clock = START;
  const now = () => clock;
  const store = createMemoryStore({ now });

  const apnsCalls: ApnsCall[] = [];
  type ApnsReply = { status: number; body: string };
  const apnsResponses: Array<ApnsReply | Error | (() => Promise<ApnsReply>)> = [];
  const send: ApnsTransport = async (url, headers, body) => {
    apnsCalls.push({ url, headers, body: JSON.parse(body) });
    const next = apnsResponses.shift() ?? { status: 200, body: "" };
    if (next instanceof Error) throw next;
    if (typeof next === "function") return next();
    return next;
  };

  const fetchCalls: FetchCall[] = [];
  const fcmResponses: Response[] = [];
  let tokensIssued = 0;
  const fetchImpl = (async (input: RequestInfo | URL, init: RequestInit = {}) => {
    const url = String(input);
    fetchCalls.push({ url, init });
    if (url === TOKEN_URL) return Response.json({ access_token: `ya29.token-${++tokensIssued}`, expires_in: 3600 });
    return fcmResponses.shift() ?? Response.json({ name: "projects/codemux-test/messages/1" });
  }) as typeof fetch;

  const relay = createPushRelay({
    basePath: "/api/push",
    store,
    now,
    allowedApps: { ios: [APP_ID], android: [APP_ID] },
    apns: { teamId: "TEAM123456", keyId: "KEY1234567", privateKeyPem: ecPem, send },
    fcm: {
      projectId: "codemux-test",
      clientEmail: "relay@codemux-test.iam.gserviceaccount.com",
      privateKeyPem: rsaPem,
      fetch: fetchImpl,
    },
    ...overrides,
  });

  const call = (method: string, path: string, body?: unknown, headers: Record<string, string> = {}) =>
    relay.handle(
      new Request(`https://api.codemux.org/api/push${path}`, {
        method,
        headers: { "content-type": "application/json", ...headers },
        body: body === undefined ? undefined : typeof body === "string" ? body : JSON.stringify(body),
      }),
    );

  async function register(platform: "ios" | "android" = "ios", extra: Record<string, unknown> = {}) {
    const res = await call("POST", "/v1/devices", {
      platform,
      token: platform === "ios" ? IOS_TOKEN : FCM_TOKEN,
      appId: APP_ID,
      ...extra,
    });
    expect(res.status).toBe(201);
    return (await res.json()) as { deviceId: string; sendKey: string; manageKey: string };
  }

  const notify = (device: { deviceId: string; sendKey: string }, body: unknown = { title: "Approval needed", body: "Run tests?", category: "approval" }) =>
    call("POST", `/v1/devices/${device.deviceId}/notifications`, body, { authorization: `Bearer ${device.sendKey}` });

  return {
    store,
    call,
    register,
    notify,
    apnsCalls,
    apnsResponses,
    fetchCalls,
    fcmResponses,
    advance: (ms: number) => (clock += ms),
  };
}

describe("health and routing", () => {
  it("reports configured platforms with no-store JSON", async () => {
    const res = await setup().call("GET", "/v1/health");
    expect(res.status).toBe(200);
    expect(res.headers.get("content-type")).toBe("application/json");
    expect(res.headers.get("cache-control")).toBe("no-store");
    expect(await res.json()).toEqual({ ok: true, platforms: { ios: true, android: true } });

    const bare = setup({ apns: undefined, fcm: undefined });
    expect(await (await bare.call("GET", "/v1/health")).json()).toEqual({
      ok: true,
      platforms: { ios: false, android: false },
    });
  });

  it("returns 404 for unknown routes and 405 for wrong methods", async () => {
    const t = setup();
    expect((await t.call("GET", "/v2/health")).status).toBe(404);
    expect((await t.call("GET", "/v1/nope")).status).toBe(404);
    const outside = await createPushRelay({ basePath: "/api/push", allowedApps: { ios: [], android: [] } }).handle(
      new Request("https://api.codemux.org/v1/health"),
    );
    expect(outside.status).toBe(404);
    const res = await t.call("GET", "/v1/devices");
    expect(res.status).toBe(405);
    expect(res.headers.get("allow")).toBe("POST");
    expect(await res.json()).toEqual({ error: "method_not_allowed" });
  });

  it("rejects bodies over 16 KiB with 413", async () => {
    const t = setup();
    const res = await t.call("POST", "/v1/devices", { platform: "ios", token: IOS_TOKEN, appId: APP_ID, pad: "x".repeat(17_000) });
    expect(res.status).toBe(413);
    // Streamed body without a content-length header is capped too.
    const streamed = new ReadableStream<Uint8Array>({
      start(controller) {
        for (let i = 0; i < 5; i++) controller.enqueue(new Uint8Array(4096).fill(32));
        controller.close();
      },
    });
    const res2 = await createPushRelay({ allowedApps: { ios: [APP_ID], android: [] } }).handle(
      new Request("https://x/v1/devices", { method: "POST", body: streamed, duplex: "half" } as RequestInit),
    );
    expect(res2.status).toBe(413);
  });
});

describe("registration", () => {
  it("returns a device id and send key, storing only the key hash", async () => {
    const t = setup();
    const { deviceId, sendKey, manageKey } = await t.register("ios");
    expect(deviceId).toMatch(/^[A-Za-z0-9_-]{22}$/);
    expect(sendKey).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(manageKey).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(manageKey).not.toBe(sendKey);
    const record = await t.store.get(deviceId);
    expect(record).toMatchObject({
      platform: "ios",
      token: IOS_TOKEN,
      appId: APP_ID,
      apnsEnvironment: "production",
      sendKeyHash: await sha256Hex(sendKey),
      manageKeyHash: await sha256Hex(manageKey),
    });
    expect(JSON.stringify(record)).not.toContain(sendKey);
    expect(JSON.stringify(record)).not.toContain(manageKey);

    const android = await t.register("android");
    expect((await t.store.get(android.deviceId))?.apnsEnvironment).toBeNull();
    const sandbox = await t.register("ios", { apnsEnvironment: "sandbox" });
    expect((await t.store.get(sandbox.deviceId))?.apnsEnvironment).toBe("sandbox");
  });

  it.each([
    ["bad platform", { platform: "web", token: IOS_TOKEN, appId: APP_ID }],
    ["empty token", { platform: "android", token: "", appId: APP_ID }],
    ["non-hex ios token", { platform: "ios", token: "z".repeat(64), appId: APP_ID }],
    ["short ios token", { platform: "ios", token: "ab", appId: APP_ID }],
    ["oversized android token", { platform: "android", token: "a".repeat(4097), appId: APP_ID }],
    ["missing appId", { platform: "ios", token: IOS_TOKEN }],
    ["apnsEnvironment on android", { platform: "android", token: FCM_TOKEN, appId: APP_ID, apnsEnvironment: "sandbox" }],
    ["unknown apnsEnvironment", { platform: "ios", token: IOS_TOKEN, appId: APP_ID, apnsEnvironment: "dev" }],
    ["non-object body", [1, 2]],
    ["malformed JSON", "{nope"],
  ])("rejects %s with 400 invalid_request", async (_name, body) => {
    const res = await setup().call("POST", "/v1/devices", body);
    expect(res.status).toBe(400);
    expect(await res.json()).toMatchObject({ error: "invalid_request", detail: expect.any(String) });
  });

  it("rejects apps that are not allowed for the platform", async () => {
    const t = setup({ allowedApps: { ios: [APP_ID], android: [] } });
    const res = await t.call("POST", "/v1/devices", { platform: "android", token: FCM_TOKEN, appId: APP_ID });
    expect(res.status).toBe(400);
    expect(await res.json()).toEqual({ error: "unsupported_app" });
  });

  it("rate-limits registrations per client IP", async () => {
    const t = setup({ limits: { registrationsPerHour: 2 } });
    const body = { platform: "ios", token: IOS_TOKEN, appId: APP_ID };
    const fromIp = (ip: string) => t.call("POST", "/v1/devices", body, { "x-forwarded-for": `${ip}, 10.0.0.1` });
    expect((await fromIp("1.1.1.1")).status).toBe(201);
    expect((await fromIp("1.1.1.1")).status).toBe(201);
    const limited = await fromIp("1.1.1.1");
    expect(limited.status).toBe(429);
    expect(limited.headers.get("retry-after")).toBe("3600");
    expect(await limited.json()).toEqual({ error: "rate_limited" });
    expect((await fromIp("2.2.2.2")).status).toBe(201);
    t.advance(60 * 60 * 1000);
    expect((await fromIp("1.1.1.1")).status).toBe(201);
  });

  it("uses a custom client IP resolver", async () => {
    const t = setup({ limits: { registrationsPerHour: 1 }, getClientIp: (r) => r.headers.get("cf-connecting-ip") ?? "?" });
    const body = { platform: "ios", token: IOS_TOKEN, appId: APP_ID };
    expect((await t.call("POST", "/v1/devices", body, { "cf-connecting-ip": "a" })).status).toBe(201);
    expect((await t.call("POST", "/v1/devices", body, { "cf-connecting-ip": "b" })).status).toBe(201);
    expect((await t.call("POST", "/v1/devices", body, { "cf-connecting-ip": "a" })).status).toBe(429);
  });
});

describe("authorization", () => {
  it("returns 401 for missing or wrong keys and 404 for unknown devices", async () => {
    const t = setup();
    const device = await t.register();
    const path = `/v1/devices/${device.deviceId}/notifications`;
    const body = { title: "t", body: "b", category: "test" };
    const missing = await t.call("POST", path, body);
    expect(missing.status).toBe(401);
    expect(await missing.json()).toEqual({ error: "unauthorized" });
    expect((await t.call("POST", path, body, { authorization: `Basic ${device.sendKey}` })).status).toBe(401);
    expect((await t.notify({ ...device, sendKey: "wrong-key" }, body)).status).toBe(401);
    const other = await t.register();
    expect((await t.notify({ deviceId: device.deviceId, sendKey: other.sendKey }, body)).status).toBe(401);
    const unknown = await t.notify({ deviceId: "AAAAAAAAAAAAAAAAAAAAAA", sendKey: device.sendKey }, body);
    expect(unknown.status).toBe(404);
    expect(await unknown.json()).toEqual({ error: "not_found" });
    expect(t.apnsCalls).toHaveLength(0);
  });
});

describe("notification validation", () => {
  const valid = { title: "Done", body: "All green", category: "complete" };
  it.each([
    ["missing title", { ...valid, title: undefined }],
    ["title too long", { ...valid, title: "x".repeat(121) }],
    ["body too long", { ...valid, body: "x".repeat(401) }],
    ["unknown category", { ...valid, category: "spam" }],
    ["collapseKey too long", { ...valid, collapseKey: "k".repeat(65) }],
    ["too many data keys", { ...valid, data: Object.fromEntries(Array.from({ length: 11 }, (_, i) => [`k${i}`, "v"])) }],
    ["data key too long", { ...valid, data: { ["k".repeat(33)]: "v" } }],
    ["reserved data key", { ...valid, data: { from: "x" } }],
    ["non-string data value", { ...valid, data: { n: 1 } }],
    ["data value too long", { ...valid, data: { v: "x".repeat(257) } }],
    ["negative ttl", { ...valid, ttlSeconds: -1 }],
    ["ttl too large", { ...valid, ttlSeconds: 86_401 }],
    ["fractional ttl", { ...valid, ttlSeconds: 1.5 }],
  ])("rejects %s", async (_name, body) => {
    const t = setup();
    const device = await t.register();
    const res = await t.notify(device, body);
    expect(res.status).toBe(400);
    expect(await res.json()).toMatchObject({ error: "invalid_request" });
    expect(t.apnsCalls).toHaveLength(0);
  });

  it("rate-limits sends per device", async () => {
    const t = setup({ limits: { sendsPerHour: 1 } });
    const device = await t.register();
    expect((await t.notify(device)).status).toBe(202);
    const res = await t.notify(device);
    expect(res.status).toBe(429);
    expect(res.headers.get("retry-after")).toBe("3600");
  });
});

describe("APNs delivery", () => {
  it("sends a token-authenticated HTTP/2 request with the expected shape", async () => {
    const t = setup();
    const device = await t.register("ios");
    const res = await t.notify(device, {
      title: "Approval needed",
      body: "Allow npm test?",
      category: "approval",
      collapseKey: "ws-1:approval",
      data: { workspaceId: "ws-1", threadId: "th-9" },
      ttlSeconds: 600,
    });
    expect(res.status).toBe(202);
    expect(await res.json()).toEqual({ delivered: true });

    const [sent] = t.apnsCalls;
    expect(sent.url).toBe(`https://api.push.apple.com/3/device/${IOS_TOKEN}`);
    expect(sent.headers).toEqual({
      authorization: expect.stringMatching(/^bearer /),
      "apns-topic": APP_ID,
      "apns-push-type": "alert",
      "apns-priority": "10",
      "apns-expiration": String(START / 1000 + 600),
      "apns-collapse-id": "ws-1:approval",
    });
    expect(sent.body).toEqual({
      aps: {
        alert: { title: "Approval needed", body: "Allow npm test?" },
        sound: "default",
        "thread-id": "ws-1",
        category: "approval",
      },
      codemux: { category: "approval", workspaceId: "ws-1", threadId: "th-9" },
    });

    const jwt = sent.headers.authorization.slice("bearer ".length);
    const { header, claims } = decodeJwt(jwt);
    expect(header).toEqual({ alg: "ES256", kid: "KEY1234567" });
    expect(claims).toEqual({ iss: "TEAM123456", iat: START / 1000 });
    expect(await verifyJwt(jwt, ec.publicKey)).toBe(true);
  });

  it("targets the sandbox host and omits collapse id when absent", async () => {
    const t = setup();
    const device = await t.register("ios", { apnsEnvironment: "sandbox" });
    expect((await t.notify(device, { title: "t", body: "b", category: "test" })).status).toBe(202);
    expect(t.apnsCalls[0].url).toBe(`https://api.sandbox.push.apple.com/3/device/${IOS_TOKEN}`);
    expect(t.apnsCalls[0].headers).not.toHaveProperty("apns-collapse-id");
    expect(t.apnsCalls[0].body.aps["thread-id"]).toBe("test");
  });

  it("signs one provider token for concurrent sends, cold and after expiry", async () => {
    const t = setup();
    const device = await t.register("ios");
    await Promise.all([t.notify(device), t.notify(device), t.notify(device)]);
    t.advance(51 * 60 * 1000);
    await Promise.all([t.notify(device), t.notify(device)]);
    const tokens = t.apnsCalls.map((call) => call.headers.authorization);
    expect(new Set(tokens.slice(0, 3)).size).toBe(1);
    expect(new Set(tokens.slice(3)).size).toBe(1);
    expect(tokens[3]).not.toBe(tokens[0]);
  });

  it("caches the provider token for 50 minutes", async () => {
    const t = setup();
    const device = await t.register("ios");
    await t.notify(device);
    t.advance(49 * 60 * 1000);
    await t.notify(device);
    t.advance(2 * 60 * 1000);
    await t.notify(device);
    const [a, b, c] = t.apnsCalls.map((call) => call.headers.authorization);
    expect(b).toBe(a);
    expect(c).not.toBe(a);
  });

  it.each([
    ["410 Unregistered", { status: 410, body: '{"reason":"Unregistered","timestamp":1}' }],
    ["400 BadDeviceToken", { status: 400, body: '{"reason":"BadDeviceToken"}' }],
  ])("maps %s to 410 and deletes the device", async (_name, response) => {
    const t = setup();
    const device = await t.register("ios");
    t.apnsResponses.push(response);
    const res = await t.notify(device);
    expect(res.status).toBe(410);
    expect(await res.json()).toEqual({ error: "gone" });
    expect(await t.store.get(device.deviceId)).toBeUndefined();
  });

  it("keeps a device whose token was rotated while a send to the old one failed", async () => {
    const t = setup();
    const device = await t.register("ios");
    const fresh = "b".repeat(64);
    t.apnsResponses.push(async () => {
      await t.store.updateToken(device.deviceId, fresh);
      return { status: 410, body: '{"reason":"Unregistered","timestamp":1}' };
    });
    const res = await t.notify(device);
    expect(res.status).toBe(502);
    expect((await t.store.get(device.deviceId))?.token).toBe(fresh);
  });

  it("maps other upstream failures to 502 and keeps the device", async () => {
    const t = setup();
    const device = await t.register("ios");
    t.apnsResponses.push({ status: 500, body: '{"reason":"InternalServerError"}' }, new Error("socket hang up"));
    for (let i = 0; i < 2; i++) {
      const res = await t.notify(device);
      expect(res.status).toBe(502);
      expect(await res.json()).toEqual({ error: "upstream_failed" });
    }
    expect(await t.store.get(device.deviceId)).toBeDefined();
  });

  it("returns 503 when APNs credentials are not configured", async () => {
    const t = setup({ apns: undefined });
    const device = await t.register("ios");
    const res = await t.notify(device);
    expect(res.status).toBe(503);
    expect(await res.json()).toEqual({ error: "platform_unavailable" });
  });
});

describe("FCM delivery", () => {
  it("fetches an OAuth token via the JWT bearer grant and sends a v1 message", async () => {
    const t = setup();
    const device = await t.register("android");
    const res = await t.notify(device, {
      title: "Input needed",
      body: "Pick a branch",
      category: "input",
      collapseKey: "ws-2",
      data: { workspaceId: "ws-2" },
      ttlSeconds: 120,
    });
    expect(res.status).toBe(202);

    const [tokenCall, sendCall] = t.fetchCalls;
    expect(tokenCall.url).toBe(TOKEN_URL);
    expect(tokenCall.init.method).toBe("POST");
    expect(new Headers(tokenCall.init.headers).get("content-type")).toBe("application/x-www-form-urlencoded");
    const form = new URLSearchParams(String(tokenCall.init.body));
    expect(form.get("grant_type")).toBe("urn:ietf:params:oauth:grant-type:jwt-bearer");
    const assertion = form.get("assertion")!;
    const { header, claims } = decodeJwt(assertion);
    expect(header).toEqual({ alg: "RS256", typ: "JWT" });
    expect(claims).toEqual({
      iss: "relay@codemux-test.iam.gserviceaccount.com",
      scope: "https://www.googleapis.com/auth/firebase.messaging",
      aud: TOKEN_URL,
      iat: START / 1000,
      exp: START / 1000 + 3600,
    });
    expect(await verifyJwt(assertion, rsa.publicKey)).toBe(true);

    expect(sendCall.url).toBe(FCM_SEND_URL);
    expect(new Headers(sendCall.init.headers).get("authorization")).toBe("Bearer ya29.token-1");
    expect(JSON.parse(String(sendCall.init.body))).toEqual({
      message: {
        token: FCM_TOKEN,
        notification: { title: "Input needed", body: "Pick a branch" },
        data: { category: "input", workspaceId: "ws-2" },
        android: {
          // FCM refuses delivery when the token belongs to another package.
          restricted_package_name: APP_ID,
          priority: "HIGH",
          ttl: "120s",
          collapse_key: "ws-2",
          notification: { channel_id: "attention", tag: "ws-2" },
        },
      },
    });
  });

  it("uses the activity channel for non-attention categories", async () => {
    const t = setup();
    const device = await t.register("android");
    await t.notify(device, { title: "PR merged", body: "", category: "pr" });
    const message = JSON.parse(String(t.fetchCalls[1].init.body)).message;
    expect(message.android).toEqual({
      restricted_package_name: APP_ID,
      priority: "HIGH",
      ttl: "3600s",
      notification: { channel_id: "activity" },
    });
  });

  it("caches the access token until 60 seconds before expiry", async () => {
    const t = setup();
    const device = await t.register("android");
    await t.notify(device);
    t.advance(58 * 60 * 1000);
    await t.notify(device);
    t.advance(60 * 1000);
    await t.notify(device);
    const tokenFetches = t.fetchCalls.filter((c) => c.url === TOKEN_URL);
    expect(tokenFetches).toHaveLength(2);
    const auths = t.fetchCalls.filter((c) => c.url === FCM_SEND_URL).map((c) => new Headers(c.init.headers).get("authorization"));
    expect(auths).toEqual(["Bearer ya29.token-1", "Bearer ya29.token-1", "Bearer ya29.token-2"]);
  });

  it.each([
    [
      "UNREGISTERED",
      () =>
        Response.json(
          {
            error: {
              code: 400,
              status: "INVALID_ARGUMENT",
              details: [{ "@type": "type.googleapis.com/google.firebase.fcm.v1.FcmError", errorCode: "UNREGISTERED" }],
            },
          },
          { status: 400 },
        ),
    ],
    ["404", () => Response.json({ error: { code: 404, status: "NOT_FOUND" } }, { status: 404 })],
  ])("maps %s to 410 and deletes the device", async (_name, response) => {
    const t = setup();
    const device = await t.register("android");
    t.fcmResponses.push(response());
    const res = await t.notify(device);
    expect(res.status).toBe(410);
    expect(await t.store.get(device.deviceId)).toBeUndefined();
  });

  it("maps other FCM failures to 502", async () => {
    const t = setup();
    const device = await t.register("android");
    t.fcmResponses.push(new Response("oops", { status: 500 }));
    const res = await t.notify(device);
    expect(res.status).toBe(502);
    expect(await t.store.get(device.deviceId)).toBeDefined();
  });

  it("returns 503 when FCM credentials are not configured", async () => {
    const t = setup({ fcm: undefined });
    const device = await t.register("android");
    expect((await t.notify(device)).status).toBe(503);
  });
});

describe("device management", () => {
  it("rotates the token with PUT", async () => {
    const t = setup();
    const device = await t.register("ios");
    const newToken = "cd".repeat(40);
    const auth = { authorization: `Bearer ${device.manageKey}` };
    // A desktop's send key can't redirect the phone's notifications.
    const asDesktop = { authorization: `Bearer ${device.sendKey}` };
    expect((await t.call("PUT", `/v1/devices/${device.deviceId}`, { token: newToken }, asDesktop)).status).toBe(401);
    t.advance(1000);
    const res = await t.call("PUT", `/v1/devices/${device.deviceId}`, { token: newToken, apnsEnvironment: "sandbox" }, auth);
    expect(res.status).toBe(204);
    expect(res.headers.get("cache-control")).toBe("no-store");
    const record = await t.store.get(device.deviceId);
    expect(record).toMatchObject({ token: newToken, apnsEnvironment: "sandbox" });
    expect(record!.updatedAt.getTime()).toBe(START + 1000);

    expect((await t.call("PUT", `/v1/devices/${device.deviceId}`, { token: "nothex" }, auth)).status).toBe(400);
    expect((await t.call("PUT", `/v1/devices/${device.deviceId}`, { token: newToken }, { authorization: "Bearer nope" })).status).toBe(401);
    await t.notify(device);
    expect(t.apnsCalls[0].url).toBe(`https://api.sandbox.push.apple.com/3/device/${newToken}`);
  });

  it("drops muted categories, set only with the phone's key", async () => {
    const t = setup();
    const device = await t.register("android", { mutedCategories: ["complete"] });
    const finished = { title: "Ready for review", body: "Checkout", category: "complete" };
    expect(await (await t.notify(device, finished)).json()).toEqual({ delivered: false });
    const path = `/v1/devices/${device.deviceId}`;
    const muteAll = { mutedCategories: ["approval", "complete"] };
    expect((await t.call("PUT", path, muteAll, { authorization: `Bearer ${device.sendKey}` })).status).toBe(401);
    expect((await t.call("PUT", path, { mutedCategories: ["test"] }, { authorization: `Bearer ${device.manageKey}` })).status).toBe(400);
    expect((await t.call("PUT", path, { mutedCategories: [] }, { authorization: `Bearer ${device.manageKey}` })).status).toBe(204);
    expect(await (await t.notify(device, finished)).json()).toEqual({ delivered: true });
    expect((await t.call("PUT", path, {}, { authorization: `Bearer ${device.manageKey}` })).status).toBe(400);
  });

  it("changes the APNs environment on its own, keeping the token", async () => {
    const t = setup();
    const device = await t.register("ios");
    const path = `/v1/devices/${device.deviceId}`;
    const auth = { authorization: `Bearer ${device.manageKey}` };
    expect((await t.call("PUT", path, { apnsEnvironment: "sandbox" }, auth)).status).toBe(204);
    expect(await t.store.get(device.deviceId)).toMatchObject({ token: IOS_TOKEN, apnsEnvironment: "sandbox" });
    // Alongside preferences, both apply.
    const both = { apnsEnvironment: "production", mutedCategories: ["pr"] };
    expect((await t.call("PUT", path, both, auth)).status).toBe(204);
    expect(await t.store.get(device.deviceId)).toMatchObject({ apnsEnvironment: "production", mutedCategories: ["pr"] });
    // A bad field rejects the whole update.
    expect((await t.call("PUT", path, { apnsEnvironment: "nope", mutedCategories: [] }, auth)).status).toBe(400);
    expect((await t.store.get(device.deviceId))?.mutedCategories).toEqual(["pr"]);
  });

  it("deletes a device with DELETE", async () => {
    const t = setup();
    const device = await t.register("android");
    const path = `/v1/devices/${device.deviceId}`;
    expect((await t.call("DELETE", path, undefined, { authorization: "Bearer wrong" })).status).toBe(401);
    expect((await t.call("DELETE", path, undefined, { authorization: `Bearer ${device.sendKey}` })).status).toBe(401);
    expect(await t.store.get(device.deviceId)).toBeDefined();
    expect((await t.call("DELETE", path, undefined, { authorization: `Bearer ${device.manageKey}` })).status).toBe(204);
    expect(await t.store.get(device.deviceId)).toBeUndefined();
    expect((await t.call("DELETE", path, undefined, { authorization: `Bearer ${device.manageKey}` })).status).toBe(404);
  });
});
