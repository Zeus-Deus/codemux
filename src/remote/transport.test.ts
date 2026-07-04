import { describe, it, expect, afterEach, vi } from "vitest";
import {
  RemoteTransport,
  parseBinaryChannelFrame,
  type TransportHooks,
} from "./transport";
import {
  FakeWebSocket,
  createFakeFetch,
  buildBinaryFrame,
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
