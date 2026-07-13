import { describe, it, expect } from "vitest";
import {
  encodeFrame,
  FrameDecoder,
  IrohCodecError,
  KIND_TEXT,
  KIND_BINARY,
  IROH_MAX_FRAME_LEN,
} from "./iroh-codec";

const enc = new TextEncoder();

describe("encodeFrame", () => {
  it("lays out [kind][u32 BE len][payload]", () => {
    const framed = encodeFrame(KIND_TEXT, enc.encode("hello"));
    expect(framed[0]).toBe(KIND_TEXT);
    // len = 5, big-endian
    expect(Array.from(framed.slice(1, 5))).toEqual([0, 0, 0, 5]);
    expect(new TextDecoder().decode(framed.slice(5))).toBe("hello");
  });

  it("encodes an empty payload as a bare header", () => {
    const framed = encodeFrame(KIND_BINARY, new Uint8Array());
    expect(framed.length).toBe(5);
    expect(framed[0]).toBe(KIND_BINARY);
    expect(Array.from(framed.slice(1, 5))).toEqual([0, 0, 0, 0]);
  });
});

describe("FrameDecoder", () => {
  it("reassembles two concatenated frames of different kinds", () => {
    const dec = new FrameDecoder();
    dec.push(encodeFrame(KIND_TEXT, enc.encode('{"t":"invoke"}')));
    dec.push(encodeFrame(KIND_BINARY, new Uint8Array([0x01, 0xaa, 0xbb])));

    const a = dec.next()!;
    expect(a.kind).toBe(KIND_TEXT);
    expect(new TextDecoder().decode(a.payload)).toBe('{"t":"invoke"}');
    const b = dec.next()!;
    expect(b.kind).toBe(KIND_BINARY);
    expect(Array.from(b.payload)).toEqual([0x01, 0xaa, 0xbb]);
    expect(dec.next()).toBeNull();
  });

  it("never yields a frame early when fed one byte at a time", () => {
    const payload = enc.encode('{"t":"ok","id":1,"data":null}');
    const framed = encodeFrame(KIND_TEXT, payload);
    const dec = new FrameDecoder();
    for (let i = 0; i < framed.length; i++) {
      dec.push(framed.subarray(i, i + 1));
      if (i + 1 < framed.length) {
        expect(dec.next()).toBeNull();
      }
    }
    const frame = dec.next()!;
    expect(frame.kind).toBe(KIND_TEXT);
    expect(Array.from(frame.payload)).toEqual(Array.from(payload));
  });

  it("splits a frame arriving as a header chunk then a body chunk", () => {
    const framed = encodeFrame(KIND_BINARY, new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8]));
    const dec = new FrameDecoder();
    dec.push(framed.subarray(0, 4)); // header + first payload bytes only
    expect(dec.next()).toBeNull();
    dec.push(framed.subarray(4));
    const frame = dec.next()!;
    expect(Array.from(frame.payload)).toEqual([1, 2, 3, 4, 5, 6, 7, 8]);
  });

  it("preserves frames when a chunk carries one whole + one partial frame", () => {
    const whole = encodeFrame(KIND_TEXT, enc.encode("aa"));
    const partial = encodeFrame(KIND_BINARY, new Uint8Array([9, 9, 9]));
    const combined = new Uint8Array(whole.length + 2);
    combined.set(whole, 0);
    combined.set(partial.subarray(0, 2), whole.length); // only 2 bytes of the 2nd
    const dec = new FrameDecoder();
    dec.push(combined);
    const first = dec.next()!;
    expect(new TextDecoder().decode(first.payload)).toBe("aa");
    expect(dec.next()).toBeNull(); // second frame still incomplete
    dec.push(partial.subarray(2));
    const second = dec.next()!;
    expect(Array.from(second.payload)).toEqual([9, 9, 9]);
  });

  it("throws on an unknown kind byte", () => {
    const dec = new FrameDecoder();
    dec.push(new Uint8Array([0x09, 0, 0, 0, 1, 0xff])); // kind 9
    expect(() => dec.next()).toThrowError(IrohCodecError);
    try {
      new FrameDecoder();
      const d = new FrameDecoder();
      d.push(new Uint8Array([0x09, 0, 0, 0, 1, 0xff]));
      d.next();
    } catch (e) {
      expect((e as IrohCodecError).kind).toBe("unknown_kind");
      expect((e as IrohCodecError).value).toBe(9);
    }
  });

  it("throws on a declared length above the frame cap", () => {
    const dec = new FrameDecoder();
    const tooBig = IROH_MAX_FRAME_LEN + 1;
    const header = new Uint8Array(5);
    header[0] = KIND_TEXT;
    new DataView(header.buffer).setUint32(1, tooBig, false);
    dec.push(header);
    expect(() => dec.next()).toThrowError(IrohCodecError);
    try {
      const d = new FrameDecoder();
      d.push(header);
      d.next();
    } catch (e) {
      expect((e as IrohCodecError).kind).toBe("frame_too_large");
    }
  });

  it("round-trips a byte-exact PTY frame through encode + decode", () => {
    // The [0x01][u32 ch][u64 idx][payload] PTY frame nested inside a kind-1
    // codec frame must survive unchanged, including embedded control bytes.
    const pty = new Uint8Array([
      0x01, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 42, 0x1b, 0x5b, 0x30, 0x6d, 0x00,
    ]);
    const dec = new FrameDecoder();
    dec.push(encodeFrame(KIND_BINARY, pty));
    const frame = dec.next()!;
    expect(frame.kind).toBe(KIND_BINARY);
    expect(Array.from(frame.payload)).toEqual(Array.from(pty));
  });
});
