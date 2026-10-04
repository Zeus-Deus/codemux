import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { connectOverIroh } from "./hosted-bootstrap";
import { DeviceRegistry, type RegisteredDevice } from "./device-registry";
import { loadIrohDialer } from "./iroh-wasm-loader";
import type { IrohByteStream } from "./iroh-transport";
import { encodeFrame, KIND_TEXT } from "./iroh-codec";

vi.mock("./iroh-wasm-loader", async (importOriginal) => ({
  ...await importOriginal<typeof import("./iroh-wasm-loader")>(),
  loadIrohDialer: vi.fn(),
}));

const device: RegisteredDevice = {
  id: "test-desktop", deviceId: "test-install", nodeId: "test-node",
  name: "Test Desktop", platform: "linux", lastSeenAt: null,
};

function args() {
  const fetchImpl = vi.fn(async () => new Response(JSON.stringify({
    nodeId: "test-node", grant: "fixture-grant", expiresAt: "2099-01-01T00:00:00Z",
  })));
  return {
    registry: new DeviceRegistry({ baseUrl: "https://api.example", fetchImpl }),
    device,
    handlers: { onPending: vi.fn(), onOfflineRetry: vi.fn(), onConnecting: vi.fn() },
    onStatus: vi.fn(),
    onUnauthorized: vi.fn(),
    signal: new AbortController().signal,
  };
}

class ScriptedStream implements IrohByteStream {
  private waiting: ((value: Uint8Array | null) => void) | null = null;
  private frames: Uint8Array[];
  write = vi.fn(async (_bytes: Uint8Array) => {});
  close = vi.fn(() => { this.waiting?.(null); this.waiting = null; });

  constructor(...replies: Record<string, string>[]) {
    this.frames = replies.map((reply) => encodeFrame(
      KIND_TEXT, new TextEncoder().encode(JSON.stringify(reply)),
    ));
  }

  read(): Promise<Uint8Array | null> {
    const frame = this.frames.shift();
    if (frame) return Promise.resolve(frame);
    return new Promise((resolve) => { this.waiting = resolve; });
  }
}

beforeEach(() => vi.useFakeTimers());
afterEach(() => {
  vi.useRealTimers();
  vi.clearAllMocks();
});

describe("hosted connection startup", () => {
  it("bounds a stalled WASM load and ignores its late completion", async () => {
    let finishLoad!: (dialer: { dial: ReturnType<typeof vi.fn> }) => void;
    vi.mocked(loadIrohDialer).mockReturnValue(new Promise((resolve) => (finishLoad = resolve)));
    let failure: unknown;
    const pending = connectOverIroh(args()).catch((error: unknown) => { failure = error; });
    await vi.advanceTimersByTimeAsync(30_000);
    expect((failure as Error).message).toMatch(/timed out/);
    await pending;
    const dial = vi.fn();
    finishLoad({ dial });
    await vi.advanceTimersByTimeAsync(0);
    expect(dial).not.toHaveBeenCalled();
  });

  it("bounds a stalled grant request and does not dial after its late response", async () => {
    let finishGrant!: (value: Response) => void;
    const registry = new DeviceRegistry({
      baseUrl: "https://api.example",
      fetchImpl: vi.fn(() => new Promise<Response>((resolve) => (finishGrant = resolve))),
    });
    const dial = vi.fn();
    vi.mocked(loadIrohDialer).mockResolvedValue({ dial });
    let failure: unknown;
    const pending = connectOverIroh({ ...args(), registry }).catch((error: unknown) => { failure = error; });
    await vi.advanceTimersByTimeAsync(30_000);
    expect((failure as Error).message).toMatch(/timed out/);
    await pending;
    finishGrant(new Response(JSON.stringify({ nodeId: "test-node", grant: "fixture-grant" })));
    await vi.advanceTimersByTimeAsync(60_000);
    expect(dial).not.toHaveBeenCalled();
  });

  it("closes a stream whose desktop handshake never responds", async () => {
    const stream = new ScriptedStream();
    vi.mocked(loadIrohDialer).mockResolvedValue({ dial: async () => stream });
    let failure: unknown;
    const pending = connectOverIroh(args()).catch((error: unknown) => { failure = error; });
    await vi.advanceTimersByTimeAsync(30_000);
    expect((failure as Error).message).toMatch(/timed out/);
    await pending;
    expect(stream.close).toHaveBeenCalledOnce();
  });

  it("does not extend the approval deadline on each pending rejection", async () => {
    const dial = vi.fn(async () => new ScriptedStream({ t: "unauthorized", reason: "pending_approval" }));
    vi.mocked(loadIrohDialer).mockResolvedValue({ dial });
    let failure: unknown;
    const pending = connectOverIroh(args()).catch((error: unknown) => { failure = error; });
    await vi.advanceTimersByTimeAsync(300_000);
    expect((failure as Error).message).toMatch(/approval.*timed out/i);
    await pending;
    expect(dial.mock.calls.length).toBeGreaterThan(1);
  });

  it("bounds alternating approval waits and relay failures by the first approval deadline", async () => {
    let attempts = 0;
    const dial = vi.fn(async () => {
      if (++attempts % 2 === 0) throw new Error("relay unavailable");
      return new ScriptedStream({ t: "unauthorized", reason: "pending_approval" });
    });
    vi.mocked(loadIrohDialer).mockResolvedValue({ dial });
    const options = args();
    let failure: unknown;
    const pending = connectOverIroh(options).catch((error: unknown) => { failure = error; });
    await vi.advanceTimersByTimeAsync(300_001);
    expect(options.handlers.onPending.mock.calls.length).toBeGreaterThan(1);
    expect(options.handlers.onOfflineRetry.mock.calls.length).toBeGreaterThan(1);
    expect(failure).toBeInstanceOf(Error);
    expect((failure as Error).message).toMatch(/timed out/i);
    await pending;
    const finalAttempts = attempts;
    await vi.advanceTimersByTimeAsync(60_000);
    expect(attempts).toBe(finalAttempts);
  });

  it("closes the current stream when cancelled and a fresh attempt can connect", async () => {
    const stalled = new ScriptedStream();
    const live = new ScriptedStream({ t: "welcome" });
    vi.mocked(loadIrohDialer).mockResolvedValue({ dial: async () => stalled });
    const controller = new AbortController();
    const options = args();
    const cancelled = connectOverIroh({ ...options, signal: controller.signal }).catch((error: unknown) => error);
    await vi.advanceTimersByTimeAsync(0);
    controller.abort();
    expect(await cancelled).toMatchObject({ name: "AbortError" });
    expect(stalled.close).toHaveBeenCalledOnce();
    expect(options.onStatus).not.toHaveBeenCalled();

    vi.mocked(loadIrohDialer).mockResolvedValue({ dial: async () => live });
    const retry = args();
    await connectOverIroh(retry);
    expect(retry.onStatus).toHaveBeenLastCalledWith("connected");
    live.close();
  });

  it("preserves expired-account routing instead of reporting a dial timeout", async () => {
    const options = args();
    const registry = new DeviceRegistry({
      baseUrl: "https://api.example", fetchImpl: async () => new Response(null, { status: 401 }),
    });
    const dial = vi.fn();
    vi.mocked(loadIrohDialer).mockResolvedValue({ dial });
    await expect(connectOverIroh({ ...options, registry })).rejects.toMatchObject({ reason: "unauthorized" });
    expect(options.onUnauthorized).toHaveBeenCalledOnce();
    expect(dial).not.toHaveBeenCalled();
  });

  it("surfaces repeated relay dial failures and stops retrying at the deadline", async () => {
    const dial = vi.fn(async () => { throw "No addressing information available"; });
    vi.mocked(loadIrohDialer).mockResolvedValue({ dial });
    const options = args();
    let failure: unknown;
    const pending = connectOverIroh(options).catch((error: unknown) => { failure = error; });
    await vi.advanceTimersByTimeAsync(30_000);
    expect(options.handlers.onOfflineRetry).toHaveBeenCalled();
    expect((failure as Error).message).toContain("No addressing information available");
    await pending;
    const attempts = dial.mock.calls.length;
    expect(attempts).toBeGreaterThan(1);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(dial).toHaveBeenCalledTimes(attempts);
    expect(options.onUnauthorized).not.toHaveBeenCalled();
  });

  it("allows desktop approval beyond the dial deadline, then connects and keeps live reconnects", async () => {
    let approved = false;
    const streams: IrohByteStream[] = [];
    const closes: ReturnType<typeof vi.fn>[] = [];
    let drop!: () => void;
    const dial = vi.fn(async () => {
      let firstRead = true;
      let finishRead: ((value: Uint8Array | null) => void) | null = null;
      const close = vi.fn(() => { finishRead?.(null); });
      const stream: IrohByteStream = {
        write: async () => {},
        read: () => {
          if (firstRead) {
            firstRead = false;
            return Promise.resolve(encodeFrame(KIND_TEXT, new TextEncoder().encode(JSON.stringify(
              approved ? { t: "welcome" } : { t: "unauthorized", reason: "pending_approval" },
            ))));
          }
          return new Promise((resolve) => { finishRead = resolve; drop = () => resolve(null); });
        },
        close,
      };
      streams.push(stream);
      closes.push(close);
      return stream;
    });
    vi.mocked(loadIrohDialer).mockResolvedValue({ dial });
    const options = args();
    let failure: unknown;
    const pending = connectOverIroh(options).catch((error: unknown) => { failure = error; });
    await vi.advanceTimersByTimeAsync(45_000);
    expect(options.handlers.onPending).toHaveBeenCalled();
    expect(failure).toBeUndefined();
    approved = true;
    await vi.advanceTimersByTimeAsync(16_000);
    await pending;
    expect(options.onStatus).toHaveBeenLastCalledWith("connected");
    await vi.advanceTimersByTimeAsync(300_000);
    expect(closes[closes.length - 1]).not.toHaveBeenCalled();
    drop();
    await vi.advanceTimersByTimeAsync(1_000);
    expect(options.onStatus.mock.calls.map(([status]) => status)).toEqual([
      "connected", "reconnecting", "connected",
    ]);
    // Stop the fixture's last reader; production connections stay alive.
    for (const stream of streams) stream.close();
  });

  it("cancels before WASM loads without installing a late shim", async () => {
    let finishLoad!: (dialer: { dial: ReturnType<typeof vi.fn> }) => void;
    vi.mocked(loadIrohDialer).mockReturnValue(new Promise((resolve) => (finishLoad = resolve)));
    const controller = new AbortController();
    const options = { ...args(), signal: controller.signal };
    const internals = (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
    let failure: unknown;
    const pending = connectOverIroh(options).catch((error: unknown) => { failure = error; });
    controller.abort();
    await vi.advanceTimersByTimeAsync(0);
    expect(failure).toMatchObject({ name: "AbortError" });
    await pending;
    const dial = vi.fn();
    finishLoad({ dial });
    await vi.advanceTimersByTimeAsync(60_000);
    expect(dial).not.toHaveBeenCalled();
    expect((window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__).toBe(internals);
    expect(options.onUnauthorized).not.toHaveBeenCalled();
  });

  it("stops a stalled dial and closes a stream that arrives after the timeout", async () => {
    let finishDial!: (stream: IrohByteStream) => void;
    const dial = vi.fn(() => new Promise<IrohByteStream>((resolve) => (finishDial = resolve)));
    vi.mocked(loadIrohDialer).mockResolvedValue({ dial });
    const options = args();
    let failure: unknown;
    const pending = connectOverIroh(options).catch((error: unknown) => { failure = error; });
    await vi.advanceTimersByTimeAsync(30_000);
    expect(failure).toBeInstanceOf(Error);
    expect((failure as Error).message).toMatch(/timed out/i);
    await pending;

    const late = { read: vi.fn(), write: vi.fn(), close: vi.fn() };
    finishDial(late);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(late.close).toHaveBeenCalledOnce();
    expect(late.write).not.toHaveBeenCalled();
    expect(dial).toHaveBeenCalledOnce();
    expect(options.onStatus).not.toHaveBeenCalled();
  });
});
