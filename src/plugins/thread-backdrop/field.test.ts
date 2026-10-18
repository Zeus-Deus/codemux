import { describe, expect, it } from "vitest";
import {
  CELL_H,
  CELL_W,
  OPEN_S,
  PULSE_S,
  buildField,
  computeFrame,
  decodeCell,
  isMoving,
  setCalmZone,
} from "./field";
import { pickBackdropArt } from "./Backdrop";
import { PLATES } from "@codemux/sdk";

const W = 700;
const H = 480;
const COLS = Math.ceil(W / CELL_W);
const ROWS = Math.ceil(H / CELL_H);

/** A plate that is solid ink in the middle and blank around it. */
function solidPlate() {
  const alpha = new Uint8Array(COLS * ROWS);
  for (let r = 0; r < ROWS; r++) {
    for (let c = 0; c < COLS; c++) {
      const inside = Math.abs(c - COLS / 2) < COLS / 4 && Math.abs(r - ROWS / 2) < ROWS / 4;
      alpha[r * COLS + c] = inside ? 255 : 0;
    }
  }
  return buildField(alpha, W, H);
}

function frame(field: ReturnType<typeof solidPlate>, t: number, pulses = [] as { x: number; y: number; age: number }[]) {
  const out = new Uint16Array(field.cols * field.rows);
  computeFrame(field, t, pulses, out);
  return out;
}

function changed(a: Uint16Array, b: Uint16Array) {
  let n = 0;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) n++;
  return n;
}

describe("thread backdrop field", () => {
  it("only keeps cells that carry ink", () => {
    const field = solidPlate();
    expect(field.active.length).toBeGreaterThan(0);
    expect(field.active.length).toBeLessThan(field.cols * field.rows / 2);
    for (const i of field.active) expect(field.ink[i]).toBeGreaterThan(0);
  });

  it("is blank before it prints and types in from the top", () => {
    const field = solidPlate();
    expect(frame(field, 0).every((v) => v === 0)).toBe(true);
    const early = frame(field, 0.55);
    const printedRows = new Set<number>();
    for (const i of field.active) if (early[i] !== 0) printedRows.add(Math.floor(i / field.cols));
    const top = Math.min(...field.active.map((i) => Math.floor(i / field.cols)));
    const bottom = Math.max(...field.active.map((i) => Math.floor(i / field.cols)));
    expect(printedRows.has(top)).toBe(true);
    expect(printedRows.has(bottom)).toBe(false);
  });

  it("settles: once open, consecutive frames are identical", () => {
    const field = solidPlate();
    const a = frame(field, OPEN_S + 0.01);
    const b = frame(field, OPEN_S + 5);
    expect(changed(a, b)).toBe(0);
    expect(frame(field, Infinity)).toEqual(a);
    expect(isMoving(OPEN_S + 0.01, [])).toBe(false);
  });

  it("a click ring changes only the cells it passes over, then settles", () => {
    const field = solidPlate();
    const rest = frame(field, Infinity);
    const mid = { x: W / 2, y: H / 2 };
    const ringed = frame(field, Infinity, [{ ...mid, age: 0.1 }]);
    expect(changed(rest, ringed)).toBeGreaterThan(0);
    // At 0.1s the ring is ~95px out with a 70px half-width: every changed
    // cell lies in that band, so the cells inside and outside it stay put.
    const radius = 0.1 * 950;
    for (const i of field.active) {
      if (ringed[i] === rest[i]) continue;
      const x = ((i % field.cols) + 0.5) * CELL_W;
      const y = (Math.floor(i / field.cols) + 0.5) * CELL_H;
      expect(Math.abs(Math.hypot(x - mid.x, y - mid.y) - radius)).toBeLessThan(70);
    }
    expect(isMoving(Infinity, [{ ...mid, age: 0.1 }])).toBe(true);
    expect(isMoving(Infinity, [{ ...mid, age: PULSE_S }])).toBe(false);
    expect(changed(rest, frame(field, Infinity, [{ ...mid, age: PULSE_S }]))).toBe(0);
  });

  it("lights the ring above the resting ink", () => {
    const field = solidPlate();
    const rest = frame(field, Infinity);
    const ringed = frame(field, Infinity, [{ x: W / 2, y: H / 2, age: 0.1 }]);
    let brighter = 0;
    for (const i of field.active) {
      if (decodeCell(ringed[i]).alpha > decodeCell(rest[i]).alpha) brighter++;
      expect(decodeCell(rest[i]).light).toBe(0);
    }
    expect(brighter).toBeGreaterThan(0);
  });

  it("dims the type behind the headline", () => {
    const field = solidPlate();
    const before = frame(field, Infinity);
    setCalmZone(field, { left: W / 2 - 60, top: H / 2 - 20, right: W / 2 + 60, bottom: H / 2 + 20 });
    const after = frame(field, Infinity);
    const centre = Math.floor(H / 2 / CELL_H) * field.cols + Math.floor(W / 2 / CELL_W);
    expect(decodeCell(after[centre]).alpha).toBeLessThan(decodeCell(before[centre]).alpha);
    // Far outside the zone nothing changes.
    const corner = field.active[0];
    expect(after[corner]).toBe(before[corner]);
    setCalmZone(field, null);
    expect(frame(field, Infinity)).toEqual(before);
  });

  it("keeps a cell's settled character across rebuilds", () => {
    const a = solidPlate();
    const b = solidPlate();
    expect(Array.from(a.glyph)).toEqual(Array.from(b.glyph));
  });
});

describe("pickBackdropArt", () => {
  const arts = [...new Set(PLATES.map((p) => p.src))];
  it("never prints the plate the sidebar is showing", () => {
    for (const avoid of arts) {
      for (let k = 0; k < 50; k++) expect(pickBackdropArt(avoid, k / 50)).not.toBe(avoid);
    }
  });
  it("draws from every plate when nothing is showing", () => {
    const seen = new Set<string>();
    for (let k = 0; k < 100; k++) seen.add(pickBackdropArt(null, k / 100));
    expect(seen.size).toBe(arts.length);
  });
});
