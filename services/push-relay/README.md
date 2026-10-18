# Codemux push relay

Desktops can't hold APNs/FCM credentials, so the shared API server (`api.codemux.org`, Bun + Hono, separate infra) owns them and relays notifications to the Codemux phone app. This directory is the portable reference implementation: a framework-agnostic `fetch`-style handler with no runtime dependencies (WebCrypto, `fetch`, and `node:http2` for APNs).

## Contract (v1)

Paths are relative to `basePath`. All JSON responses carry `content-type: application/json` and `cache-control: no-store`. Bodies over 16 KiB → 413; unknown routes → 404; wrong method → 405.

| Method & path | Caller | Auth | Body | Success | Errors |
| --- | --- | --- | --- | --- | --- |
| `POST /v1/devices` | phone | none (rate-limited per IP, 20/h) | `{platform, token, appId, apnsEnvironment?, mutedCategories?}` | `201 {deviceId, sendKey, manageKey}` | 400 `invalid_request` / `unsupported_app`, 429 |
| `PUT /v1/devices/:id` | phone | `Bearer <manageKey>` | `{token?, apnsEnvironment?, mutedCategories?}` (at least one) | 204 | 400, 401, 404 |
| `DELETE /v1/devices/:id` | phone | `Bearer <manageKey>` | — | 204 | 401, 404 |
| `POST /v1/devices/:id/notifications` | desktop | `Bearer <sendKey>` | `{title≤120, body≤400, category, collapseKey?≤64, data?, ttlSeconds?}` | `202 {delivered:true}` | 400, 401, 404, 410 `gone` (device deleted), 429 (120/h per device), 502 `upstream_failed`, 503 `platform_unavailable` |
| `GET /v1/health` | anyone | none | — | `200 {ok, platforms:{ios, android}}` | — |

`category` is one of `approval | input | complete | failure | pr | test`. A category the phone muted (`mutedCategories`, any but `test`) is accepted with `202 {delivered:false}` and not sent, so an opt-out holds while the desktops are out of the phone's reach. `data` is ≤10 string entries (keys `[A-Za-z][A-Za-z0-9_]{0,31}`, values ≤256 chars, FCM-reserved keys rejected); `data.workspaceId` becomes the iOS `thread-id`. `collapseKey` is restricted to `[A-Za-z0-9._:-]` because it becomes an HTTP header.

## Mounting (Hono)

```ts
import { createPushRelay } from "./push-relay/src";

const relay = createPushRelay({
  basePath: "/api/push",
  store: drizzleStore, // implements PushRelayStore against schema.sql
  allowedApps: { ios: ["org.codemux.app"], android: ["org.codemux.app"] },
  apns: { teamId: env.APNS_TEAM_ID, keyId: env.APNS_KEY_ID, privateKeyPem: env.APNS_KEY_P8 },
  fcm: { projectId: sa.project_id, clientEmail: sa.client_email, privateKeyPem: sa.private_key },
  getClientIp: (req) => req.headers.get("x-real-ip") ?? "unknown",
});

app.all("/api/push/*", (c) => relay.handle(c.req.raw));
```

## Credentials

- **APNs**: an Apple Developer token-auth key (`AuthKey_XXXXXXXXXX.p8`), its Key ID, and the Team ID. One key covers sandbox and production. The provider JWT is cached for 50 minutes.
- **FCM**: a Firebase service-account JSON with the Cloud Messaging role; pass `project_id`, `client_email`, `private_key` (literal `\n` sequences are accepted). Access tokens are cached until 60 s before expiry.
- Omitting either block disables that platform: `/v1/health` reports it and sends return 503.

## Security notes

- Two 256-bit capabilities per device. The `sendKey` only lets its holder notify that one device; the phone hands it to its paired desktops. The `manageKey` stays on the phone and alone can change the token or delete the device, so a compromised desktop can't redirect notifications meant for the phone. The relay stores only sha256 hashes and compares them in constant time.
- Device ids are 128-bit random, so 404 vs 401 does not enable useful enumeration.
- Registrations are rate-limited per client IP and sends per device. `getClientIp` defaults to the first `x-forwarded-for` entry, which is only trustworthy behind a proxy that overwrites it.
- Payloads carry only a title, a short body, a category, and opaque ids. The relay never sees chat content beyond the title/body the desktop chooses to send, and it logs nothing about requests or tokens.
- Invalid tokens reported by APNs/FCM delete the registration and return 410, so the desktop can stop sending.

## Desktop and phone

- **Desktop** (`src-tauri/src/web_remote/native_push.rs`): set the relay base URL with `CODEMUX_PUSH_RELAY_URL` or the `mobile_push.relay_url` setting, e.g. `https://api.codemux.org/api/push`. Unset means no native push: paired phones are told so and browser Web Push carries on unchanged. The desktop only ever posts to the URL it was configured with; a phone supplies the handle, never the URL.
- **Phone** (`mobile/src/lib/push.ts`): asks each paired desktop for its relay URL, registers its native token there once per relay, and hands `{deviceId, sendKey}` to the desktop over its authenticated session.

## Development

```sh
npx vitest run services/push-relay
npx tsc --noEmit -p services/push-relay/tsconfig.json
```

## Not deployed — follow-ups

- Mount on the API server and implement `PushRelayStore` with Drizzle against `schema.sql`.
- Replace the in-memory rate limiter with a shared one (Postgres/Redis) if the API runs more than one process.
- Provision the APNs key and Firebase service account as server secrets; add the bundle ids and package names of every app variant (`org.codemux.app`, `org.codemux.app.preview`, `org.codemux.app.dev`) to `allowedApps`.
- Periodic cleanup of registrations that have not been updated for a long time.
- Real-device verification against APNs sandbox and FCM (nothing here has contacted Apple or Google).
