/**
 * Web-remote WebSocket transport.
 *
 * The single wire between a plain-browser Codemux UI and the desktop
 * app's embedded web-remote server. It owns:
 *
 *   - The ticket handshake: `POST /api/ws-ticket` (Bearer session token)
 *     yields a short-lived single-use ticket, then `GET /ws?ticket=…`
 *     upgrades to a socket. Session tokens never appear in a WS URL.
 *   - The invoke request/response map (`{t:"invoke"}` → `{t:"ok"|"err"}`,
 *     id-matched, out-of-order allowed).
 *   - Event subscriptions (`{t:"listen"}` / `{t:"unlisten"}`), re-sent in
 *     full after every reconnect.
 *   - Channel frame parsing — JSON `{t:"chan"}` bodies and the binary
 *     `[0x01][u32 BE cb][u64 BE idx][payload]` framing for PTY bytes —
 *     handed to the shim as `(callbackId, index, message)`.
 *   - Negotiated inbound compression: the client offers `compress=deflate`
 *     on the WS URL and, if the server takes it up, unwraps the
 *     `[0x02]`/`[0x03]` compressed envelopes back into ordinary text and
 *     binary frames (see `FrameInflater`).
 *   - Reconnect with exponential backoff (1s → 16s cap): re-issue a
 *     ticket, reopen, re-subscribe. In-flight invokes reject on the drop;
 *     the UI's own `get_app_state`-on-mount + snapshot events self-heal.
 *
 * The protocol contract is defined in `docs/plans/web-remote-access.md`
 * ("WS protocol contract") and MUST match the Rust server exactly.
 */
import { Inflate } from "fflate";

export type ConnectionStatus = "connecting" | "connected" | "reconnecting";

/** Callbacks the transport fires into the shim. */
export interface TransportHooks {
  /** A channel body arrived (JSON `chan` frame or a binary frame). */
  onChannelMessage(callbackId: number, index: number, message: unknown): void;
  /** A backend event arrived for a subscribed event name. */
  onEvent(event: string, payload: unknown): void;
  /** Connection state changed (drives the reconnecting banner). */
  onStatusChange(status: ConnectionStatus): void;
  /** The session is no longer valid (revoked / rejected). */
  onUnauthorized(): void;
}

/**
 * The subset of the `WebSocket` API the transport uses. Declared as an
 * interface so tests can inject a scripted fake without a real socket.
 */
export interface WebSocketLike {
  binaryType: string;
  send(data: string): void;
  close(code?: number, reason?: string): void;
  onopen: ((ev: unknown) => void) | null;
  onclose: ((ev: { code?: number; reason?: string }) => void) | null;
  onerror: ((ev: unknown) => void) | null;
  onmessage: ((ev: { data: unknown }) => void) | null;
}

/** Injectable seams (defaults use the browser globals). */
export interface TransportDeps {
  fetchImpl?: typeof fetch;
  wsFactory?: (url: string) => WebSocketLike;
}

export interface TransportOptions {
  /** Server origin, e.g. `https://box.tailnet.ts.net` or `http://…:4377`. */
  baseUrl: string;
  /** Current session bearer token, or `null` (cookie fallback then). */
  getToken(): string | null;
  hooks: TransportHooks;
  deps?: TransportDeps;
}

/** Rejected-invoke / ticket errors carry a flag so the loop can decide
 *  between "retry with backoff" and "give up, session is dead". */
class UnauthorizedError extends Error {
  constructor() {
    super("web-remote: session unauthorized");
    this.name = "UnauthorizedError";
  }
}

const BASE_RECONNECT_MS = 1000;
const MAX_RECONNECT_MS = 16000;

/** Minimum length of a binary channel frame: tag(1) + cb(4) + idx(8). */
const BINARY_HEADER_LEN = 13;
const BINARY_FRAME_TAG = 0x01;

interface ParsedBinaryFrame {
  callbackId: number;
  index: number;
  payload: Uint8Array;
}

/**
 * Decode a binary channel frame `[0x01][u32 BE cb][u64 BE idx][payload]`.
 * Returns `null` for anything that isn't a well-formed channel frame.
 * Exported for direct unit testing.
 */
export function parseBinaryChannelFrame(
  buffer: ArrayBuffer,
): ParsedBinaryFrame | null {
  if (buffer.byteLength < BINARY_HEADER_LEN) return null;
  const view = new DataView(buffer);
  if (view.getUint8(0) !== BINARY_FRAME_TAG) return null;
  const callbackId = view.getUint32(1, false);
  // u64 big-endian index read as hi*2^32 + lo. PTY indices stay well
  // under 2^53, so a JS number preserves ordering exactly.
  const hi = view.getUint32(5, false);
  const lo = view.getUint32(9, false);
  const index = hi * 2 ** 32 + lo;
  const payload = new Uint8Array(buffer, BINARY_HEADER_LEN);
  return { callbackId, index, payload };
}

/** Compressed envelopes: an inflated text frame / an inflated binary frame. */
const COMPRESSED_TEXT_TAG = 0x02;
const COMPRESSED_BINARY_TAG = 0x03;
/** Compressed envelope header: tag(1) + u32 BE uncompressed length. */
const COMPRESSED_HEADER_LEN = 5;

const textDecoder = new TextDecoder();

/**
 * Ceiling on a compressed envelope's declared uncompressed length. Matches the
 * 64 MiB inbound frame cap the server side enforces (tungstenite's default
 * message limit on `/ws`, and `iroh::MAX_FRAME_LEN` on the relay path), so no
 * legitimate frame can declare more. Without the check the declared `u32` is
 * attacker-supplied input to `new Uint8Array(…)`: 4 GiB would either throw a
 * `RangeError` straight out of `ws.onmessage` or balloon the tab's heap.
 */
const MAX_INFLATED_BYTES = 64 * 1024 * 1024;

/**
 * Per-connection raw-inflate stream for the server's compressed frames.
 *
 * tungstenite has no permessage-deflate, so compression is done at the
 * application level: the server runs ONE raw-deflate stream (32 KiB window)
 * per connection and sync-flushes after every message, which means the
 * `00 00 FF FF` boundary ships inside the frame and the sliding-window
 * dictionary carries into the next one (context takeover). Decoding is
 * therefore stateful — frames must be pushed in arrival order, and every
 * new connection starts from a fresh instance on both sides.
 *
 * A malformed frame desyncs that dictionary beyond repair (later frames would
 * inflate to garbage), so the first failure *poisons* the stream: the frame is
 * dropped, one line is logged, and `onPoison` fires. Dropping later frames
 * silently is not an option — a lost `ok`/`err` frame would hang its invoke
 * forever and a lost `0x03` PTY frame would stall the `Channel` reorder buffer
 * on the missing index — so the transport's `onPoison` closes the socket. That
 * rejects in-flight invokes, flips the status to reconnecting, and the
 * reconnect's `reset` gives both sides a fresh stream; any compressed frame
 * still in flight on the dying socket is dropped on the way. Plain text and
 * `0x01` binary frames never touch this stream.
 *
 * Exported for direct unit testing.
 */
export class FrameInflater {
  private stream: Inflate | null = null;
  private poisoned = false;

  /**
   * @param onPoison Fired once, on the failure that poisons the stream. The
   *   transport uses it to tear the socket down (see the class note); a bare
   *   `FrameInflater` under test can leave it out.
   */
  constructor(private readonly onPoison?: () => void) {}

  /**
   * Decode one `[tag][u32 BE uncompressed_len][deflate-raw bytes]` envelope
   * into the exact bytes the server compressed. Returns `null` for anything
   * malformed (already logged) — callers drop the frame; the poison escalation
   * takes care of getting the connection back to a decodable state.
   */
  decode(frame: ArrayBuffer): Uint8Array | null {
    if (this.poisoned) return null;
    if (frame.byteLength < COMPRESSED_HEADER_LEN) {
      this.poison("compressed frame is shorter than its header");
      return null;
    }
    const expected = new DataView(frame).getUint32(1, false);
    // Validate before allocating: see `MAX_INFLATED_BYTES`.
    if (expected > MAX_INFLATED_BYTES) {
      this.poison(
        `compressed frame declares ${expected} inflated bytes, above the ${MAX_INFLATED_BYTES} cap`,
      );
      return null;
    }
    const stream = (this.stream ??= new Inflate());
    try {
      // Belt and braces: the cap above already rules out a hostile length, and
      // an allocation failure here is caught rather than escaping `onmessage`
      // with the dictionary silently desynced.
      const out = new Uint8Array(expected);
      let written = 0;
      let overflowed = false;
      // `ondata` fires synchronously inside `push`, so arrival order is
      // preserved with no async hop. Each chunk is already a fresh allocation;
      // copying into `out` is what assembles one contiguous frame and enforces
      // the declared length.
      stream.ondata = (chunk) => {
        if (written + chunk.length > expected) {
          overflowed = true;
          return;
        }
        out.set(chunk, written);
        written += chunk.length;
      };
      stream.push(new Uint8Array(frame, COMPRESSED_HEADER_LEN));
      if (overflowed || written !== expected) {
        this.poison(
          `inflated length mismatch (declared ${expected}, got ${
            overflowed ? "more" : written
          })`,
        );
        return null;
      }
      return out;
    } catch (err) {
      this.poison(`inflate failed: ${String(err)}`);
      return null;
    }
  }

  /** Start over — the server opens a fresh deflate stream per connection. */
  reset(): void {
    this.stream = null;
    this.poisoned = false;
  }

  private poison(reason: string): void {
    this.poisoned = true;
    console.error(`[web-remote] ${reason}; reconnecting for a fresh stream`);
    this.onPoison?.();
  }
}

interface PendingInvoke {
  resolve(value: unknown): void;
  reject(reason: unknown): void;
}

export class RemoteTransport {
  private readonly baseUrl: string;
  private readonly getToken: () => string | null;
  private readonly hooks: TransportHooks;
  private readonly fetchImpl: typeof fetch;
  private readonly wsFactory: (url: string) => WebSocketLike;

  private ws: WebSocketLike | null = null;
  private socketOpen = false;
  private closed = false;

  /** Inbound-compression state; reset for every socket (see `FrameInflater`). */
  private readonly inflater = new FrameInflater(() => this.dropPoisonedSocket());

  private nextInvokeId = 1;
  private readonly pending = new Map<number, PendingInvoke>();
  /** Active event subscriptions, re-sent on every (re)connect. */
  private readonly subscriptions = new Set<string>();

  private reconnectDelay = BASE_RECONNECT_MS;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;

  private firstConnectResolve: (() => void) | null = null;
  private firstConnectReject: ((reason: unknown) => void) | null = null;

  constructor(opts: TransportOptions) {
    this.baseUrl = opts.baseUrl.replace(/\/+$/, "");
    this.getToken = opts.getToken;
    this.hooks = opts.hooks;
    this.fetchImpl = opts.deps?.fetchImpl ?? ((...a) => fetch(...a));
    this.wsFactory =
      opts.deps?.wsFactory ??
      ((url) => new WebSocket(url) as unknown as WebSocketLike);
  }

  /**
   * Open the socket. Resolves once the first connection is live; rejects
   * only if the session is unauthorized on that first attempt. Transient
   * failures keep retrying (the caller shows a connecting/waiting state).
   */
  connect(): Promise<void> {
    this.closed = false;
    return new Promise<void>((resolve, reject) => {
      this.firstConnectResolve = resolve;
      this.firstConnectReject = reject;
      void this.attemptConnect();
    });
  }

  /** Tear down intentionally (no reconnect). */
  close(): void {
    this.closed = true;
    if (this.reconnectTimer !== null) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    this.socketOpen = false;
    const ws = this.ws;
    this.ws = null;
    if (ws) {
      ws.onopen = ws.onclose = ws.onerror = ws.onmessage = null;
      try {
        ws.close();
      } catch {
        /* ignore */
      }
    }
  }

  /** Send an invoke; reject immediately if the socket is down. */
  invoke(cmd: string, args: Record<string, unknown> = {}): Promise<unknown> {
    if (!this.socketOpen || !this.ws) {
      return Promise.reject(
        new Error(`web-remote: not connected (cannot invoke "${cmd}")`),
      );
    }
    const id = this.nextInvokeId++;
    return new Promise<unknown>((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      try {
        // JSON.stringify converts `Channel` args to "__CHANNEL__:<id>"
        // strings via their `toJSON`; the shim's transformCallback minted
        // those ids and holds the callbacks.
        this.ws!.send(JSON.stringify({ t: "invoke", id, cmd, args }));
      } catch (err) {
        this.pending.delete(id);
        reject(err);
      }
    });
  }

  /** Register a backend event subscription (first subscriber only). */
  subscribe(event: string): void {
    if (this.subscriptions.has(event)) return;
    this.subscriptions.add(event);
    if (this.socketOpen && this.ws) {
      this.ws.send(JSON.stringify({ t: "listen", event }));
    }
  }

  /** Drop a backend event subscription (last subscriber only). */
  unsubscribe(event: string): void {
    if (!this.subscriptions.has(event)) return;
    this.subscriptions.delete(event);
    if (this.socketOpen && this.ws) {
      this.ws.send(JSON.stringify({ t: "unlisten", event }));
    }
  }

  // ── Internals ──────────────────────────────────────────────────────

  private async attemptConnect(): Promise<void> {
    if (this.closed) return;
    let ticket: string;
    try {
      ticket = await this.fetchTicket();
    } catch (err) {
      if (err instanceof UnauthorizedError) {
        this.closed = true;
        this.hooks.onUnauthorized();
        this.firstConnectReject?.(err);
        this.firstConnectReject = this.firstConnectResolve = null;
        return;
      }
      this.scheduleReconnect();
      return;
    }
    if (this.closed) return;

    let ws: WebSocketLike;
    try {
      ws = this.wsFactory(this.wsUrl(ticket));
    } catch {
      this.scheduleReconnect();
      return;
    }
    // A fresh socket means a fresh server-side deflate stream.
    this.inflater.reset();
    ws.binaryType = "arraybuffer";
    ws.onopen = () => this.handleOpen();
    ws.onmessage = (ev) => this.handleMessage(ev.data);
    ws.onclose = () => this.handleClose();
    ws.onerror = () => {
      /* an `onclose` always follows — reconnect is driven from there */
    };
    this.ws = ws;
  }

  private async fetchTicket(): Promise<string> {
    const token = this.getToken();
    const headers: Record<string, string> = {};
    if (token) headers.Authorization = `Bearer ${token}`;
    const res = await this.fetchImpl(`${this.baseUrl}/api/ws-ticket`, {
      method: "POST",
      headers,
      credentials: "include",
    });
    // 401 → the session is gone (revoked / invalid). 403 → valid but not
    // yet approved: transient, keep polling until the desktop approves.
    if (res.status === 401) throw new UnauthorizedError();
    if (!res.ok) throw new Error(`web-remote: ws-ticket failed (${res.status})`);
    const body = (await res.json()) as { ticket?: unknown };
    if (typeof body.ticket !== "string" || body.ticket.length === 0) {
      throw new Error("web-remote: ws-ticket response missing ticket");
    }
    return body.ticket;
  }

  private wsUrl(ticket: string): string {
    const wsBase = this.baseUrl.replace(/^http/i, "ws");
    // `compress=deflate` *offers* application-level compression; its mere
    // presence is the whole negotiation. A server that doesn't know the
    // param just never sends `0x02`/`0x03` frames, so an older desktop keeps
    // working untouched. C→S frames are never compressed.
    return `${wsBase}/ws?ticket=${encodeURIComponent(ticket)}&compress=deflate`;
  }

  private handleOpen(): void {
    this.socketOpen = true;
    this.reconnectDelay = BASE_RECONNECT_MS;
    // Re-send every active subscription so the server re-registers its
    // `listen_any` hooks for this fresh socket.
    for (const event of this.subscriptions) {
      this.ws?.send(JSON.stringify({ t: "listen", event }));
    }
    this.hooks.onStatusChange("connected");
    if (this.firstConnectResolve) {
      this.firstConnectResolve();
      this.firstConnectResolve = this.firstConnectReject = null;
    }
  }

  private handleClose(): void {
    const wasOpen = this.socketOpen;
    this.socketOpen = false;
    this.ws = null;
    // Reject in-flight invokes — their responses will never arrive.
    if (this.pending.size > 0) {
      const err = new Error("web-remote: connection lost");
      for (const p of this.pending.values()) p.reject(err);
      this.pending.clear();
    }
    if (this.closed) return;
    if (wasOpen) this.hooks.onStatusChange("reconnecting");
    this.scheduleReconnect();
  }

  /**
   * The inflate stream desynced, so nothing else on this socket can be
   * trusted or even parsed. Close it and let the normal drop path do the
   * recovery: `handleClose` rejects in-flight invokes (a dropped `ok`/`err`
   * frame would otherwise hang forever — there is no invoke timeout), reports
   * `reconnecting`, and schedules the reconnect whose `attemptConnect` resets
   * the inflater against the server's fresh deflate stream.
   */
  private dropPoisonedSocket(): void {
    try {
      this.ws?.close();
    } catch {
      /* already gone — `handleClose` still runs, or already ran */
    }
  }

  private scheduleReconnect(): void {
    if (this.closed || this.reconnectTimer !== null) return;
    const delay = this.reconnectDelay;
    this.reconnectDelay = Math.min(this.reconnectDelay * 2, MAX_RECONNECT_MS);
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      void this.attemptConnect();
    }, delay);
  }

  /** Both transports (WS and iroh) surface strings and `ArrayBuffer`s here. */
  private handleMessage(data: unknown): void {
    if (data instanceof ArrayBuffer) {
      this.handleBinaryMessage(data);
      return;
    }
    if (typeof data !== "string") return;
    this.handleTextMessage(data);
  }

  /**
   * Route a binary message: the plain `[0x01]` channel framing, or a
   * compressed envelope that unwraps to a text or binary frame and is then
   * handled exactly as if it had arrived in that uncompressed shape.
   */
  private handleBinaryMessage(buffer: ArrayBuffer): void {
    if (buffer.byteLength > 0) {
      const tag = new DataView(buffer).getUint8(0);
      if (tag === COMPRESSED_TEXT_TAG) {
        const inflated = this.inflater.decode(buffer);
        if (inflated) this.handleTextMessage(textDecoder.decode(inflated));
        return;
      }
      if (tag === COMPRESSED_BINARY_TAG) {
        // Inflates to a whole binary frame, `0x01` tag included.
        const inflated = this.inflater.decode(buffer);
        if (inflated) this.deliverBinaryChannelFrame(inflated.buffer);
        return;
      }
    }
    this.deliverBinaryChannelFrame(buffer);
  }

  private deliverBinaryChannelFrame(buffer: ArrayBuffer): void {
    const frame = parseBinaryChannelFrame(buffer);
    if (frame) {
      this.hooks.onChannelMessage(frame.callbackId, frame.index, frame.payload);
    }
  }

  private handleTextMessage(data: string): void {
    let msg: {
      t?: string;
      id?: number;
      data?: unknown;
      error?: unknown;
      event?: string;
      payload?: unknown;
      ch?: number;
      idx?: number;
    };
    try {
      msg = JSON.parse(data);
    } catch {
      return;
    }
    switch (msg.t) {
      case "ok": {
        const p = msg.id !== undefined ? this.pending.get(msg.id) : undefined;
        if (p && msg.id !== undefined) {
          this.pending.delete(msg.id);
          p.resolve(msg.data);
        }
        return;
      }
      case "err": {
        const p = msg.id !== undefined ? this.pending.get(msg.id) : undefined;
        if (p && msg.id !== undefined) {
          this.pending.delete(msg.id);
          p.reject(msg.error);
        }
        return;
      }
      case "event":
        if (typeof msg.event === "string") {
          this.hooks.onEvent(msg.event, msg.payload);
        }
        return;
      case "chan":
        if (typeof msg.ch === "number" && typeof msg.idx === "number") {
          this.hooks.onChannelMessage(msg.ch, msg.idx, msg.data);
        }
        return;
      default:
        return;
    }
  }
}
