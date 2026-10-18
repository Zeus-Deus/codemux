import { useLayoutEffect, useRef, useState, type RefObject } from "react";

/** Axis scale and sizing shared by the Usage page's charts. */

/** Gridline count the axis top is divided into. */
const GRIDLINES = 4;
/** Minimum horizontal room per axis label before ticks are thinned. */
const TICK_MIN_SPACING_PX = 72;
/** Fallback before the first measurement — wide enough that a server
 *  or jsdom render still lays the points out sensibly. */
const FALLBACK_WIDTH_PX = 640;

/** Smooth `points` with a monotone cubic curve (Fritsch–Carlson
 *  tangents). Unlike a Catmull-Rom spline it never overshoots, so a
 *  run of zeros stays flat on the baseline instead of dipping below
 *  it — a chart of spend must not draw negative money. */
export function monotonePath(points: { x: number; y: number }[]): string {
  const n = points.length;
  if (n === 0) return "";
  if (n === 1) return `M${points[0].x},${points[0].y}`;
  const dx: number[] = [];
  const slope: number[] = [];
  for (let i = 0; i < n - 1; i++) {
    const d = points[i + 1].x - points[i].x;
    dx.push(d);
    slope.push(d === 0 ? 0 : (points[i + 1].y - points[i].y) / d);
  }
  const tangent: number[] = [slope[0]];
  for (let i = 1; i < n - 1; i++) {
    const a = slope[i - 1];
    const b = slope[i];
    tangent.push(a * b <= 0 ? 0 : (a + b) / 2);
  }
  tangent.push(slope[n - 2]);
  // Clamp tangents so the curve stays monotone between samples.
  for (let i = 0; i < n - 1; i++) {
    if (slope[i] === 0) {
      tangent[i] = 0;
      tangent[i + 1] = 0;
      continue;
    }
    const a = tangent[i] / slope[i];
    const b = tangent[i + 1] / slope[i];
    const s = a * a + b * b;
    if (s > 9) {
      const t = 3 / Math.sqrt(s);
      tangent[i] = t * a * slope[i];
      tangent[i + 1] = t * b * slope[i];
    }
  }
  let d = `M${fmt(points[0].x)},${fmt(points[0].y)}`;
  for (let i = 0; i < n - 1; i++) {
    const p0 = points[i];
    const p1 = points[i + 1];
    const h = dx[i] / 3;
    d += ` C${fmt(p0.x + h)},${fmt(p0.y + tangent[i] * h)} ${fmt(p1.x - h)},${fmt(
      p1.y - tangent[i + 1] * h,
    )} ${fmt(p1.x)},${fmt(p1.y)}`;
  }
  return d;
}

function fmt(n: number): string {
  return Number.isInteger(n) ? String(n) : n.toFixed(2);
}

/** The smallest round axis top (1, 2, 2.5 or 5 × 10ⁿ per step) that
 *  holds `max` in `steps` equal steps, so gridlines land on readable
 *  values instead of fractions of the data's peak. */
export function niceAxisMax(max: number, steps: number = GRIDLINES): number {
  if (!(max > 0) || !Number.isFinite(max)) return 1;
  const rough = max / steps;
  const magnitude = 10 ** Math.floor(Math.log10(rough));
  const step =
    [1, 2, 2.5, 5, 10].map((m) => m * magnitude).find((candidate) => candidate >= rough) ??
    10 * magnitude;
  return step * steps;
}

/** Evenly spaced tick indices that always include the first and last
 *  bucket, thinned so labels never collide at 24 hours or 90 days. */
export function tickIndices(count: number, width: number): number[] {
  if (count <= 0) return [];
  if (count === 1) return [0];
  const maxTicks = Math.max(2, Math.floor(width / TICK_MIN_SPACING_PX));
  if (count <= maxTicks) return Array.from({ length: count }, (_, i) => i);
  const step = Math.ceil((count - 1) / (maxTicks - 1));
  const ticks: number[] = [];
  for (let i = 0; i < count - 1; i += step) {
    // Drop a tick that would crowd the final one.
    if (count - 1 - i < step / 2) break;
    ticks.push(i);
  }
  ticks.push(count - 1);
  return ticks;
}

export function useMeasuredWidth<T extends HTMLElement>(): [
  RefObject<T | null>,
  number,
] {
  const ref = useRef<T | null>(null);
  const [width, setWidth] = useState(FALLBACK_WIDTH_PX);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => {
      const w = el.getBoundingClientRect().width;
      if (w > 0) setWidth(w);
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return [ref, width];
}
