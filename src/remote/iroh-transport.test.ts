import { describe, it, expect } from "vitest";
import {
  IrohWebSocket,
  type IrohByteStream,
  type IrohDialer,
  type IrohRejectKind,
} from "./iroh-transport";
import { encodeFrame, FrameDecoder, KIND_TEXT, KIND_BINARY } from "./iroh-codec";

const enc = new TextEncoder();
const dec = new TextDecoder();

/** A scripted iroh byte stream: `deliver`/`eof`/`fail` drive the read pump. */
class FakeIrohStream implements IrohByteStream {
  written: Uint8Array[] = [];
  closed = false;
  private queued: Uint8Array[] = [];
  private waiting: ((v: Uint8Array | null) => void) | null = null;
  private waitingReject: ((e: unknown) => void) | null = null;
  private ended = false;

  async write(data: Uint8Array): Promise<void> {
    this.written.push(data.slice());
  }

  read(): Promise<Uint8Array | null> {
    if (this.queued.length) return Promise.resolve(this.queued.shift()!);
    if (this.ended || this.closed) return Promise.resolve(null);
    return new Promise((res, rej) => {
      this.waiting = res;
      this.waitingReject = rej;
    });
  }

  close(): void {
    this.closed = true;
    this.wake(null);
  }

  // ── test drivers ──
  deliver(bytes: Uint8Array): void {
    if (this.waiting) this.wake(bytes);
    else this.queued.push(bytes);
  }
  eof(): void {
    this.ended = true;
    this.wake(null);
  }
  failRead(err: unknown): void {
    const rej = this.waitingReject;
    this.waiting = this.waitingReject = null;
    rej?.(err);
  }

  /** Decode every frame written to the stream so far. */
  writtenFrames(): Array<{ kind: number; text: string }> {
    const d = new FrameDecoder();
    for (const w of this.written) d.push(w);
    const out: Array<{ kind: number; text: string }> = [];
    for (let f = d.next(); f !== null; f = d.next()) {
      out.push({ kind: f.kind, text: dec.decode(f.payload) });
    }
    return out;
  }

  private wake(v: Uint8Array | null): void {
    const res = this.waiting;
    this.waiting = this.waitingReject = null;
    res?.(v);
  }
}

const welcome = () =>
  encodeFrame(KIND_TEXT, enc.encode(JSON.stringify({ t: "welcome", session_id: "s1" })));
const unauthorized = (reason: string) =>
  encodeFrame(KIND_TEXT, enc.encode(JSON.stringify({ t: "unauthorized", reason })));

async function tick(n = 8): Promise<void> {
  for (let i = 0; i < n; i++) await Promise.resolve();
}

interface Harness {
  ws: IrohWebSocket;
  stream: FakeIrohStream;
  opened: number;
  closes: Array<{ reason?: string }>;
  messages: unknown[];
  rejects: Array<{ kind: IrohRejectKind; reason: string }>;
  errors: unknown[];
}

function mount(dialer?: IrohDialer): Harness {
  const stream = new FakeIrohStream();
  const h: Harness = {
    stream,
    opened: 0,
    closes: [],
    messages: [],
    rejects: [],
    errors: [],
    ws: undefined as unknown as IrohWebSocket,
  };
  const d: IrohDialer = dialer ?? { dial: async () => stream };
  const ws = new IrohWebSocket({
    dialer: d,
    target: () => ({ nodeId: "node-abc", relayUrl: "https://relay.example" }),
    handshake: () => ({ grant: "v1.grant.sig", nonce: "nonce-xyz" }),
    onReject: (kind, reason) => h.rejects.push({ kind, reason }),
    onError: (err) => h.errors.push(err),
  });
  ws.onopen = () => (h.opened += 1);
  ws.onmessage = (ev) => h.messages.push(ev.data);
  ws.onclose = (ev) => h.closes.push(ev ?? {});
  h.ws = ws;
  return h;
}

describe("IrohWebSocket handshake", () => {
  it("sends hello-account as the first frame and opens on welcome", async () => {
    const h = mount();
    await tick();
    const frames = h.stream.writtenFrames();
    expect(frames).toHaveLength(1);
    expect(frames[0].kind).toBe(KIND_TEXT);
    expect(JSON.parse(frames[0].text)).toEqual({
      t: "hello-account",
      grant: "v1.grant.sig",
      nonce: "nonce-xyz",
    });
    expect(h.opened).toBe(0); // not open until welcome
    h.stream.deliver(welcome());
    await tick();
    expect(h.opened).toBe(1);
  });

  it("reports a pending rejection and closes (transport will keep polling)", async () => {
    const h = mount();
    await tick();
    h.stream.deliver(unauthorized("pending_approval"));
    await tick();
    expect(h.rejects).toEqual([{ kind: "pending", reason: "pending_approval" }]);
    expect(h.opened).toBe(0);
    expect(h.closes).toHaveLength(1);
  });

  it("reports a fatal rejection for a bad grant", async () => {
    const h = mount();
    await tick();
    h.stream.deliver(unauthorized("invalid_session"));
    await tick();
    expect(h.rejects).toEqual([{ kind: "fatal", reason: "invalid_session" }]);
    expect(h.closes).toHaveLength(1);
  });
});

describe("IrohWebSocket message surfacing", () => {
  it("surfaces text frames as strings and binary frames as ArrayBuffers", async () => {
    const h = mount();
    await tick();
    h.stream.deliver(welcome());
    await tick();

    h.stream.deliver(
      encodeFrame(KIND_TEXT, enc.encode('{"t":"event","event":"x","payload":1}')),
    );
    const pty = new Uint8Array([0x01, 0, 0, 0, 5, 0, 0, 0, 0, 0, 0, 0, 9, 0xab]);
    h.stream.deliver(encodeFrame(KIND_BINARY, pty));
    await tick();

    expect(h.messages).toHaveLength(2);
    expect(typeof h.messages[0]).toBe("string");
    expect(JSON.parse(h.messages[0] as string)).toMatchObject({ t: "event" });
    expect(h.messages[1]).toBeInstanceOf(ArrayBuffer);
    expect(Array.from(new Uint8Array(h.messages[1] as ArrayBuffer))).toEqual(
      Array.from(pty),
    );
  });

  it("dispatches multiple frames delivered in a single chunk", async () => {
    const h = mount();
    await tick();
    h.stream.deliver(welcome());
    await tick();
    const a = encodeFrame(KIND_TEXT, enc.encode('"a"'));
    const b = encodeFrame(KIND_TEXT, enc.encode('"b"'));
    const both = new Uint8Array(a.length + b.length);
    both.set(a, 0);
    both.set(b, a.length);
    h.stream.deliver(both);
    await tick();
    expect(h.messages).toEqual(['"a"', '"b"']);
  });

  it("frames send() as a kind-0 text frame", async () => {
    const h = mount();
    await tick();
    h.stream.deliver(welcome());
    await tick();
    h.ws.send('{"t":"invoke","id":1,"cmd":"ping"}');
    await tick();
    const frames = h.stream.writtenFrames();
    // frame[0] = hello-account, frame[1] = the invoke
    expect(frames).toHaveLength(2);
    expect(frames[1].kind).toBe(KIND_TEXT);
    expect(JSON.parse(frames[1].text)).toMatchObject({ t: "invoke", id: 1 });
  });
});

describe("IrohWebSocket teardown", () => {
  it("closes on a clean EOF", async () => {
    const h = mount();
    await tick();
    h.stream.deliver(welcome());
    await tick();
    h.stream.eof();
    await tick();
    expect(h.closes).toHaveLength(1);
  });

  it("closes and reports onError when the dial fails", async () => {
    const failing: IrohDialer = {
      dial: async () => {
        throw new Error("relay unreachable");
      },
    };
    const h = mount(failing);
    await tick();
    expect(h.errors).toHaveLength(1);
    expect(h.closes).toHaveLength(1);
    expect(h.opened).toBe(0);
  });

  it("closes and reports onError when a read throws mid-stream", async () => {
    const h = mount();
    await tick();
    h.stream.deliver(welcome());
    await tick();
    h.stream.failRead(new Error("stream reset"));
    await tick();
    expect(h.errors).toHaveLength(1);
    expect(h.closes).toHaveLength(1);
  });

  it("fires onclose only once even if close() is also called", async () => {
    const h = mount();
    await tick();
    h.stream.deliver(welcome());
    await tick();
    h.stream.eof();
    await tick();
    h.ws.close();
    await tick();
    expect(h.closes).toHaveLength(1);
  });
});

describe("IrohWebSocket integration with the codec across chunk splits", () => {
  it("reassembles a welcome split across two reads", async () => {
    const h = mount();
    await tick();
    const w = welcome();
    h.stream.deliver(w.subarray(0, 3));
    await tick();
    expect(h.opened).toBe(0);
    h.stream.deliver(w.subarray(3));
    await tick();
    expect(h.opened).toBe(1);
  });

  it("close() before dial resolves does not leak an open stream", async () => {
    let created: FakeIrohStream | null = null;
    const dialer: IrohDialer = {
      dial: async () => {
        created = new FakeIrohStream();
        return created;
      },
    };
    const h = mount(dialer);
    h.ws.close();
    await tick();
    expect(created).not.toBeNull();
    expect((created as unknown as FakeIrohStream).closed).toBe(true);
  });
});
