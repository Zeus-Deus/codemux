/**
 * Iroh-backed {@link WebSocketLike} adapter — the "different pipe, same
 * protocol" seam for web remote access.
 *
 * {@link RemoteTransport} already speaks the `/ws` frame contract over a socket
 * it obtains from an injectable `wsFactory: (url) => WebSocketLike`. This module
 * provides an alternative `WebSocketLike` whose bytes ride an **iroh** QUIC
 * bi-stream (hole-punched or relay-forwarded ciphertext) instead of a LAN
 * WebSocket, so the exact same transport/shim/app stack runs "from anywhere"
 * with zero changes above this line.
 *
 * The QUIC stream is a raw byte pipe; this adapter layers the kind-tagged codec
 * ({@link ./iroh-codec}) on top and performs the account handshake:
 *
 *   1. On construction it dials the desktop's `node_id` (via the injected
 *      {@link IrohDialer}, backed by the lazily-loaded wasm client) and, once the
 *      bi-stream is open, sends the account handshake as its FIRST control frame:
 *      `{"t":"hello-account","grant":"<grant>","nonce":"<browserNonce>"}`.
 *   2. It consumes the desktop's reply internally: `{"t":"welcome",…}` fires
 *      `onopen` (the transport then sends `listen`/`invoke` frames exactly as
 *      over WS); `{"t":"unauthorized","reason":…}` is reported via `onReject`
 *      and closes the stream.
 *   3. A decoupled read pump surfaces every subsequent frame to `onmessage`:
 *      kind-0 text as a `string`, kind-1 binary as an `ArrayBuffer` — the two
 *      shapes `RemoteTransport.handleMessage` branches on.
 *
 * The wasm boundary is abstracted behind {@link IrohByteStream}/{@link IrohDialer}
 * so the adapter is unit-testable with a scripted fake stream and never imports
 * the (optional, lazily-loaded) wasm artifact directly.
 */
import type { WebSocketLike } from "./transport";
import {
  encodeFrame,
  FrameDecoder,
  KIND_TEXT,
  type IrohFrame,
} from "./iroh-codec";

/** A single open iroh bi-stream: a raw, ordered byte pipe. Implemented by the
 *  wasm client; faked in tests. */
export interface IrohByteStream {
  /** Write bytes to the QUIC send stream (write_all + flush). */
  write(data: Uint8Array): Promise<void>;
  /** Read the next chunk, or `null` at end-of-stream (peer finished / closed). */
  read(): Promise<Uint8Array | null>;
  /** Best-effort close of the underlying stream + connection. */
  close(): void;
}

/** Opens an iroh bi-stream to a target `node_id` (over its relay hint when
 *  direct hole-punching is unavailable). */
export interface IrohDialer {
  dial(target: {
    nodeId: string;
    relayUrl?: string | null;
  }): Promise<IrohByteStream>;
}

/** How a handshake rejection is classified for the caller. */
export type IrohRejectKind =
  /** The session is pending desktop approval — retry/poll until approved. */
  | "pending"
  /** Unrecoverable (bad/expired grant, node mismatch) — stop and re-authenticate. */
  | "fatal";

export interface IrohSocketOptions {
  dialer: IrohDialer;
  /** Resolves the target `node_id` (+ optional relay hint) at connect time —
   *  read lazily so a reconnect can pick up a freshly minted descriptor. */
  target(): { nodeId: string; relayUrl?: string | null };
  /** Resolves the `{grant, nonce}` for the `hello-account` first frame. */
  handshake(): { grant: string; nonce: string };
  /** Handshake was rejected by the desktop (never fires on success). */
  onReject?(kind: IrohRejectKind, reason: string): void;
  /** A transport-level failure (dial/read/write/codec error) before or after
   *  open. Advisory only — `onclose` always follows. */
  onError?(err: unknown): void;
}

const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder();

/** The subset of handshake replies the desktop sends before `welcome`. */
interface HandshakeReply {
  t?: string;
  reason?: string;
}

/**
 * A {@link WebSocketLike} whose frames ride an iroh QUIC bi-stream. Construction
 * kicks off the async dial + handshake; `onopen` fires only after the desktop's
 * `welcome`, so the transport's post-open logic is byte-identical to the WS path.
 */
export class IrohWebSocket implements WebSocketLike {
  binaryType = "arraybuffer";
  onopen: ((ev: unknown) => void) | null = null;
  onclose: ((ev: { code?: number; reason?: string }) => void) | null = null;
  onerror: ((ev: unknown) => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;

  private stream: IrohByteStream | null = null;
  private readonly decoder = new FrameDecoder();
  private readonly opts: IrohSocketOptions;
  private welcomed = false;
  private done = false;
  private closedByCaller = false;

  constructor(opts: IrohSocketOptions) {
    this.opts = opts;
    void this.start();
  }

  /** Send a `/ws` text frame (invoke/listen/unlisten). The transport only ever
   *  sends strings; binary client→server frames are not part of the protocol. */
  send(data: string): void {
    if (this.done || !this.stream) return;
    void this.writeFrame(KIND_TEXT, textEncoder.encode(data)).catch((err) =>
      this.fail(err),
    );
  }

  close(): void {
    this.closedByCaller = true;
    this.teardown(undefined);
  }

  // ── Internals ──────────────────────────────────────────────────────

  private async start(): Promise<void> {
    let stream: IrohByteStream;
    try {
      stream = await this.opts.dialer.dial(this.opts.target());
    } catch (err) {
      this.fail(err);
      return;
    }
    if (this.closedByCaller) {
      try {
        stream.close();
      } catch {
        /* ignore */
      }
      return;
    }
    this.stream = stream;
    try {
      const { grant, nonce } = this.opts.handshake();
      const hello = JSON.stringify({ t: "hello-account", grant, nonce });
      await this.writeFrame(KIND_TEXT, textEncoder.encode(hello));
    } catch (err) {
      this.fail(err);
      return;
    }
    void this.pump();
  }

  private async writeFrame(kind: number, payload: Uint8Array): Promise<void> {
    if (!this.stream) throw new Error("iroh: stream not open");
    await this.stream.write(encodeFrame(kind, payload));
  }

  /** Decoupled read pump: refill from the stream, drain complete frames. */
  private async pump(): Promise<void> {
    for (;;) {
      let chunk: Uint8Array | null;
      try {
        chunk = await this.stream!.read();
      } catch (err) {
        this.fail(err);
        return;
      }
      if (this.done) return;
      if (chunk === null) {
        // Clean EOF: the peer finished the stream.
        this.teardown(undefined);
        return;
      }
      this.decoder.push(chunk);
      try {
        let frame: IrohFrame | null;
        while ((frame = this.decoder.next()) !== null) {
          this.onFrame(frame);
          if (this.done) return;
        }
      } catch (err) {
        // Fatal codec violation (unknown kind / oversized) — drop the socket.
        this.fail(err);
        return;
      }
    }
  }

  private onFrame(frame: IrohFrame): void {
    if (!this.welcomed) {
      this.handleHandshakeFrame(frame);
      return;
    }
    if (frame.kind === KIND_TEXT) {
      // Surface as a string so `handleMessage` JSON-parses it.
      this.onmessage?.({ data: textDecoder.decode(frame.payload) });
    } else {
      // Surface binary as an ArrayBuffer (exact length) so `handleMessage`
      // takes the binary-frame branch. `slice()` yields a standalone buffer.
      const copy = frame.payload.slice();
      this.onmessage?.({ data: copy.buffer });
    }
  }

  private handleHandshakeFrame(frame: IrohFrame): void {
    if (frame.kind !== KIND_TEXT) {
      this.fail(new Error("iroh: non-text handshake reply"));
      return;
    }
    let reply: HandshakeReply;
    try {
      reply = JSON.parse(textDecoder.decode(frame.payload)) as HandshakeReply;
    } catch {
      this.fail(new Error("iroh: malformed handshake reply"));
      return;
    }
    if (reply.t === "welcome") {
      this.welcomed = true;
      this.onopen?.({});
      return;
    }
    if (reply.t === "unauthorized") {
      const reason = typeof reply.reason === "string" ? reply.reason : "unknown";
      const kind: IrohRejectKind =
        reason === "pending_approval" ? "pending" : "fatal";
      this.opts.onReject?.(kind, reason);
      // Not a transport error — the stream is simply closed by the desktop.
      this.teardown({ reason });
      return;
    }
    this.fail(new Error(`iroh: unexpected pre-welcome frame "${reply.t}"`));
  }

  /** Report a transport-level failure, then close (drives the reconnect loop). */
  private fail(err: unknown): void {
    if (this.done) return;
    this.opts.onError?.(err);
    this.onerror?.(err);
    this.teardown({ reason: err instanceof Error ? err.message : "error" });
  }

  /** Idempotent close: finish the stream and fire `onclose` exactly once. */
  private teardown(ev: { code?: number; reason?: string } | undefined): void {
    if (this.done) return;
    this.done = true;
    const stream = this.stream;
    this.stream = null;
    if (stream) {
      try {
        stream.close();
      } catch {
        /* ignore */
      }
    }
    this.onclose?.(ev ?? {});
  }
}
