/**
 * Test-only scaffolding for the web-remote transport/shim suites.
 *
 * A scripted `WebSocket` fake plus a fake `fetch` for the ticket
 * handshake, so the WS protocol can be exercised deterministically with
 * no real socket. Not imported by any app code, so it never reaches the
 * production bundle.
 */
import type { WebSocketLike } from "./transport";

export class FakeWebSocket implements WebSocketLike {
  static instances: FakeWebSocket[] = [];
  static reset(): void {
    FakeWebSocket.instances = [];
  }

  binaryType = "";
  readonly url: string;
  readonly sent: string[] = [];
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

/** Flush pending microtasks (fetch resolution chains). */
export async function flushMicrotasks(times = 6): Promise<void> {
  for (let i = 0; i < times; i++) await Promise.resolve();
}
