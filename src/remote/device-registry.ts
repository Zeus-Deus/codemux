/**
 * Control-plane device registry client (hosted relay bootstrap).
 *
 * When the web client is served from the hosted origin (app.codemux.org) there
 * is no same-origin desktop server to pair with. Instead the browser signs into
 * the user's Codemux account against the control plane (api.codemux.org) and
 * lists the desktops that account has registered for relay access, then mints a
 * short-lived **grant** the desktop verifies over iroh.
 *
 * Endpoints (shared contract):
 *   - `POST /api/auth/sign-in/email {email, password}` — account sign-in. The
 *     "password" is the locally-derived AuthSecret (see `auth-derivation.ts`);
 *     the raw password never leaves the browser. A session cookie is set and a
 *     bearer token is captured for the device calls below.
 *   - `GET  /api/devices` → `{devices:[…]}` — the account's registered desktops.
 *   - `POST /api/devices/:id/connect {browserNonce}` →
 *     `{nodeId, relayUrlHint?, grant, expiresAt}` — a fresh grant bound to this
 *     browser session, presented to the desktop as the `hello-account` credential.
 *
 * All JSON is camelCase. Every method is injectable-`fetch` for tests and never
 * throws a bare network error — failures surface as a typed
 * {@link DeviceRegistryError} the bootstrap maps to friendly copy.
 */

/** A desktop registered for relay access under the signed-in account. */
export interface RegisteredDevice {
  /** Server row id (used in the `:id/connect` path). */
  id: string;
  /** Stable per-install device id. */
  deviceId: string;
  /** The desktop's iroh `node_id` (what the browser dials). */
  nodeId: string;
  /** Human label ("MacBook Pro"). */
  name: string;
  /** OS/platform tag. */
  platform: string;
  /** ISO timestamp of the last registration heartbeat, or `null`. */
  lastSeenAt: string | null;
}

/** A minted connection grant for one device. */
export interface DeviceGrant {
  nodeId: string;
  /** Optional relay URL hint to speed up the first connection. */
  relayUrlHint?: string | null;
  /** Opaque `v1.<payload>.<sig>` grant the desktop verifies. */
  grant: string;
  /** ISO expiry — grants are short-lived and re-minted on every (re)connect. */
  expiresAt: string;
}

/** Classified failure so the bootstrap can react (re-auth vs retry vs message). */
export type DeviceRegistryErrorKind =
  | "unauthorized" // account session invalid/expired → re-sign-in
  | "not_found" // the device was removed
  | "offline" // the device isn't currently reachable
  | "rate_limited"
  | "network"
  | "server";

export class DeviceRegistryError extends Error {
  constructor(
    readonly kind: DeviceRegistryErrorKind,
    message: string,
    readonly status?: number,
  ) {
    super(message);
    this.name = "DeviceRegistryError";
  }
}

export interface DeviceRegistryOptions {
  /** Control-plane base URL, e.g. `https://api.codemux.org`. */
  baseUrl: string;
  fetchImpl?: typeof fetch;
}

export class DeviceRegistry {
  private readonly baseUrl: string;
  private readonly fetchImpl: typeof fetch;
  /** Bearer captured from sign-in; also sent as a cookie via `credentials`. */
  private token: string | null = null;

  constructor(opts: DeviceRegistryOptions) {
    this.baseUrl = opts.baseUrl.replace(/\/+$/, "");
    this.fetchImpl = opts.fetchImpl ?? ((...a) => fetch(...a));
  }

  /** True once a sign-in has succeeded this session. */
  get isSignedIn(): boolean {
    return this.token !== null;
  }

  /**
   * Adopt a bearer obtained out-of-band — the GitHub OAuth code exchange
   * (`/api/auth/web/exchange`) hands back the same account bearer the
   * email/password `signIn` captures, so the rest of the flow is identical.
   */
  setToken(token: string): void {
    this.token = token;
  }

  /**
   * Sign into the account with a pre-derived AuthSecret (the browser derives it
   * from the raw password via `deriveAuthSecret`). Captures the session on
   * success; throws a mapped {@link DeviceRegistryError} otherwise.
   */
  async signIn(email: string, authSecret: string): Promise<void> {
    const res = await this.request("/api/auth/sign-in/email", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ email: email.trim(), password: authSecret }),
    });
    if (!res.ok) {
      throw this.mapError(res.status, await safeErrorCode(res), "sign-in");
    }
    const body = (await safeJson(res)) as { token?: unknown } | null;
    // Better Auth returns the session token in the body; the cookie is also set
    // for subsequent same-session calls. Either is sufficient — keep the bearer.
    this.token = typeof body?.token === "string" ? body.token : "cookie";
  }

  /** List the account's registered desktops. */
  async listDevices(): Promise<RegisteredDevice[]> {
    const res = await this.request("/api/devices", { method: "GET" });
    if (!res.ok) {
      throw this.mapError(res.status, await safeErrorCode(res), "list devices");
    }
    const body = (await safeJson(res)) as { devices?: unknown } | null;
    const raw = Array.isArray(body?.devices) ? body!.devices : [];
    return raw.filter(isRegisteredDevice);
  }

  /** Mint a fresh grant for `deviceId`, bound to `browserNonce`. */
  async connectDevice(
    deviceId: string,
    browserNonce: string,
  ): Promise<DeviceGrant> {
    const res = await this.request(
      `/api/devices/${encodeURIComponent(deviceId)}/connect`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ browserNonce }),
      },
    );
    if (!res.ok) {
      throw this.mapError(res.status, await safeErrorCode(res), "connect");
    }
    const body = (await safeJson(res)) as Partial<DeviceGrant> | null;
    if (
      !body ||
      typeof body.nodeId !== "string" ||
      typeof body.grant !== "string"
    ) {
      throw new DeviceRegistryError(
        "server",
        "The connect response was incomplete.",
      );
    }
    return {
      nodeId: body.nodeId,
      grant: body.grant,
      relayUrlHint:
        typeof body.relayUrlHint === "string" ? body.relayUrlHint : null,
      expiresAt: typeof body.expiresAt === "string" ? body.expiresAt : "",
    };
  }

  // ── Internals ──────────────────────────────────────────────────────

  private async request(path: string, init: RequestInit): Promise<Response> {
    const headers = new Headers(init.headers);
    if (this.token && this.token !== "cookie") {
      headers.set("Authorization", `Bearer ${this.token}`);
    }
    try {
      return await this.fetchImpl(`${this.baseUrl}${path}`, {
        ...init,
        headers,
        credentials: "include",
      });
    } catch (err) {
      throw new DeviceRegistryError(
        "network",
        err instanceof Error ? err.message : "Network request failed.",
      );
    }
  }

  private mapError(
    status: number,
    code: string | null,
    action: string,
  ): DeviceRegistryError {
    if (code === "device_offline" || status === 409 || status === 503) {
      return new DeviceRegistryError(
        "offline",
        "That desktop is offline right now.",
        status,
      );
    }
    if (status === 401 || status === 403) {
      return new DeviceRegistryError(
        "unauthorized",
        "Your account session has expired. Sign in again.",
        status,
      );
    }
    if (status === 404) {
      return new DeviceRegistryError(
        "not_found",
        "That device is no longer registered.",
        status,
      );
    }
    if (status === 429) {
      return new DeviceRegistryError(
        "rate_limited",
        "Too many attempts. Wait a minute and try again.",
        status,
      );
    }
    return new DeviceRegistryError(
      "server",
      `Could not ${action} (${status}).`,
      status,
    );
  }
}

function isRegisteredDevice(v: unknown): v is RegisteredDevice {
  if (typeof v !== "object" || v === null) return false;
  const d = v as Record<string, unknown>;
  return (
    typeof d.id === "string" &&
    typeof d.nodeId === "string" &&
    typeof d.name === "string"
  );
}

async function safeJson(res: Response): Promise<unknown> {
  try {
    return await res.json();
  } catch {
    return null;
  }
}

/** Pull a machine error `code` out of a JSON error body, if any. */
async function safeErrorCode(res: Response): Promise<string | null> {
  const body = (await safeJson(res)) as
    | { error?: unknown; code?: unknown; message?: unknown }
    | null;
  if (!body) return null;
  for (const v of [body.code, body.error]) {
    if (typeof v === "string") return v;
  }
  return null;
}
