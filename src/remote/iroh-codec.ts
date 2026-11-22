/**
 * Kind-tagged, length-delimited frame codec — the browser twin of the Rust
 * codec in `src-tauri/src/web_remote/iroh.rs`.
 *
 * An iroh QUIC bi-stream is a boundary-less byte stream, so every logical
 * `/ws` frame is wrapped as `[u8 kind][u32 BE len][payload]`:
 *
 *   - `kind = 0` ({@link KIND_TEXT}): a UTF-8 JSON control frame — identical
 *     JSON to the `/ws` text frames (`invoke`/`ok`/`err`/`listen`/`event`/`chan`
 *     + the account handshake frames).
 *   - `kind = 1` ({@link KIND_BINARY}): a raw binary payload — the same
 *     `[0x01][u32 BE ch][u64 BE idx][payload]` PTY frame the WS binary path
 *     carries. The PTY framing is the *payload* of a kind-1 codec frame; the two
 *     framings nest, they never collide.
 *
 * {@link FrameDecoder} reassembles discrete frames from arbitrary read chunks
 * (QUIC packet boundaries fall anywhere), exactly as the Rust `FrameDecoder`
 * does. Every constant here MUST match the Rust peer byte-for-byte.
 */

/** Control frame: a UTF-8 JSON payload (the `/ws` text-frame shapes). */
export const KIND_TEXT = 0;
/** Binary frame: a raw payload (the `[0x01]…` PTY framing). */
export const KIND_BINARY = 1;

/** Frame header: `[u8 kind][u32 BE len]`. */
const HEADER_LEN = 5;

/**
 * Largest single frame accepted, matching the Rust `MAX_FRAME_LEN` (axum's
 * default 64 MiB WS message cap). A declared length above this is fatal rather
 * than letting a peer make us buffer unbounded memory.
 */
export const IROH_MAX_FRAME_LEN = 64 * 1024 * 1024;

/** Retain one iroh read window's worth of scratch space between frames. */
const RETAINED_BUFFER_LIMIT = 64 * 1024;

/** One decoded wire frame. */
export interface IrohFrame {
  kind: number;
  /** Owned payload bytes. */
  payload: Uint8Array;
}

/**
 * A fatal framing violation — the read pump closes the connection on either,
 * mirroring the Rust `CodecError`.
 */
export class IrohCodecError extends Error {
  constructor(
    readonly kind: "frame_too_large" | "unknown_kind",
    readonly value: number,
  ) {
    super(
      kind === "frame_too_large"
        ? `iroh frame too large: ${value} bytes`
        : `iroh frame unknown kind: ${value}`,
    );
    this.name = "IrohCodecError";
  }
}

/** Encode one frame: `[kind][u32 BE len][payload]`. */
export function encodeFrame(kind: number, payload: Uint8Array): Uint8Array {
  if (payload.length > IROH_MAX_FRAME_LEN) {
    throw new IrohCodecError("frame_too_large", payload.length);
  }
  const out = new Uint8Array(HEADER_LEN + payload.length);
  const view = new DataView(out.buffer);
  view.setUint8(0, kind);
  view.setUint32(1, payload.length, false);
  out.set(payload, HEADER_LEN);
  return out;
}

/**
 * Incremental decoder that reassembles discrete {@link IrohFrame}s from the
 * boundary-less QUIC byte stream. Feed it read chunks with {@link push}; pull
 * complete frames with {@link next} (returns `null` when only a partial frame is
 * buffered — the caller reads more and retries). A malformed header (unknown
 * kind / oversized length) throws {@link IrohCodecError}, exactly as the Rust
 * decoder returns a fatal `CodecError`.
 */
export class FrameDecoder {
  /** Unread bytes occupy the half-open range [start, end). */
  private buf = new Uint8Array(0);
  private start = 0;
  private end = 0;

  /** Append freshly read bytes. */
  push(chunk: Uint8Array): void {
    if (chunk.length === 0) return;
    if (chunk.length > this.buf.length - this.end) {
      const unread = this.buffered;
      const needed = unread + chunk.length;
      if (needed > this.buf.length) {
        // Grow geometrically so a large fragmented frame does not copy
        // its entire prefix each time another chunk arrives.
        const grown = Math.min(this.buf.length * 2, HEADER_LEN + IROH_MAX_FRAME_LEN);
        const buf = new Uint8Array(Math.max(needed, grown));
        buf.set(this.buf.subarray(this.start, this.end));
        this.buf = buf;
      } else {
        this.buf.copyWithin(0, this.start, this.end);
      }
      this.start = 0;
      this.end = unread;
    }
    this.buf.set(chunk, this.end);
    this.end += chunk.length;
  }

  /**
   * Pull the next complete frame, or `null` if the buffer holds only a partial
   * frame. Throws {@link IrohCodecError} on a malformed header.
   */
  next(): IrohFrame | null {
    if (this.buffered < HEADER_LEN) return null;
    const kind = this.buf[this.start];
    if (kind !== KIND_TEXT && kind !== KIND_BINARY) {
      throw new IrohCodecError("unknown_kind", kind);
    }
    const view = new DataView(this.buf.buffer);
    const len = view.getUint32(this.start + 1, false);
    if (len > IROH_MAX_FRAME_LEN) {
      throw new IrohCodecError("frame_too_large", len);
    }
    if (this.buffered < HEADER_LEN + len) return null;
    const payloadStart = this.start + HEADER_LEN;
    this.start = payloadStart + len;
    const payload = this.buf.slice(payloadStart, this.start);
    // Release a large frame's backing storage even if the same read
    // contained the start of the next frame.
    if (this.buf.length > RETAINED_BUFFER_LIMIT && this.buffered <= RETAINED_BUFFER_LIMIT) {
      this.buf = this.buf.slice(this.start, this.end);
      this.start = 0;
      this.end = this.buf.length;
    } else if (this.start === this.end) {
      this.start = this.end = 0;
    }
    return { kind, payload };
  }

  /** Bytes currently buffered (partial frame). Exposed for tests. */
  get buffered(): number {
    return this.end - this.start;
  }
}
