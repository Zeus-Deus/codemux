import { describe, it, expect, vi } from "vitest";
import { DeviceRegistry, DeviceRegistryError } from "./device-registry";

function jsonResponse(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

function registry(fetchImpl: typeof fetch): DeviceRegistry {
  return new DeviceRegistry({ baseUrl: "https://api.codemux.org/", fetchImpl });
}

describe("DeviceRegistry.signIn", () => {
  it("captures the bearer token and sends it on later calls", async () => {
    const calls: Array<{ url: string; init?: RequestInit }> = [];
    const fetchImpl = vi.fn(async (url: string, init?: RequestInit) => {
      calls.push({ url: String(url), init });
      if (String(url).includes("/api/auth/sign-in/email")) {
        return jsonResponse(200, { token: "sess-tok", user: { id: "u1" } });
      }
      return jsonResponse(200, { devices: [] });
    }) as unknown as typeof fetch;

    const reg = registry(fetchImpl);
    expect(reg.isSignedIn).toBe(false);
    await reg.signIn("me@example.com", "derived-secret");
    expect(reg.isSignedIn).toBe(true);
    // The sign-in body carried the derived secret as the password, not raw text.
    const body = JSON.parse(calls[0].init!.body as string);
    expect(body).toEqual({ email: "me@example.com", password: "derived-secret" });

    await reg.listDevices();
    const authHeader = new Headers(calls[1].init!.headers).get("Authorization");
    expect(authHeader).toBe("Bearer sess-tok");
  });

  it("maps a 401 sign-in to an unauthorized error", async () => {
    const fetchImpl = vi.fn(async () =>
      jsonResponse(401, { error: "invalid_credentials" }),
    ) as unknown as typeof fetch;
    await expect(
      registry(fetchImpl).signIn("me@example.com", "wrong"),
    ).rejects.toMatchObject({ kind: "unauthorized" });
  });
});

describe("DeviceRegistry.listDevices", () => {
  it("returns only well-formed device rows", async () => {
    const fetchImpl = vi.fn(async () =>
      jsonResponse(200, {
        devices: [
          {
            id: "d1",
            deviceId: "inst-1",
            nodeId: "node1",
            name: "Laptop",
            platform: "macos",
            lastSeenAt: "2026-07-13T00:00:00Z",
          },
          { id: "d2" }, // missing nodeId/name → dropped
          "garbage",
        ],
      }),
    ) as unknown as typeof fetch;
    const devices = await registry(fetchImpl).listDevices();
    expect(devices).toHaveLength(1);
    expect(devices[0]).toMatchObject({ id: "d1", nodeId: "node1", name: "Laptop" });
  });

  it("throws unauthorized on a 401 so the flow re-signs-in", async () => {
    const fetchImpl = vi.fn(async () => jsonResponse(401, {})) as unknown as typeof fetch;
    await expect(registry(fetchImpl).listDevices()).rejects.toMatchObject({
      kind: "unauthorized",
    });
  });
});

describe("DeviceRegistry.connectDevice", () => {
  it("posts the browserNonce and returns the grant descriptor", async () => {
    let sentBody: unknown;
    const fetchImpl = vi.fn(async (url: string, init?: RequestInit) => {
      expect(String(url)).toContain("/api/devices/d1/connect");
      sentBody = JSON.parse(init!.body as string);
      return jsonResponse(200, {
        nodeId: "node1",
        relayUrlHint: "https://relay.example",
        grant: "v1.payload.sig",
        expiresAt: "2026-07-13T00:10:00Z",
      });
    }) as unknown as typeof fetch;

    const grant = await registry(fetchImpl).connectDevice("d1", "nonce-123");
    expect(sentBody).toEqual({ browserNonce: "nonce-123" });
    expect(grant).toEqual({
      nodeId: "node1",
      relayUrlHint: "https://relay.example",
      grant: "v1.payload.sig",
      expiresAt: "2026-07-13T00:10:00Z",
    });
  });

  it("classifies an offline device (409) distinctly", async () => {
    const fetchImpl = vi.fn(async () =>
      jsonResponse(409, { error: "device_offline" }),
    ) as unknown as typeof fetch;
    await expect(registry(fetchImpl).connectDevice("d1", "n")).rejects.toMatchObject(
      { kind: "offline" },
    );
  });

  it("classifies a removed device (404) as not_found", async () => {
    const fetchImpl = vi.fn(async () => jsonResponse(404, {})) as unknown as typeof fetch;
    await expect(registry(fetchImpl).connectDevice("d1", "n")).rejects.toMatchObject(
      { kind: "not_found" },
    );
  });

  it("wraps a network throw as a network error", async () => {
    const fetchImpl = vi.fn(async () => {
      throw new TypeError("Failed to fetch");
    }) as unknown as typeof fetch;
    const err = await registry(fetchImpl)
      .connectDevice("d1", "n")
      .catch((e) => e);
    expect(err).toBeInstanceOf(DeviceRegistryError);
    expect(err.kind).toBe("network");
  });

  it("rejects an incomplete grant body", async () => {
    const fetchImpl = vi.fn(async () =>
      jsonResponse(200, { nodeId: "node1" }),
    ) as unknown as typeof fetch;
    await expect(registry(fetchImpl).connectDevice("d1", "n")).rejects.toMatchObject(
      { kind: "server" },
    );
  });
});
