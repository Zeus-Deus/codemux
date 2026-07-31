import { describe, it, expect, afterEach, vi } from "vitest";
import {
  RemoteTransport,
  FrameInflater,
  parseBinaryChannelFrame,
  type TransportHooks,
} from "./transport";
import {
  FakeWebSocket,
  FakeDeflateStream,
  createFakeFetch,
  base64Frame,
  buildBinaryFrame,
  buildCompressedFrame,
  buildCompressedTextFrame,
  buildCompressedBinaryFrame,
  flushMicrotasks,
} from "./testing";

interface Recorder {
  channels: Array<{ cb: number; idx: number; msg: unknown }>;
  events: Array<{ event: string; payload: unknown }>;
  statuses: string[];
  unauthorized: number;
}

function makeHooks(): { hooks: TransportHooks; rec: Recorder } {
  const rec: Recorder = {
    channels: [],
    events: [],
    statuses: [],
    unauthorized: 0,
  };
  const hooks: TransportHooks = {
    onChannelMessage: (cb, idx, msg) => rec.channels.push({ cb, idx, msg }),
    onEvent: (event, payload) => rec.events.push({ event, payload }),
    onStatusChange: (s) => rec.statuses.push(s),
    onUnauthorized: () => (rec.unauthorized += 1),
  };
  return { hooks, rec };
}

const live: RemoteTransport[] = [];

function newTransport(overrides: {
  fetchImpl?: typeof fetch;
  getToken?: () => string | null;
} = {}): { transport: RemoteTransport; rec: Recorder } {
  const { hooks, rec } = makeHooks();
  const fetch = overrides.fetchImpl ?? createFakeFetch().fetchImpl;
  const transport = new RemoteTransport({
    baseUrl: "http://desktop.local:4377",
    getToken: overrides.getToken ?? (() => "session-token"),
    hooks,
    deps: {
      fetchImpl: fetch,
      wsFactory: (url) => new FakeWebSocket(url),
    },
  });
  live.push(transport);
  return { transport, rec };
}

async function connect(transport: RemoteTransport): Promise<FakeWebSocket> {
  const p = transport.connect();
  await flushMicrotasks();
  const ws = FakeWebSocket.instances[FakeWebSocket.instances.length - 1];
  ws.open();
  await p;
  return ws;
}

afterEach(() => {
  for (const t of live) t.close();
  live.length = 0;
  FakeWebSocket.reset();
  vi.useRealTimers();
});

describe("parseBinaryChannelFrame", () => {
  it("decodes a well-formed frame", () => {
    const frame = parseBinaryChannelFrame(
      buildBinaryFrame(7, 42, new Uint8Array([1, 2, 3, 255])),
    );
    expect(frame).not.toBeNull();
    expect(frame!.callbackId).toBe(7);
    expect(frame!.index).toBe(42);
    expect(Array.from(frame!.payload)).toEqual([1, 2, 3, 255]);
  });

  it("preserves large u64 indices past 2^32", () => {
    const idx = 2 ** 32 + 5;
    const frame = parseBinaryChannelFrame(buildBinaryFrame(1, idx, new Uint8Array()));
    expect(frame!.index).toBe(idx);
  });

  it("rejects short buffers and a wrong frame tag", () => {
    expect(parseBinaryChannelFrame(new ArrayBuffer(4))).toBeNull();
    const buf = buildBinaryFrame(1, 0, new Uint8Array([9]));
    new DataView(buf).setUint8(0, 0x02);
    expect(parseBinaryChannelFrame(buf)).toBeNull();
  });
});

describe("invoke roundtrip", () => {
  it("resolves on the id-matched ok frame", async () => {
    const { transport } = newTransport();
    const ws = await connect(transport);
    const p = transport.invoke("get_app_state", { a: 1 });
    const sent = ws.lastSent();
    expect(sent).toMatchObject({ t: "invoke", cmd: "get_app_state", args: { a: 1 } });
    ws.recvText({ t: "ok", id: sent.id, data: { ok: true } });
    await expect(p).resolves.toEqual({ ok: true });
  });

  it("rejects on the id-matched err frame", async () => {
    const { transport } = newTransport();
    const ws = await connect(transport);
    const p = transport.invoke("boom");
    const id = ws.lastSent().id;
    ws.recvText({ t: "err", id, error: "nope" });
    await expect(p).rejects.toBe("nope");
  });

  it("matches out-of-order responses by id", async () => {
    const { transport } = newTransport();
    const ws = await connect(transport);
    const p1 = transport.invoke("a");
    const id1 = ws.lastSent().id;
    const p2 = transport.invoke("b");
    const id2 = ws.lastSent().id;
    ws.recvText({ t: "ok", id: id2, data: 2 });
    ws.recvText({ t: "ok", id: id1, data: 1 });
    expect(await p1).toBe(1);
    expect(await p2).toBe(2);
  });

  it("rejects invoke when the socket is not connected", async () => {
    const { transport } = newTransport();
    await expect(transport.invoke("x")).rejects.toThrow(/not connected/);
  });
});

describe("channel + event frame routing", () => {
  it("routes JSON chan frames to onChannelMessage", async () => {
    const { transport, rec } = newTransport();
    const ws = await connect(transport);
    ws.recvText({ t: "chan", ch: 5, idx: 0, data: { hello: "world" } });
    expect(rec.channels).toEqual([{ cb: 5, idx: 0, msg: { hello: "world" } }]);
  });

  it("routes binary frames to onChannelMessage as a Uint8Array", async () => {
    const { transport, rec } = newTransport();
    const ws = await connect(transport);
    ws.recvBinary(buildBinaryFrame(9, 3, new Uint8Array([10, 20])));
    expect(rec.channels).toHaveLength(1);
    expect(rec.channels[0].cb).toBe(9);
    expect(rec.channels[0].idx).toBe(3);
    expect(rec.channels[0].msg).toBeInstanceOf(Uint8Array);
    expect(Array.from(rec.channels[0].msg as Uint8Array)).toEqual([10, 20]);
  });

  it("routes event frames to onEvent", async () => {
    const { transport, rec } = newTransport();
    const ws = await connect(transport);
    ws.recvText({ t: "event", event: "app-state-changed", payload: { n: 1 } });
    expect(rec.events).toEqual([
      { event: "app-state-changed", payload: { n: 1 } },
    ]);
  });
});

describe("disconnect handling", () => {
  it("rejects in-flight invokes and reports reconnecting on drop", async () => {
    vi.useFakeTimers();
    const { transport, rec } = newTransport();
    const p = transport.connect();
    await flushMicrotasks();
    const ws = FakeWebSocket.instances[0];
    ws.open();
    await p;
    const inflight = transport.invoke("slow");
    ws.drop();
    await expect(inflight).rejects.toThrow(/connection lost/);
    expect(rec.statuses).toContain("reconnecting");
  });
});

describe("reconnect", () => {
  it("re-issues a ticket and re-sends all subscriptions", async () => {
    vi.useFakeTimers();
    const fetch = createFakeFetch();
    const { transport } = newTransport({ fetchImpl: fetch.fetchImpl });

    const p = transport.connect();
    await flushMicrotasks();
    const ws1 = FakeWebSocket.instances[0];
    ws1.open();
    await p;

    transport.subscribe("terminal:1");
    transport.subscribe("terminal:2");
    const ws1Listens = ws1
      .parsedSent()
      .filter((m) => m.t === "listen")
      .map((m) => m.event);
    expect(ws1Listens).toEqual(["terminal:1", "terminal:2"]);

    const ticketsBefore = fetch.ticketCount;
    ws1.drop();
    await vi.advanceTimersByTimeAsync(1000);
    await flushMicrotasks();

    expect(FakeWebSocket.instances).toHaveLength(2);
    expect(fetch.ticketCount).toBe(ticketsBefore + 1);

    const ws2 = FakeWebSocket.instances[1];
    ws2.open();
    const ws2Listens = ws2
      .parsedSent()
      .filter((m) => m.t === "listen")
      .map((m) => m.event);
    expect(ws2Listens).toEqual(
      expect.arrayContaining(["terminal:1", "terminal:2"]),
    );
  });

  it("gives up and reports unauthorized on a 401 ticket", async () => {
    const fetch = createFakeFetch({ ticketStatus: 401 });
    const { transport, rec } = newTransport({ fetchImpl: fetch.fetchImpl });
    await expect(transport.connect()).rejects.toBeTruthy();
    expect(rec.unauthorized).toBe(1);
  });
});

describe("compression negotiation", () => {
  it("offers compress=deflate alongside the encoded ticket", async () => {
    const { transport } = newTransport();
    const ws = await connect(transport);
    expect(ws.url).toContain("compress=deflate");
    expect(ws.url).toContain("ticket=ticket-1");
  });
});

describe("compressed frames", () => {
  /** A body long enough to be worth compressing and full of back-references. */
  const bigPayload = { items: Array(8).fill("repeatable-payload") };

  it("resolves a pending invoke from a 0x02 text envelope", async () => {
    const { transport } = newTransport();
    const ws = await connect(transport);
    const stream = new FakeDeflateStream();
    const p = transport.invoke("get_app_state");
    const id = ws.lastSent().id;
    ws.recvBinary(
      await buildCompressedTextFrame(stream, { t: "ok", id, data: bigPayload }),
    );
    await expect(p).resolves.toEqual(bigPayload);
  });

  it("delivers a 0x03 envelope like the uncompressed binary frame", async () => {
    const { transport, rec } = newTransport();
    const ws = await connect(transport);
    const stream = new FakeDeflateStream();
    const payload = new Uint8Array([0, 1, 2, 254, 255]);
    ws.recvBinary(
      await buildCompressedBinaryFrame(stream, buildBinaryFrame(9, 3, payload)),
    );
    expect(rec.channels).toHaveLength(1);
    expect(rec.channels[0].cb).toBe(9);
    expect(rec.channels[0].idx).toBe(3);
    expect(rec.channels[0].msg).toBeInstanceOf(Uint8Array);
    expect(Array.from(rec.channels[0].msg as Uint8Array)).toEqual(
      Array.from(payload),
    );
  });

  it("keeps the inflate stream across frames (context takeover)", async () => {
    const { transport, rec } = newTransport();
    const ws = await connect(transport);
    const stream = new FakeDeflateStream();
    const first = await buildCompressedTextFrame(stream, {
      t: "event",
      event: "app-state-changed",
      payload: bigPayload,
    });
    const second = await buildCompressedTextFrame(stream, {
      t: "event",
      event: "app-state-changed",
      payload: bigPayload,
    });
    // The second message is mostly back-references into the first, so it is
    // far smaller — the client can only decode it by holding the dictionary.
    expect(second.byteLength).toBeLessThan(first.byteLength / 2);
    ws.recvBinary(first);
    ws.recvBinary(second);
    expect(rec.events).toEqual([
      { event: "app-state-changed", payload: bigPayload },
      { event: "app-state-changed", payload: bigPayload },
    ]);
  });

  it("cannot decode a later frame without the earlier dictionary", async () => {
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    const { transport, rec } = newTransport();
    const ws = await connect(transport);
    const stream = new FakeDeflateStream();
    const body = { t: "event", event: "app-state-changed", payload: bigPayload };
    await buildCompressedTextFrame(stream, body);
    const second = await buildCompressedTextFrame(stream, body);
    // Feeding only the second frame to a fresh client stream must fail — the
    // proof that the passing test above really depends on retained state.
    ws.recvBinary(second);
    expect(rec.events).toEqual([]);
    expect(errors).toHaveBeenCalled();
    errors.mockRestore();
  });

  it("interleaves compressed, plain text and plain binary frames in order", async () => {
    const { transport, rec } = newTransport();
    const ws = await connect(transport);
    const stream = new FakeDeflateStream();
    const compressed1 = await buildCompressedTextFrame(stream, {
      t: "chan",
      ch: 4,
      idx: 0,
      data: bigPayload,
    });
    const compressed2 = await buildCompressedTextFrame(stream, {
      t: "chan",
      ch: 4,
      idx: 3,
      data: bigPayload,
    });
    ws.recvBinary(compressed1);
    ws.recvText({ t: "chan", ch: 4, idx: 1, data: "plain" });
    ws.recvBinary(buildBinaryFrame(4, 2, new Uint8Array([7])));
    ws.recvBinary(compressed2);
    expect(rec.channels.map((c) => c.idx)).toEqual([0, 1, 2, 3]);
    expect(rec.channels[0].msg).toEqual(bigPayload);
    expect(rec.channels[1].msg).toBe("plain");
    expect(Array.from(rec.channels[2].msg as Uint8Array)).toEqual([7]);
    expect(rec.channels[3].msg).toEqual(bigPayload);
  });

  it("decodes a fresh server stream after a reconnect", async () => {
    vi.useFakeTimers();
    const { transport, rec } = newTransport();
    const p = transport.connect();
    await flushMicrotasks();
    const ws1 = FakeWebSocket.instances[0];
    ws1.open();
    await p;

    const stream1 = new FakeDeflateStream();
    ws1.recvBinary(
      await buildCompressedTextFrame(stream1, {
        t: "event",
        event: "before",
        payload: bigPayload,
      }),
    );

    ws1.drop();
    await vi.advanceTimersByTimeAsync(1000);
    await flushMicrotasks();
    const ws2 = FakeWebSocket.instances[1];
    ws2.open();

    // A new connection restarts the server's deflate stream, so the client
    // must have thrown its dictionary away too.
    const stream2 = new FakeDeflateStream();
    ws2.recvBinary(
      await buildCompressedTextFrame(stream2, {
        t: "event",
        event: "after",
        payload: bigPayload,
      }),
    );
    expect(rec.events.map((e) => e.event)).toEqual(["before", "after"]);
  });
});

/**
 * The only test that puts both production implementations on one stream: the
 * frames below were emitted by the real Rust `FrameCompressor` (flate2 /
 * miniz_oxide) and are inflated here by the real client `FrameInflater`
 * (fflate). A mock on either side could agree with itself forever while the
 * two encoders quietly diverged.
 *
 * To regenerate after any encoder/level/window change:
 *
 *   cargo test --manifest-path src-tauri/Cargo.toml --lib \
 *     web_remote::compress::tests::golden_frames_for_the_client_suite \
 *     -- --ignored --nocapture
 *
 * That `#[ignore]`d test in `src-tauri/src/web_remote/compress.rs` prints one
 * base64 line per frame, in order; replace the array below with them verbatim
 * (copy whole lines — a partially-transcribed frame still base64-decodes, it
 * just decodes to a short deflate stream). The payload builders there
 * (`GOLDEN_UNIT`, `golden_pty_payload`, the 3x / 1450x repeats, cb 77 / idx 9)
 * are mirrored literally here, so changing one means changing both.
 */
const GOLDEN_UNIT =
  '{"t":"event","event":"app-state-changed","payload":{"workspaces":[]}}';
const GOLDEN_PTY_UNIT = "\x1b[2J\x1b[H$ ls -la\r\ntotal 0\r\n";
const GOLDEN_CB = 77;
const GOLDEN_IDX = 9;
/** One connection's stream: text, the same text again, a `0x01` frame, ~100KB. */
const GOLDEN_FRAMES = [
  "AgAAAM/MzMEJgDAMRuFd/rNdIKuIh9AGBaUNJigSurseHKKnd3jwBRwEuaQ6pr8EVk3m7JLyxnWV8j3l52hcQIG7nbspZzHQvPQewyAvAAAA//8=",
  "AgAAAM/czIEAAAAAAJD/ax8GZAgAAAD//w==",
  "AwAAAHWsybEJACAMAMFYKmSCNClsBXEKEZwgVfqAhdkfdQcfvroAAPP+iiRtkPTMtrmYYvLlalwx/ZUDAAD//w==",
  "AgABhtLs0TENAAAAgKD+re3hKMCDCAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAgEAoFAIBAIBAKBQCAQCAQCgUAgEAjkhAQAAP//",
];

describe("golden frames from the real server compressor", () => {
  it("inflates a recorded connection stream to exact bytes", () => {
    // `new Uint8Array(…)` around every expectation: `TextEncoder` hands back a
    // Node-realm typed array, and `toEqual` treats cross-realm views as
    // unequal even when every byte matches.
    const enc = (s: string) => new Uint8Array(new TextEncoder().encode(s));
    const expected: Array<[number, Uint8Array]> = [
      [0x02, enc(GOLDEN_UNIT.repeat(3))],
      [0x02, enc(GOLDEN_UNIT.repeat(3))],
      [
        0x03,
        new Uint8Array(
          buildBinaryFrame(GOLDEN_CB, GOLDEN_IDX, enc(GOLDEN_PTY_UNIT.repeat(4))),
        ),
      ],
      [0x02, enc(GOLDEN_UNIT.repeat(1450))],
    ];

    const inflater = new FrameInflater();
    GOLDEN_FRAMES.forEach((b64, i) => {
      const frame = base64Frame(b64);
      const [tag, want] = expected[i];
      expect(new Uint8Array(frame)[0]).toBe(tag);
      const out = inflater.decode(frame);
      expect(out, `golden frame ${i} must inflate`).not.toBeNull();
      expect(out).toEqual(want);
    });

    // Frame 1 is frame 0's payload again: only the shared dictionary makes it
    // that small, and only a client holding the same dictionary can read it.
    expect(GOLDEN_FRAMES[1].length).toBeLessThan(GOLDEN_FRAMES[0].length / 2);
  });

  it("cannot read a golden frame out of order", () => {
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    // Proof the test above depends on the retained dictionary rather than on
    // each frame being self-contained.
    expect(new FrameInflater().decode(base64Frame(GOLDEN_FRAMES[3]))).toBeNull();
    expect(errors).toHaveBeenCalled();
    errors.mockRestore();
  });
});

describe("malformed compressed frames", () => {
  const body = { t: "event", event: "ping", payload: { n: 1 } };

  /**
   * Connect on fake timers with `console.error` captured. Every case here
   * poisons the inflate stream, which logs one line and then escalates: a
   * desynced dictionary cannot be recovered from in place, and silently
   * dropping later compressed frames would hang invokes (no invoke timeout) and
   * stall the `Channel` reorder buffer on a missing PTY index.
   */
  async function connectSilenced() {
    vi.useFakeTimers();
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    const fetch = createFakeFetch();
    const { transport, rec } = newTransport({ fetchImpl: fetch.fetchImpl });
    const p = transport.connect();
    await flushMicrotasks();
    const ws = FakeWebSocket.instances[0];
    ws.open();
    await p;
    return { transport, ws, rec, errors, fetch };
  }

  /**
   * Assert the poison tore the socket down, then run the backoff timer and
   * hand back the socket the retry opened.
   */
  async function expectEscalation(
    ws: FakeWebSocket,
    rec: Recorder,
    fetch: ReturnType<typeof createFakeFetch>,
  ): Promise<FakeWebSocket> {
    expect(ws.closeCalls, "the poisoned socket must be closed").toBe(1);
    expect(rec.statuses).toContain("reconnecting");
    await vi.advanceTimersByTimeAsync(1000);
    await flushMicrotasks();
    expect(FakeWebSocket.instances).toHaveLength(2);
    expect(fetch.ticketCount).toBe(2);
    const ws2 = FakeWebSocket.instances[1];
    ws2.open();
    return ws2;
  }

  it("closes the socket and reconnects onto a fresh stream on a failed inflate", async () => {
    const { ws, rec, errors, fetch } = await connectSilenced();
    const garbage = new Uint8Array([0xff, 0x13, 0x42, 0x99, 0x01, 0x7f]);
    expect(() =>
      ws.recvBinary(buildCompressedFrame(0x02, 40, garbage)),
    ).not.toThrow();
    expect(errors).toHaveBeenCalled();

    const ws2 = await expectEscalation(ws, rec, fetch);
    // The reconnect reset the inflater, so the new connection's stream decodes.
    const stream = new FakeDeflateStream();
    ws2.recvBinary(await buildCompressedTextFrame(stream, body));
    expect(rec.events).toEqual([{ event: "ping", payload: { n: 1 } }]);
    errors.mockRestore();
  });

  it("rejects in-flight invokes when the stream is poisoned", async () => {
    const { transport, ws, errors } = await connectSilenced();
    const inflight = transport.invoke("slow");
    // Without the escalation this invoke would hang forever: its `ok` frame
    // would have been a compressed frame that the poisoned stream dropped.
    ws.recvBinary(buildCompressedFrame(0x02, 40, new Uint8Array([0xff, 0x13])));
    await expect(inflight).rejects.toThrow(/connection lost/);
    errors.mockRestore();
  });

  it("escalates a truncated envelope header", async () => {
    const { ws, rec, errors, fetch } = await connectSilenced();
    expect(() => ws.recvBinary(new Uint8Array([0x02, 0, 0]).buffer)).not.toThrow();
    expect(errors).toHaveBeenCalled();
    await expectEscalation(ws, rec, fetch);
    errors.mockRestore();
  });

  it("escalates an envelope whose uncompressed_len disagrees", async () => {
    const { ws, rec, errors, fetch } = await connectSilenced();
    const stream = new FakeDeflateStream();
    const json = new TextEncoder().encode(JSON.stringify(body));
    ws.recvBinary(
      buildCompressedFrame(0x02, json.byteLength + 5, await stream.message(json)),
    );
    expect(rec.events).toEqual([]);
    expect(errors).toHaveBeenCalled();
    await expectEscalation(ws, rec, fetch);
    errors.mockRestore();
  });

  it("rejects an absurd declared length without allocating for it", async () => {
    const { ws, rec, errors, fetch } = await connectSilenced();
    // 4 GiB. Allocating first would throw a RangeError straight out of
    // `onmessage` (leaving the stream desynced but unpoisoned) or wreck the tab.
    expect(() =>
      ws.recvBinary(buildCompressedFrame(0x02, 0xffffffff, new Uint8Array([0x00]))),
    ).not.toThrow();
    expect(errors.mock.calls.flat().join(" ")).toMatch(/above the .* cap/);
    await expectEscalation(ws, rec, fetch);
    errors.mockRestore();
  });

  it("drops compressed frames still arriving on the dying socket", async () => {
    const { ws, rec, errors } = await connectSilenced();
    ws.recvBinary(buildCompressedFrame(0x02, 40, new Uint8Array([0xff, 0x13])));
    const errorsAfterPoison = errors.mock.calls.length;

    // A real socket keeps delivering for a tick or two after `close()`. Those
    // frames were compressed against a dictionary the client no longer shares,
    // so they are dropped without a second log line — the reconnect is already
    // in flight.
    const stream = new FakeDeflateStream();
    ws.recvBinary(await buildCompressedTextFrame(stream, body));
    expect(rec.events).toEqual([]);
    expect(errors.mock.calls.length).toBe(errorsAfterPoison);

    // Plain text and `0x01` frames never touched the inflate stream, so they
    // are still parsed on the way out.
    ws.recvText(body);
    ws.recvBinary(buildBinaryFrame(1, 0, new Uint8Array([5])));
    expect(rec.events).toHaveLength(1);
    expect(rec.channels).toHaveLength(1);
    errors.mockRestore();
  });
});
