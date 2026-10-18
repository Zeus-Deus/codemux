import { beforeEach, describe, expect, it, vi } from "vitest";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({
  invoke,
  Channel: class {
    constructor(readonly onmessage: (message: unknown) => void) {}
  },
}));

import { DeviceRegistry, DeviceRegistryError } from "@/remote/device-registry";

import { NativeByteStream, NativeIrohDialer, nativeAccountFetch } from "./native";

beforeEach(() => {
  invoke.mockReset();
  invoke.mockResolvedValue(undefined);
});

describe("nativeAccountFetch", () => {
  it("lets DeviceRegistry list devices and mint grants through the backend", async () => {
    invoke.mockImplementation(async (_cmd: string, { request }: { request: { method: string; path: string } }) =>
      request.method === "GET"
        ? { status: 200, body: { devices: [{ id: "7", deviceId: "env", nodeId: "node", name: "ai-node", platform: "linux", lastSeenAt: null }] } }
        : { status: 200, body: { nodeId: "node", grant: "v1.g", relayUrlHint: "https://relay", expiresAt: "x" } },
    );
    const registry = new DeviceRegistry({ baseUrl: "https://account.codemux.invalid", fetchImpl: nativeAccountFetch });

    expect(await registry.listDevices()).toHaveLength(1);
    expect(await registry.connectDevice("7", "nonce-1")).toMatchObject({ grant: "v1.g", relayUrlHint: "https://relay" });
    expect(invoke).toHaveBeenCalledWith("environment_account_request", {
      request: { method: "GET", path: "/api/devices", body: undefined },
    });
    expect(invoke).toHaveBeenCalledWith("environment_account_request", {
      request: { method: "POST", path: "/api/devices/7/connect", body: { browserNonce: "nonce-1" } },
    });
  });

  it("maps a signed-out desktop to an unauthorized registry error", async () => {
    invoke.mockResolvedValue({ status: 401, body: { error: "signed_out" } });
    const registry = new DeviceRegistry({ baseUrl: "https://account.codemux.invalid", fetchImpl: nativeAccountFetch });
    await expect(registry.listDevices()).rejects.toMatchObject({ kind: "unauthorized" });
    await expect(registry.listDevices()).rejects.toBeInstanceOf(DeviceRegistryError);
  });
});

describe("NativeByteStream", () => {
  it("hands chunks to reads in order, whether they arrive before or after the read", async () => {
    const stream = new NativeByteStream();
    stream.push(new Uint8Array([1]).buffer);
    const pending = stream.read();
    expect(await pending).toEqual(new Uint8Array([1]));
    const waiting = stream.read();
    stream.push(new Uint8Array([2, 3]).buffer);
    expect(await waiting).toEqual(new Uint8Array([2, 3]));
  });

  it("ends reads cleanly or with the backend's read error", async () => {
    const clean = new NativeByteStream();
    const waiting = clean.read();
    clean.push({ t: "end", error: null });
    expect(await waiting).toBeNull();
    expect(await clean.read()).toBeNull();

    const broken = new NativeByteStream();
    broken.push({ t: "end", error: "connection lost" });
    await expect(broken.read()).rejects.toThrow("connection lost");
  });

  it("writes raw bytes with the stream id and closes the backend stream once", async () => {
    const stream = new NativeByteStream();
    stream.id = 4;
    await stream.write(new Uint8Array([9]));
    expect(invoke).toHaveBeenCalledWith("environment_stream_write", new Uint8Array([9]), {
      headers: { "x-stream-id": "4" },
    });
    const waiting = stream.read();
    stream.close();
    stream.close();
    expect(await waiting).toBeNull();
    expect(invoke.mock.calls.filter(([cmd]) => cmd === "environment_stream_close")).toEqual([
      ["environment_stream_close", { streamId: 4 }],
    ]);
    await expect(stream.write(new Uint8Array([1]))).rejects.toThrow("closed");
  });
});

describe("NativeIrohDialer", () => {
  it("dials through the backend and feeds its channel into the stream", async () => {
    let channel: { onmessage(message: unknown): void } | null = null;
    invoke.mockImplementation(async (_cmd: string, args: { onData: typeof channel }) => {
      channel = args.onData;
      return 11;
    });
    const stream = await new NativeIrohDialer().dial({ nodeId: "node", relayUrl: "https://relay" });
    expect(invoke).toHaveBeenCalledWith("environment_dial", {
      nodeId: "node",
      relayUrl: "https://relay",
      onData: expect.anything(),
    });
    channel!.onmessage(new Uint8Array([5]).buffer);
    expect(await stream.read()).toEqual(new Uint8Array([5]));
  });
});
