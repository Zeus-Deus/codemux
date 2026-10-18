/**
 * The model behind the empty thread's ASCII backdrop (Backdrop.tsx).
 *
 * One of the sidebar engravings is rebuilt as a grid of monospace characters.
 * Everything here is pure: a frame is a function of time and the live click
 * pulses, encoded as one number per cell, so the renderer can diff two frames
 * and repaint only the cells whose character or brightness changed. A still
 * frame repaints nothing, and an idle backdrop runs no frame loop at all.
 */

/** Cell size in CSS pixels. Small enough to read as texture, not as text. */
export const CELL_W = 7;
export const CELL_H = 12;
/** No 0/O, 1/l/I: the field is texture, so skip the glyphs that look alike. */
export const GLYPHS = "abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/** Opacity of fully inked, unlit type at rest. Kept faint: it sits behind
 *  the headline and must never compete with it. */
export const REST_ALPHA = 0.2;
/** Extra opacity at full light (a fresh character, the sheen, a click ring). */
export const LIT_ALPHA = 0.55;
export const MAX_ALPHA = REST_ALPHA + LIT_ALPHA;
/** Quantisation of a cell's light (atlas rows) and opacity (draw alpha). */
export const LIGHT_STEPS = 7;
export const ALPHA_STEPS = 15;

/** A character flickers this long after it prints, then settles. */
const SCRAMBLE_S = 0.3;
/** Flicker rate. Each cell changes at most this often, which also caps how
 *  many cells a frame repaints. */
const SCRAMBLE_HZ = 15;
const SHEEN_START_S = 1.1;
const SHEEN_S = 1.5;
/** When the opening animation is fully over. */
export const OPEN_S = SHEEN_START_S + SHEEN_S;
export const PULSE_S = 1.5;
const PULSE_SPEED = 950; // CSS px per second
const RING_HALF_WIDTH = 70;
/** Below this the plate is too faint to bother printing. */
const MIN_INK = 0.07;

export interface Pulse {
  x: number;
  y: number;
  /** Seconds since the click. */
  age: number;
}

export interface Field {
  /** CSS size the field was built for. */
  w: number;
  h: number;
  cols: number;
  rows: number;
  /** Indices of cells with any ink; every other cell is always blank. */
  active: Uint32Array;
  /** Ink per cell, 0..1, from the plate's alpha and its soft round edge. */
  ink: Float32Array;
  /** Dimming per cell, 0..1, behind the headline and composer. */
  calm: Float32Array;
  /** Seconds after opening when the cell prints. */
  delay: Float32Array;
  /** The cell's settled character. */
  glyph: Uint8Array;
}

/** Cheap integer hash: stable per cell, so a resize keeps the same type. */
function hash(a: number, b: number): number {
  let h = Math.imul(a ^ 0x9e3779b9, 0x85ebca6b) ^ Math.imul(b + 0x632be59b, 0xc2b2ae35);
  h ^= h >>> 15;
  h = Math.imul(h, 0x2c1b3c6d);
  h ^= h >>> 12;
  return h >>> 0;
}

/** Where the plate is printed: page-sized, centred, filling the height. */
export function plateBox(w: number, h: number) {
  const size = Math.min(w * 1.02, h * 1.08);
  return { x: (w - size) / 2, y: (h - size) / 2, size };
}

/**
 * Builds the field from the plate's alpha, sampled at one value per cell
 * (`alpha[r * cols + c]`, 0..255, already positioned with `plateBox`).
 */
export function buildField(alpha: ArrayLike<number>, w: number, h: number): Field {
  const cols = Math.ceil(w / CELL_W);
  const rows = Math.ceil(h / CELL_H);
  const n = cols * rows;
  const ink = new Float32Array(n);
  const delay = new Float32Array(n);
  const glyph = new Uint8Array(n);
  const calm = new Float32Array(n).fill(1);
  const active: number[] = [];
  const { size } = plateBox(w, h);
  for (let r = 0; r < rows; r++) {
    for (let c = 0; c < cols; c++) {
      const i = r * cols + c;
      // Soft round edge so the print never ends on a hard square.
      const dx = ((c + 0.5) * CELL_W - w / 2) / (size / 2);
      const dy = ((r + 0.5) * CELL_H - h / 2) / (size / 2);
      const edge = Math.max(0, Math.min(1, (1.08 - Math.hypot(dx, dy)) / 0.3));
      const v = ((alpha[i] ?? 0) / 255) * edge;
      if (v < MIN_INK) continue;
      ink[i] = v;
      active.push(i);
      const seed = hash(c, r);
      glyph[i] = seed % GLYPHS.length;
      // Prints top to bottom in ragged runs, like a terminal filling up.
      delay[i] = (r / rows) * 0.9 + ((seed >>> 8) % 1000) / 1000 * 0.35;
    }
  }
  return { w, h, cols, rows, active: Uint32Array.from(active), ink, calm, delay, glyph };
}

/**
 * Dims the cells behind `rect` (the headline and composer, in the field's CSS
 * coordinates) so the type never fights the words on top of it.
 */
export function setCalmZone(
  field: Field,
  rect: { left: number; top: number; right: number; bottom: number } | null,
): void {
  const { cols, calm, active } = field;
  const FEATHER = 56;
  const DEPTH = 0.55;
  for (let k = 0; k < active.length; k++) {
    const i = active[k];
    if (!rect) {
      calm[i] = 1;
      continue;
    }
    const x = ((i % cols) + 0.5) * CELL_W;
    const y = (Math.floor(i / cols) + 0.5) * CELL_H;
    const ox = Math.max(rect.left - x, 0, x - rect.right);
    const oy = Math.max(rect.top - y, 0, y - rect.bottom);
    const inside = 1 - Math.min(1, Math.hypot(ox, oy) / FEATHER);
    calm[i] = 1 - DEPTH * inside;
  }
}

/** One cell's look: 0 is blank; otherwise glyph | light << 6 | alpha << 9. */
export function encodeCell(glyph: number, light: number, alpha: number): number {
  return alpha === 0 ? 0 : glyph | (light << 6) | (alpha << 9);
}
export function decodeCell(code: number) {
  return { glyph: code & 63, light: (code >> 6) & 7, alpha: code >> 9 };
}

/**
 * Fills `out` with the frame at `t` seconds after opening. `t = Infinity`
 * (with no pulses) is the settled backdrop.
 */
export function computeFrame(field: Field, t: number, pulses: readonly Pulse[], out: Uint16Array): void {
  const { cols, active, ink, calm, delay, glyph, w, h } = field;
  const flicker = Math.floor(t * SCRAMBLE_HZ);
  const st = (t - SHEEN_START_S) / SHEEN_S;
  const sheenOn = st > 0 && st < 1;
  // The sheen is a diagonal band sweeping left to right.
  const span = w + h * 0.5;
  const sheenX = -span * 0.25 + st * span * 1.5;
  const sheenGain = sheenOn ? Math.sin(st * Math.PI) : 0;
  const rings = pulses
    .map((p) => ({ x: p.x, y: p.y, r: p.age * PULSE_SPEED, fade: 1 - p.age / PULSE_S }))
    .filter((p) => p.fade > 0);

  for (let k = 0; k < active.length; k++) {
    const i = active[k];
    const local = t - delay[i];
    if (local < 0) {
      out[i] = 0;
      continue;
    }
    const x = ((i % cols) + 0.5) * CELL_W;
    const y = (Math.floor(i / cols) + 0.5) * CELL_H;
    let light = 0;
    let scramble = false;
    if (local < SCRAMBLE_S) {
      light = 1 - local / SCRAMBLE_S;
      scramble = true;
    }
    if (sheenOn) {
      const d = Math.abs(x - (sheenX - y * 0.35));
      if (d < 160) light = Math.max(light, (1 - d / 160) * sheenGain);
    }
    for (const p of rings) {
      const d = Math.abs(Math.hypot(x - p.x, y - p.y) - p.r);
      if (d < RING_HALF_WIDTH) {
        const ring = (1 - d / RING_HALF_WIDTH) * p.fade;
        if (ring > 0.35) scramble = true;
        if (ring > light) light = ring;
      }
    }
    const g = scramble ? hash(i, flicker) % GLYPHS.length : glyph[i];
    const a = ink[i] * calm[i] * (REST_ALPHA + LIT_ALPHA * light);
    out[i] = encodeCell(
      g,
      Math.round(light * LIGHT_STEPS),
      Math.min(ALPHA_STEPS, Math.round((a / MAX_ALPHA) * ALPHA_STEPS)),
    );
  }
}

/** True while the frame at `t` with these pulses still differs from rest. */
export function isMoving(t: number, pulses: readonly Pulse[]): boolean {
  return t < OPEN_S || pulses.some((p) => p.age < PULSE_S);
}
