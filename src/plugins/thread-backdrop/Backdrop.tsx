import { useEffect, useRef, useState } from "react";
import { PLATES, readLastPlate, type Host, type LandingBackdropProps } from "@codemux/sdk";
import {
  ALPHA_STEPS,
  CELL_H,
  CELL_W,
  GLYPHS,
  LIGHT_STEPS,
  MAX_ALPHA,
  PULSE_S,
  buildField,
  computeFrame,
  decodeCell,
  isMoving,
  plateBox,
  setCalmZone,
  type Field,
  type Pulse,
} from "./field";

/**
 * The empty thread's backdrop: one of the sidebar's engravings, rebuilt from
 * monospace type across the whole thread.
 *
 * Opening the thread types the plate in from the top and runs one pass of
 * light across it. A press on empty space sends a ring of light out from the
 * pointer. Otherwise it is a still image with no frame loop.
 *
 * Cost stays small: every glyph is pre-drawn once per brightness step into a
 * small atlas, so a frame is image copies rather than text layout; each frame
 * repaints only the cells whose look changed; and the animation runs at 30
 * fps, plenty for flickering type. Once it settles, the canvas is swapped for
 * a plain image of the same pixels and its backing store is released. A
 * canvas is its own compositing layer, re-blended on every frame anything
 * else on screen animates; an image paints once into the page and is free
 * from then on. Colours come from the theme (muted ink, foreground light) and
 * are re-read whenever the theme changes, so any palette works.
 */

/** Distinct artworks, in plate order. */
const ARTWORKS = [...new Set(PLATES.map((p) => p.src))];

/** A random artwork other than `avoid` (the one the sidebar shows), so the two
 *  surfaces don't print the same engraving side by side. `rand` is in [0, 1). */
export function pickBackdropArt(avoid: string | null, rand: number): string {
  const pool = ARTWORKS.filter((src) => src !== avoid);
  const from = pool.length > 0 ? pool : ARTWORKS;
  return from[Math.min(from.length - 1, Math.floor(rand * from.length))];
}

const REDUCE_QUERY = "(prefers-reduced-motion: reduce)";

/** The app's reduce-motion setting or the OS preference. */
export function reducedMotion(host: Host): boolean {
  const os = window.matchMedia?.(REDUCE_QUERY).matches ?? false;
  return host.reducedMotion() || os;
}

/** Calls `fn` with the combined preference whenever either source changes,
 *  so a mounted backdrop stops animating the moment motion is reduced. */
export function watchReducedMotion(host: Host, fn: (reduced: boolean) => void): () => void {
  let last = reducedMotion(host);
  const check = () => {
    const next = reducedMotion(host);
    if (next !== last) {
      last = next;
      fn(next);
    }
  };
  const offSetting = host.settings.subscribe("appearance.reduce_motion", check);
  const query = window.matchMedia?.(REDUCE_QUERY);
  query?.addEventListener?.("change", check);
  return () => {
    offSetting();
    query?.removeEventListener?.("change", check);
  };
}

const FRAME_MS = 1000 / 30;
/** Backing store cap, in device pixels (~12 MB). A full-thread canvas on a
 *  large HiDPI display would otherwise hold tens of megabytes for a faint
 *  texture; past this it renders a little soft, which type this faint hides. */
const MAX_BACKING_PIXELS = 3_000_000;
const FONT = '500 10.5px "JetBrains Mono Variable", "JetBrains Mono", ui-monospace, monospace';

type Rgb = [number, number, number];

/** Resolves any CSS colour (oklch, color-mix, …) to RGB. Uses its own 1×1
 *  canvas: the backdrop's canvas is 0×0 while parked, and reading through it
 *  would return black. */
function toRgb(colour: string): Rgb {
  const probe = document.createElement("canvas");
  probe.width = 1;
  probe.height = 1;
  const g = probe.getContext("2d", { willReadFrequently: true });
  if (!g) return [128, 128, 128];
  g.fillStyle = "#000";
  g.fillStyle = colour;
  g.fillRect(0, 0, 1, 1);
  const [r, gr, b] = g.getImageData(0, 0, 1, 1).data;
  return [r, gr, b];
}

interface Atlas {
  canvas: HTMLCanvasElement;
  tileW: number;
  tileH: number;
}

/** Every glyph, at every light step, drawn once. */
function buildAtlas(scale: number, ink: Rgb, hi: Rgb): Atlas | null {
  const tileW = Math.ceil(CELL_W * scale);
  const tileH = Math.ceil(CELL_H * scale);
  const canvas = document.createElement("canvas");
  canvas.width = tileW * GLYPHS.length;
  canvas.height = tileH * (LIGHT_STEPS + 1);
  // CPU-backed (see the main canvas below).
  const g = canvas.getContext("2d", { willReadFrequently: true });
  if (!g) return null;
  g.scale(scale, scale);
  g.font = FONT;
  g.textAlign = "center";
  g.textBaseline = "middle";
  for (let s = 0; s <= LIGHT_STEPS; s++) {
    const k = s / LIGHT_STEPS;
    const mix = ink.map((v, j) => Math.round(v + (hi[j] - v) * k));
    g.fillStyle = `rgb(${mix[0]}, ${mix[1]}, ${mix[2]})`;
    for (let i = 0; i < GLYPHS.length; i++) {
      g.fillText(GLYPHS[i], (i * tileW) / scale + CELL_W / 2, (s * tileH) / scale + CELL_H / 2);
    }
  }
  return { canvas, tileW, tileH };
}

export function Backdrop({ onPulse, calmRect, onCalmChange, host }: LandingBackdropProps & { host: Host }) {
  const [src] = useState(() => pickBackdropArt(PLATES[readLastPlate()]?.src ?? null, Math.random()));
  const rootRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const stillRef = useRef<HTMLImageElement>(null);
  const inkRef = useRef<HTMLSpanElement>(null);
  const hiRef = useRef<HTMLSpanElement>(null);

  useEffect(() => {
    const root = rootRef.current!;
    const canvas = canvasRef.current!;
    const still = stillRef.current!;
    // CPU-backed on purpose. The work per frame is a few hundred small image
    // copies, which a CPU canvas does cheaply, while a GPU canvas makes
    // WebKit spin up an accelerated drawing context and its resource cache
    // for one faint texture.
    const ctx = canvas.getContext("2d", { willReadFrequently: true });
    if (!ctx) return;
    let reduced = reducedMotion(host);
    const start = performance.now();
    let field: Field | null = null;
    let atlas: Atlas | null = null;
    let shown = new Uint16Array(0);
    let next = new Uint16Array(0);
    let pulses: { x: number; y: number; at: number }[] = [];
    let scale = 1;
    let raf = 0;
    let lastFrame = -Infinity;
    let disposed = false;
    let img: HTMLImageElement | null = null;
    // Backing size in device pixels, and the settled image while parked.
    let backingW = 0;
    let backingH = 0;
    let parked = false;
    let settleToken = 0;
    let stillUrl: string | null = null;
    // Device-pixel edges of each column and row, so neighbouring cells meet
    // exactly and clearing one never smears another.
    let xs = new Int32Array(0);
    let ys = new Int32Array(0);

    const elapsed = (now: number) => (reduced ? Infinity : (now - start) / 1000);
    const livePulses = (now: number): Pulse[] =>
      pulses.map((p) => ({ x: p.x, y: p.y, age: (now - p.at) / 1000 }));

    const paint = (now: number, full: boolean) => {
      if (!field || !atlas) return;
      computeFrame(field, elapsed(now), livePulses(now), next);
      const { cols, active } = field;
      const { canvas: sheet, tileW, tileH } = atlas;
      if (full) ctx.clearRect(0, 0, canvas.width, canvas.height);
      for (let k = 0; k < active.length; k++) {
        const i = active[k];
        const code = next[i];
        if (!full && code === shown[i]) continue;
        const c = i % cols;
        const r = (i - c) / cols;
        const x = xs[c];
        const y = ys[r];
        const w = Math.min(xs[c + 1] - x, tileW);
        const h = Math.min(ys[r + 1] - y, tileH);
        if (!full) ctx.clearRect(x, y, xs[c + 1] - x, ys[r + 1] - y);
        if (code !== 0) {
          const { glyph, light, alpha } = decodeCell(code);
          ctx.globalAlpha = (alpha / ALPHA_STEPS) * MAX_ALPHA;
          ctx.drawImage(sheet, glyph * tileW, light * tileH, w, h, x, y, w, h);
        }
      }
      ctx.globalAlpha = 1;
      const swap = shown;
      shown = next;
      next = swap;
    };

    /** Puts the live canvas back on screen with every cell painted. */
    const present = (now: number) => {
      settleToken++;
      if (canvas.width !== backingW || canvas.height !== backingH) {
        canvas.width = backingW;
        canvas.height = backingH;
      }
      paint(now, true);
      if (parked) {
        // Same frame: the canvas shows before the image hides, so no flash.
        canvas.hidden = false;
        still.hidden = true;
        parked = false;
      }
    };

    /** Swaps the settled canvas for an image of it and frees the canvas. */
    const settle = () => {
      const token = ++settleToken;
      canvas.toBlob((blob) => {
        if (!blob || disposed || token !== settleToken) return;
        const url = URL.createObjectURL(blob);
        still.src = url;
        still
          .decode()
          .catch(() => undefined)
          .then(() => {
            if (disposed || token !== settleToken) {
              URL.revokeObjectURL(url);
              return;
            }
            still.hidden = false;
            canvas.hidden = true;
            canvas.width = 0;
            canvas.height = 0;
            if (stillUrl) URL.revokeObjectURL(stillUrl);
            stillUrl = url;
            parked = true;
            // The glyph sheet is rebuilt on the next wake; ~1 ms.
            atlas = null;
          });
      });
    };

    let colours: { ink: Rgb; hi: Rgb } | null = null;
    const ensureAtlas = () => {
      if (!atlas && colours) atlas = buildAtlas(scale, colours.ink, colours.hi);
      return atlas;
    };

    const tick = (now: number) => {
      raf = 0;
      if (disposed || !field || !ensureAtlas()) return;
      pulses = pulses.filter((p) => now - p.at < PULSE_S * 1000);
      const moving = isMoving(elapsed(now), livePulses(now));
      if (parked) {
        lastFrame = now;
        present(now);
      } else if (!moving || now - lastFrame >= FRAME_MS - 1) {
        // While moving, hold to 30 fps. Once still, this paints the settled
        // frame and the loop ends: an idle backdrop schedules nothing.
        lastFrame = now;
        paint(now, false);
      }
      if (moving) raf = requestAnimationFrame(tick);
      else settle();
    };
    const kick = () => {
      if (!raf && !disposed) {
        settleToken++;
        raf = requestAnimationFrame(tick);
      }
    };

    /** The headline block in this backdrop's coordinates, dimmed behind. */
    const localCalm = () => {
      const a = calmRect();
      if (!a) return null;
      const b = root.getBoundingClientRect();
      return { left: a.left - b.left, top: a.top - b.top, right: a.right - b.left, bottom: a.bottom - b.top };
    };

    const recolour = () => {
      if (!field || disposed) return;
      colours = {
        ink: toRgb(getComputedStyle(inkRef.current!).color),
        hi: toRgb(getComputedStyle(hiRef.current!).color),
      };
      atlas = buildAtlas(scale, colours.ink, colours.hi);
      present(performance.now());
      kick();
    };

    const layout = () => {
      if (disposed || !img) return;
      const rect = root.getBoundingClientRect();
      if (rect.width < 2 || rect.height < 2) return;
      const dpr = window.devicePixelRatio || 1;
      scale = Math.min(dpr, Math.sqrt(MAX_BACKING_PIXELS / (rect.width * rect.height)));
      backingW = Math.round(rect.width * scale);
      backingH = Math.round(rect.height * scale);
      // Sample the plate's alpha at one value per cell.
      const cols = Math.ceil(rect.width / CELL_W);
      const rows = Math.ceil(rect.height / CELL_H);
      const probe = document.createElement("canvas");
      probe.width = cols;
      probe.height = rows;
      const p = probe.getContext("2d", { willReadFrequently: true });
      if (!p) return;
      const box = plateBox(rect.width, rect.height);
      p.drawImage(img, box.x / CELL_W, box.y / CELL_H, box.size / CELL_W, box.size / CELL_H);
      const data = p.getImageData(0, 0, cols, rows).data;
      const alpha = new Uint8Array(cols * rows);
      for (let i = 0; i < alpha.length; i++) alpha[i] = data[i * 4 + 3];
      field = buildField(alpha, rect.width, rect.height);
      setCalmZone(field, localCalm());
      shown = new Uint16Array(field.cols * field.rows);
      next = new Uint16Array(field.cols * field.rows);
      xs = Int32Array.from({ length: field.cols + 1 }, (_, c) => Math.round(c * CELL_W * scale));
      ys = Int32Array.from({ length: field.rows + 1 }, (_, r) => Math.round(r * CELL_H * scale));
      recolour();
    };

    // Re-dim when the headline moves: a notice appears, or the pane scrolls.
    let lastCalm = "";
    const recalm = () => {
      if (!field) return;
      const calm = localCalm();
      const key = calm ? `${calm.left},${calm.top},${calm.right},${calm.bottom}` : "";
      if (key === lastCalm) return;
      lastCalm = key;
      setCalmZone(field, calm);
      kick();
    };

    const offPulse = onPulse((clientX, clientY) => {
      if (reduced) return;
      const rect = root.getBoundingClientRect();
      pulses.push({ x: clientX - rect.left, y: clientY - rect.top, at: performance.now() });
      kick();
    });
    const offCalm = onCalmChange(recalm);
    const offMotion = watchReducedMotion(host, (next) => {
      reduced = next;
      if (!reduced) return;
      // Jump straight to the settled frame: with `reduced` set, elapsed time
      // reads as finished, so the next tick paints it still and parks.
      pulses = [];
      if (raf) {
        cancelAnimationFrame(raf);
        raf = 0;
      }
      if (!parked) kick();
    });
    const ro = new ResizeObserver(layout);
    let themeRaf = 0;
    const offTheme = host.theme.subscribe(() => {
      // Variables are written before listeners run; read on the next frame
      // so the probes' computed colours are current.
      cancelAnimationFrame(themeRaf);
      themeRaf = requestAnimationFrame(recolour);
    });

    const image = new Image();
    image.src = src;
    image
      .decode()
      .catch(() => undefined)
      .then(() => {
        if (disposed || !image.naturalWidth) return;
        img = image;
        ro.observe(root);
      });

    return () => {
      disposed = true;
      offPulse();
      offCalm();
      offMotion();
      offTheme();
      ro.disconnect();
      cancelAnimationFrame(themeRaf);
      if (raf) cancelAnimationFrame(raf);
      // Release the backing store and image now, not whenever GC gets to it.
      canvas.width = 0;
      canvas.height = 0;
      still.removeAttribute("src");
      if (stillUrl) URL.revokeObjectURL(stillUrl);
      atlas = null;
      field = null;
    };
  }, [src, onPulse, calmRect, onCalmChange, host]);

  return (
    <div ref={rootRef} data-thread-backdrop style={{ position: "absolute", inset: 0 }}>
      <canvas ref={canvasRef} style={{ position: "absolute", inset: 0, width: "100%", height: "100%" }} />
      <img
        ref={stillRef}
        hidden
        alt=""
        style={{ position: "absolute", inset: 0, width: "100%", height: "100%" }}
      />
      {/* Colour probes: the canvas reads the theme's colours from these. */}
      <span ref={inkRef} hidden style={{ color: `var(${host.theme.cssVar("mutedForeground")})` }} />
      <span ref={hiRef} hidden style={{ color: `var(${host.theme.cssVar("foreground")})` }} />
    </div>
  );
}
