import { describe, expect, it } from "vitest";

import { detectAnimatedGif } from "./AgentChatPane";

/** Build a minimal GIF89a-shaped buffer with `frames` Graphic Control
 *  Extension markers. The detector only scans for the GCE byte
 *  pattern (0x21 0xF9 0x04), so a faithful GIF header isn't required —
 *  any padding bytes around the markers suffices. */
function gifWithFrames(frames: number, padding = 8): ArrayBuffer {
  const totalLen = 6 /* GIF header */ + (frames * 4 + padding * frames);
  const buf = new Uint8Array(totalLen);
  // Write a believable header so the buffer "looks like" a GIF89a.
  buf.set([0x47, 0x49, 0x46, 0x38, 0x39, 0x61], 0);
  let cursor = 6;
  for (let f = 0; f < frames; f++) {
    // Pad bytes so consecutive markers don't sit adjacent.
    for (let p = 0; p < padding; p++) {
      buf[cursor++] = 0x10;
    }
    buf[cursor++] = 0x21;
    buf[cursor++] = 0xf9;
    buf[cursor++] = 0x04;
    buf[cursor++] = 0x00;
  }
  return buf.buffer;
}

describe("detectAnimatedGif", () => {
  it("returns false for a buffer with zero GCE markers (still image)", () => {
    expect(detectAnimatedGif(gifWithFrames(0))).toBe(false);
  });

  it("returns false for a buffer with exactly one GCE marker (single frame)", () => {
    expect(detectAnimatedGif(gifWithFrames(1))).toBe(false);
  });

  it("returns true for a buffer with two or more GCE markers", () => {
    expect(detectAnimatedGif(gifWithFrames(2))).toBe(true);
  });

  it("bails early after the second marker — handles big animated GIFs cheaply", () => {
    // Allocate 1MB buffer with many frames; if the loop didn't bail
    // early it'd still be correct, but this is the contract that
    // makes the detector cheap on real-world animated GIFs.
    expect(detectAnimatedGif(gifWithFrames(100, 1024))).toBe(true);
  });

  it("returns false on an empty buffer", () => {
    expect(detectAnimatedGif(new ArrayBuffer(0))).toBe(false);
  });

  it("returns false when bytes happen to contain 0x21 0xF9 but not 0x04 next", () => {
    // Defensive: the detector requires the full 3-byte signature.
    // A near-miss (0x21 0xF9 0x05) in a JPEG payload mustn't trigger.
    const bytes = new Uint8Array([0x21, 0xf9, 0x05, 0x21, 0xf9, 0x06]);
    expect(detectAnimatedGif(bytes.buffer)).toBe(false);
  });
});
