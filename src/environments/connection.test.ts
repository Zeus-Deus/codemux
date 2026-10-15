import { afterEach, describe, expect, it, vi } from "vitest";

import type { RegisteredDevice } from "@/remote/device-registry";
import { DeviceRegistryError } from "@/remote/device-registry";
import { encodeFrame, FrameDecoder, KIND_TEXT } from "@/remote/iroh-codec";
import type { IrohByteStream, IrohDialer } from "@/remote/iroh-transport";
import type { AppStateSnapshot } from "@/tauri/types";

import { openEnvironmentConnection } from "./connection";
import type { EnvironmentView } from "./store";

const enc = new TextEncoder();
const dec = new TextDecoder();

const device: RegisteredDevice = {
  id: "7",
  deviceId: "env-ai-node",
  nodeId: "node-ai-node",
  name: "ai-node",
  platform: "linux",
  lastSeenAt: new Date().toISOString(),
};

function snapshot(revision: number, titles: string[]): AppStateSnapshot {
  return {
    snapshot_revision: revision,
    snapshot_instance: "instance-1",
    workspaces: titles.map((title) => ({ workspace_id: title, title })),
  } as unknown as AppStateSnapshot;
}

/** A machine at the far end of the stream: answers the account handshake,
 *  records `listen` frames and serves `get_app_state`. */
class FakeMachine implements IrohByteStream {
  readonly frames: Array<Record<string, unknown>> = [];
  closed = false;
  private readonly decoder = new FrameDecoder();
  private readonly queued: Uint8Array[] = [];
  private waiting: ((chunk: Uint8Array | null) => void) | null = null;

  constructor(
    private readonly reply: (frame: Record<string, unknown>) => Record<string, unknown> | null,
  ) {}

  async write(data: Uint8Array): Promise<void> {
    this.decoder.push(data);
    for (let frame = this.decoder.next(); frame; frame = this.decoder.next()) {
      const json = JSON.parse(dec.decode(frame.payload)) as Record<string, unknown>;
      this.frames.push(json);
      const answer = this.reply(json);
      if (answer) this.send(answer);
    }
  }

  read(): Promise<Uint8Array | null> {
    const chunk = this.queued.shift();
    if (chunk) return Promise.resolve(chunk);
    if (this.closed) return Promise.resolve(null);
    return new Promise((resolve) => {
      this.waiting = resolve;
    });
  }

  close(): void {
    this.closed = true;
    this.waiting?.(null);
    this.waiting = null;
  }

  send(frame: Record<string, unknown>): void {
    const bytes = encodeFrame(KIND_TEXT, enc.encode(JSON.stringify(frame)));
    const waiting = this.waiting;
    this.waiting = null;
    if (waiting) waiting(bytes);
    else this.queued.push(bytes);
  }
}

async function settle(): Promise<void> {
  for (let i = 0; i < 20; i++) await Promise.resolve();
  await new Promise((resolve) => setTimeout(resolve, 0));
  for (let i = 0; i < 20; i++) await Promise.resolve();
}

function collect() {
  const patches: Array<Partial<Omit<EnvironmentView, "device">>> = [];
  return { patches, onUpdate: (patch: (typeof patches)[number]) => patches.push(patch) };
}

const grant = { nodeId: device.nodeId, grant: "v1.grant", relayUrlHint: null, expiresAt: "" };

afterEach(() => vi.useRealTimers());

describe("openEnvironmentConnection", () => {
  it("handshakes with a grant, reads the machine's state and follows its changes", async () => {
    const machine = new FakeMachine((frame) => {
      if (frame.t === "hello-account") return { t: "welcome", session_id: "s1" };
      if (frame.t === "invoke" && frame.cmd === "get_app_state") {
        return { t: "ok", id: frame.id, data: snapshot(1, ["auth-refactor"]) };
      }
      return null;
    });
    const dialer: IrohDialer = { dial: vi.fn(async () => machine) };
    const registry = { connectDevice: vi.fn(async () => grant) };
    const { patches, onUpdate } = collect();

    const connection = openEnvironmentConnection({ device, registry, dialer, onUpdate });
    await settle();

    expect(registry.connectDevice).toHaveBeenCalledWith("7", expect.any(String));
    expect(dialer.dial).toHaveBeenCalledWith({ nodeId: "node-ai-node", relayUrl: null });
    expect(machine.frames[0]).toMatchObject({ t: "hello-account", grant: "v1.grant" });
    expect(machine.frames).toContainEqual({ t: "listen", event: "app-state-changed" });
    expect(patches).toContainEqual({ status: "connected", error: null });
    expect(patches).toContainEqual({ appState: snapshot(1, ["auth-refactor"]) });

    machine.send({ t: "event", event: "app-state-changed", payload: snapshot(2, ["auth-refactor", "new-chat"]) });
    await settle();
    expect(patches[patches.length - 1]).toEqual({ appState: snapshot(2, ["auth-refactor", "new-chat"]) });

    connection.close();
    await settle();
    expect(machine.closed).toBe(true);
  });

  it("reports a machine that has not approved this device yet", async () => {
    const machine = new FakeMachine((frame) =>
      frame.t === "hello-account" ? { t: "unauthorized", reason: "pending_approval" } : null,
    );
    const { patches, onUpdate } = collect();
    const connection = openEnvironmentConnection({
      device,
      registry: { connectDevice: async () => grant },
      dialer: { dial: async () => machine },
      onUpdate,
    });
    await settle();
    expect(patches).toContainEqual({ status: "pending-approval", error: null });
    connection.close();
  });

  it("reports an offline machine and stops reporting once closed", async () => {
    const { patches, onUpdate } = collect();
    const connection = openEnvironmentConnection({
      device,
      registry: {
        connectDevice: async () => {
          throw new DeviceRegistryError("offline", "That desktop is offline right now.", 409);
        },
      },
      dialer: { dial: vi.fn() },
      onUpdate,
    });
    await settle();
    expect(patches).toContainEqual({ status: "offline", error: "That desktop is offline right now." });
    connection.close();
    const count = patches.length;
    await settle();
    expect(patches).toHaveLength(count);
  });

  it("stops retrying when the account session is gone", async () => {
    const { patches, onUpdate } = collect();
    const connectDevice = vi.fn(async () => {
      throw new DeviceRegistryError("unauthorized", "Sign in again.", 401);
    });
    openEnvironmentConnection({ device, registry: { connectDevice }, dialer: { dial: vi.fn() }, onUpdate });
    await settle();
    expect(patches).toContainEqual({ status: "unauthorized", error: "Sign in again to reach this machine." });
    expect(connectDevice).toHaveBeenCalledOnce();
  });
});
