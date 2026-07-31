/**
 * Test-only scaffolding for the web-remote transport/shim suites.
 *
 * A scripted `WebSocket` fake plus a fake `fetch` for the ticket
 * handshake, so the WS protocol can be exercised deterministically with
 * no real socket. Not imported by any app code, so it never reaches the
 * production bundle.
 */
import zlib from "node:zlib";
import type { WebSocketLike } from "./transport";

export class FakeWebSocket implements WebSocketLike {
  static instances: FakeWebSocket[] = [];
  static reset(): void {
    FakeWebSocket.instances = [];
  }

  binaryType = "";
  readonly url: string;
  readonly sent: string[] = [];
  /** How many times the transport called `close()` on this socket. */
  closeCalls = 0;
  onopen: ((ev: unknown) => void) | null = null;
  onclose: ((ev: { code?: number; reason?: string }) => void) | null = null;
  onerror: ((ev: unknown) => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;

  constructor(url: string) {
    this.url = url;
    FakeWebSocket.instances.push(this);
  }

  send(data: string): void {
    this.sent.push(data);
  }

  close(): void {
    this.closeCalls += 1;
    // A real socket fires `onclose` asynchronously; firing it inline here is
    // what lets a test observe the whole drop → reconnect path in one tick.
    this.onclose?.({ code: 1000 });
  }

  // ── test drivers ──
  open(): void {
    this.onopen?.({});
  }
  drop(code = 1006): void {
    this.onclose?.({ code });
  }
  recvText(obj: unknown): void {
    this.onmessage?.({ data: JSON.stringify(obj) });
  }
  recvBinary(buffer: ArrayBuffer): void {
    this.onmessage?.({ data: buffer });
  }

  /** Parse every frame this socket has sent. */
  parsedSent(): Array<Record<string, unknown>> {
    return this.sent.map((s) => JSON.parse(s) as Record<string, unknown>);
  }
  /** The most recently sent frame, parsed. */
  lastSent(): Record<string, unknown> {
    return JSON.parse(this.sent[this.sent.length - 1]) as Record<string, unknown>;
  }
}

export interface FakeFetch {
  fetchImpl: typeof fetch;
  readonly ticketCount: number;
  calls: Array<{ url: string }>;
}

/** A fake `fetch` that answers `/api/ws-ticket` with sequential tickets. */
export function createFakeFetch(opts: { ticketStatus?: number } = {}): FakeFetch {
  let ticketCount = 0;
  const calls: Array<{ url: string }> = [];
  const impl = async (input: unknown): Promise<unknown> => {
    const url = String(input);
    calls.push({ url });
    if (url.includes("/api/ws-ticket")) {
      const status = opts.ticketStatus ?? 200;
      ticketCount += 1;
      return {
        ok: status >= 200 && status < 300,
        status,
        json: async () => ({ ticket: `ticket-${ticketCount}` }),
      };
    }
    return { ok: true, status: 200, json: async () => ({}) };
  };
  return {
    fetchImpl: impl as unknown as typeof fetch,
    get ticketCount() {
      return ticketCount;
    },
    calls,
  };
}

/** Encode a binary channel frame `[0x01][u32 BE cb][u64 BE idx][payload]`. */
export function buildBinaryFrame(
  callbackId: number,
  index: number,
  payload: Uint8Array,
): ArrayBuffer {
  const buffer = new ArrayBuffer(13 + payload.byteLength);
  const view = new DataView(buffer);
  view.setUint8(0, 0x01);
  view.setUint32(1, callbackId, false);
  view.setUint32(5, Math.floor(index / 2 ** 32), false);
  view.setUint32(9, index >>> 0, false);
  new Uint8Array(buffer, 13).set(payload);
  return buffer;
}

/**
 * A stand-in for the server's per-connection raw-deflate stream.
 *
 * One instance = one simulated connection: every message goes through the
 * same 32 KiB-window stream and is sync-flushed, so its `00 00 FF FF`
 * boundary ships inside the frame and the next message reuses the sliding
 * window (context takeover). Build a new instance to simulate a reconnect.
 */
export class FakeDeflateStream {
  private readonly deflater = zlib.createDeflateRaw({ windowBits: 15 });
  private chunks: Buffer[] = [];

  constructor() {
    this.deflater.on("data", (chunk: Buffer) => this.chunks.push(chunk));
  }

  /** Deflate one message; resolves with the bytes the frame would carry. */
  message(bytes: Uint8Array): Promise<Uint8Array> {
    this.chunks = [];
    this.deflater.write(Buffer.from(bytes));
    return new Promise((resolve) => {
      this.deflater.flush(zlib.constants.Z_SYNC_FLUSH, () => {
        resolve(new Uint8Array(Buffer.concat(this.chunks)));
      });
    });
  }
}

/** Encode a compressed envelope `[tag][u32 BE uncompressed_len][deflated]`. */
export function buildCompressedFrame(
  tag: number,
  uncompressedLen: number,
  deflated: Uint8Array,
): ArrayBuffer {
  const buffer = new ArrayBuffer(5 + deflated.byteLength);
  const view = new DataView(buffer);
  view.setUint8(0, tag);
  view.setUint32(1, uncompressedLen, false);
  new Uint8Array(buffer, 5).set(deflated);
  return buffer;
}

/** A `[0x02]` frame wrapping the JSON text frame for `obj`. */
export async function buildCompressedTextFrame(
  stream: FakeDeflateStream,
  obj: unknown,
): Promise<ArrayBuffer> {
  const json = new TextEncoder().encode(JSON.stringify(obj));
  return buildCompressedFrame(0x02, json.byteLength, await stream.message(json));
}

/** A `[0x03]` frame wrapping an already-built binary frame. */
export async function buildCompressedBinaryFrame(
  stream: FakeDeflateStream,
  inner: ArrayBuffer,
): Promise<ArrayBuffer> {
  const bytes = new Uint8Array(inner);
  return buildCompressedFrame(0x03, bytes.byteLength, await stream.message(bytes));
}

/** Decode a base64 wire capture (the golden frames) into an `ArrayBuffer`. */
export function base64Frame(b64: string): ArrayBuffer {
  const bytes = Buffer.from(b64, "base64");
  return bytes.buffer.slice(
    bytes.byteOffset,
    bytes.byteOffset + bytes.byteLength,
  ) as ArrayBuffer;
}

/** Flush pending microtasks (fetch resolution chains). */
export async function flushMicrotasks(times = 6): Promise<void> {
  for (let i = 0; i < times; i++) await Promise.resolve();
}
