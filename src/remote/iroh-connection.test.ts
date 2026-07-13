import { describe, it, expect, vi } from "vitest";
import { createIrohConnection } from "./iroh-connection";
import { DeviceRegistryError, type DeviceGrant } from "./device-registry";
import { encodeFrame, FrameDecoder, KIND_TEXT } from "./iroh-codec";
import type { IrohByteStream, IrohDialer } from "./iroh-transport";

const enc = new TextEncoder();
const dec = new TextDecoder();

async function tick(n = 8): Promise<void> {
  for (let i = 0; i < n; i++) await Promise.resolve();
}

class FakeStream implements IrohByteStream {
  written: Uint8Array[] = [];
  private waiting: ((v: Uint8Array | null) => void) | null = null;
  async write(data: Uint8Array): Promise<void> {
    this.written.push(data.slice());
  }
  read(): Promise<Uint8Array | null> {
    return new Promise((res) => (this.waiting = res));
  }
  close(): void {
    this.waiting?.(null);
    this.waiting = null;
  }
  deliver(bytes: Uint8Array): void {
    const w = this.waiting;
    this.waiting = null;
    w?.(bytes);
  }
  firstFrame(): { kind: number; json: unknown } {
    const d = new FrameDecoder();
    for (const w of this.written) d.push(w);
    const f = d.next()!;
    return { kind: f.kind, json: JSON.parse(dec.decode(f.payload)) };
  }
}

function grant(): DeviceGrant {
  return {
    nodeId: "node1",
    grant: "v1.payload.sig",
    relayUrlHint: "https://relay.example",
    expiresAt: "2026-07-13T00:10:00Z",
  };
}

function capturingDialer(): {
  dialer: IrohDialer;
  targets: Array<{ nodeId: string; relayUrl?: string | null }>;
  streams: FakeStream[];
} {
  const targets: Array<{ nodeId: string; relayUrl?: string | null }> = [];
  const streams: FakeStream[] = [];
  return {
    targets,
    streams,
    dialer: {
      dial: async (t) => {
        targets.push(t);
        const s = new FakeStream();
        streams.push(s);
        return s;
      },
    },
  };
}

describe("createIrohConnection ticket minting", () => {
  it("mints a fresh grant on /api/ws-ticket and returns a stub ticket", async () => {
    const connectDevice = vi.fn(async () => grant());
    const conn = createIrohConnection({
      registry: { connectDevice },
      deviceId: "d1",
      dialer: capturingDialer().dialer,
      nonceFactory: () => "NONCE",
    });
    const res = await conn.fetchImpl("https://app.local/api/ws-ticket", {
      method: "POST",
    });
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ ticket: "iroh" });
    expect(connectDevice).toHaveBeenCalledWith("d1", "NONCE");
  });

  it("dials the minted node and sends grant + nonce in hello-account", async () => {
    const { dialer, targets, streams } = capturingDialer();
    const conn = createIrohConnection({
      registry: { connectDevice: async () => grant() },
      deviceId: "d1",
      dialer,
      nonceFactory: () => "NONCE",
    });
    await conn.fetchImpl("https://app.local/api/ws-ticket", { method: "POST" });
    conn.wsFactory("wss://ignored");
    await tick();

    expect(targets[0]).toEqual({
      nodeId: "node1",
      relayUrl: "https://relay.example",
    });
    expect(streams[0].firstFrame()).toEqual({
      kind: KIND_TEXT,
      json: { t: "hello-account", grant: "v1.payload.sig", nonce: "NONCE" },
    });
  });

  it("maps an account-auth mint failure to a 401 ticket", async () => {
    const onMintError = vi.fn();
    const conn = createIrohConnection({
      registry: {
        connectDevice: async () => {
          throw new DeviceRegistryError("unauthorized", "expired");
        },
      },
      deviceId: "d1",
      dialer: capturingDialer().dialer,
      onMintError,
    });
    const res = await conn.fetchImpl("https://app.local/api/ws-ticket", {
      method: "POST",
    });
    expect(res.status).toBe(401);
    expect(onMintError).toHaveBeenCalled();
  });

  it("maps an offline mint failure to a 503 ticket (transport retries)", async () => {
    const onMintError = vi.fn();
    const conn = createIrohConnection({
      registry: {
        connectDevice: async () => {
          throw new DeviceRegistryError("offline", "device offline");
        },
      },
      deviceId: "d1",
      dialer: capturingDialer().dialer,
      onMintError,
    });
    const res = await conn.fetchImpl("https://app.local/api/ws-ticket", {
      method: "POST",
    });
    expect(res.status).toBe(503);
    expect(onMintError).toHaveBeenCalledWith(
      expect.objectContaining({ kind: "offline" }),
    );
  });

  it("latches a fatal handshake rejection into a 401 on the next ticket", async () => {
    const { dialer, streams } = capturingDialer();
    const onPending = vi.fn();
    const conn = createIrohConnection({
      registry: { connectDevice: async () => grant() },
      deviceId: "d1",
      dialer,
      onPending,
      nonceFactory: () => "NONCE",
    });
    // First ticket + dial.
    await conn.fetchImpl("https://app.local/api/ws-ticket", { method: "POST" });
    conn.wsFactory("wss://ignored");
    await tick();
    // Desktop fatally rejects the grant.
    streams[0].deliver(
      encodeFrame(KIND_TEXT, enc.encode(JSON.stringify({ t: "unauthorized", reason: "invalid_session" }))),
    );
    await tick();
    expect(onPending).not.toHaveBeenCalled();
    // Next ticket is refused so the transport stops the reconnect loop.
    const res = await conn.fetchImpl("https://app.local/api/ws-ticket", {
      method: "POST",
    });
    expect(res.status).toBe(401);
  });

  it("treats a pending rejection as transient (keeps minting, calls onPending)", async () => {
    const { dialer, streams } = capturingDialer();
    const onPending = vi.fn();
    const conn = createIrohConnection({
      registry: { connectDevice: async () => grant() },
      deviceId: "d1",
      dialer,
      onPending,
      nonceFactory: () => "NONCE",
    });
    await conn.fetchImpl("https://app.local/api/ws-ticket", { method: "POST" });
    conn.wsFactory("wss://ignored");
    await tick();
    streams[0].deliver(
      encodeFrame(KIND_TEXT, enc.encode(JSON.stringify({ t: "unauthorized", reason: "pending_approval" }))),
    );
    await tick();
    expect(onPending).toHaveBeenCalledTimes(1);
    // Still allowed to mint again (poll for approval).
    const res = await conn.fetchImpl("https://app.local/api/ws-ticket", {
      method: "POST",
    });
    expect(res.status).toBe(200);
  });
});
