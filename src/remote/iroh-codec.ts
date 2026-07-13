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

/** One decoded wire frame. */
export interface IrohFrame {
  kind: number;
  /** Payload bytes. May be a view into the decoder's buffer — copy before
   *  retaining it past the next {@link FrameDecoder.next} call. */
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
  /** Buffered, not-yet-consumed bytes. */
  private buf = new Uint8Array(0);

  /** Append freshly read bytes. */
  push(chunk: Uint8Array): void {
    if (chunk.length === 0) return;
    if (this.buf.length === 0) {
      // Copy so a caller reusing its read buffer can't mutate ours.
      this.buf = chunk.slice();
      return;
    }
    const merged = new Uint8Array(this.buf.length + chunk.length);
    merged.set(this.buf, 0);
    merged.set(chunk, this.buf.length);
    this.buf = merged;
  }

  /**
   * Pull the next complete frame, or `null` if the buffer holds only a partial
   * frame. Throws {@link IrohCodecError} on a malformed header.
   */
  next(): IrohFrame | null {
    if (this.buf.length < HEADER_LEN) return null;
    const kind = this.buf[0];
    if (kind !== KIND_TEXT && kind !== KIND_BINARY) {
      throw new IrohCodecError("unknown_kind", kind);
    }
    const view = new DataView(
      this.buf.buffer,
      this.buf.byteOffset,
      this.buf.byteLength,
    );
    const len = view.getUint32(1, false);
    if (len > IROH_MAX_FRAME_LEN) {
      throw new IrohCodecError("frame_too_large", len);
    }
    if (this.buf.length < HEADER_LEN + len) return null;
    // Copy the payload out so consuming the buffer (below) can't alias it.
    const payload = this.buf.slice(HEADER_LEN, HEADER_LEN + len);
    // subarray shares memory (no copy) — the consumed prefix is simply dropped.
    this.buf = this.buf.subarray(HEADER_LEN + len);
    return { kind, payload };
  }

  /** Bytes currently buffered (partial frame). Exposed for tests. */
  get buffered(): number {
    return this.buf.length;
  }
}
