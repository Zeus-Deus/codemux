// Milestone 4 readiness check: a tiny disk plugin that draws an animated
// canvas behind the new-thread landing in theme colours, pulses on
// empty-space clicks and redraws when the theme changes. Plain ESM, no JSX.
import * as React from "react";
import { host, LANDING_BACKDROP_AREA } from "@codemux/sdk";

const h = React.createElement;

// Gate instrumentation only: counts pulses and theme redraws so the native
// gate can read them from the page.
const stats = (window.__backdropTest = { pulses: [], themeRedraws: 0, frames: 0, colours: null });

function colours() {
  const roles = host.theme.get().roles;
  return { bg: roles.background, fg: roles.foreground, accent: roles.primary, muted: roles.mutedForeground };
}

function Backdrop({ onPulse, calmRect }) {
  const ref = React.useRef(null);
  React.useEffect(() => {
    const canvas = ref.current;
    const ctx = canvas && canvas.getContext("2d");
    if (!canvas || !ctx) return undefined;
    let palette = colours();
    stats.colours = palette;
    const pulses = [];
    let frame = 0;
    let t0 = performance.now();

    const fit = () => {
      const dpr = window.devicePixelRatio || 1;
      canvas.width = Math.round(canvas.clientWidth * dpr);
      canvas.height = Math.round(canvas.clientHeight * dpr);
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    };
    const ro = new ResizeObserver(fit);
    ro.observe(canvas);
    fit();

    const draw = (now) => {
      const w = canvas.clientWidth;
      const hgt = canvas.clientHeight;
      const t = (now - t0) / 1000;
      ctx.clearRect(0, 0, w, hgt);
      // A slow drifting dot field in the muted colour.
      ctx.fillStyle = palette.muted;
      const still = host.reducedMotion();
      for (let y = 24; y < hgt; y += 32) {
        for (let x = 24; x < w; x += 32) {
          const phase = still ? 0 : Math.sin(t * 0.8 + x * 0.02 + y * 0.03);
          ctx.globalAlpha = 0.12 + 0.1 * phase;
          ctx.fillRect(x, y, 2, 2);
        }
      }
      // Pulses: expanding rings in the accent colour.
      ctx.strokeStyle = palette.accent;
      ctx.lineWidth = 2;
      for (let i = pulses.length - 1; i >= 0; i -= 1) {
        const age = (now - pulses[i].t) / 1000;
        if (age > 1) {
          pulses.splice(i, 1);
          continue;
        }
        ctx.globalAlpha = 1 - age;
        ctx.beginPath();
        ctx.arc(pulses[i].x, pulses[i].y, 8 + age * 140, 0, Math.PI * 2);
        ctx.stroke();
      }
      // Dim behind the headline so text stays readable.
      const calm = calmRect();
      if (calm) {
        const r = canvas.getBoundingClientRect();
        ctx.globalAlpha = 0.85;
        ctx.fillStyle = palette.bg;
        ctx.fillRect(calm.left - r.left - 16, calm.top - r.top - 8, calm.width + 32, calm.height + 16);
      }
      ctx.globalAlpha = 1;
      stats.frames += 1;
      frame = requestAnimationFrame(draw);
    };
    frame = requestAnimationFrame(draw);

    const offPulse = onPulse((x, y) => {
      const r = canvas.getBoundingClientRect();
      pulses.push({ x: x - r.left, y: y - r.top, t: performance.now() });
      stats.pulses.push([Math.round(x), Math.round(y)]);
    });
    const offTheme = host.theme.subscribe(() => {
      palette = colours();
      stats.colours = palette;
      stats.themeRedraws += 1;
    });
    return () => {
      cancelAnimationFrame(frame);
      ro.disconnect();
      offPulse();
      offTheme();
    };
  }, [onPulse, calmRect]);
  return h("canvas", {
    ref,
    "data-backdrop-test": "",
    style: { position: "absolute", inset: 0, width: "100%", height: "100%" },
  });
}

export default {
  id: "backdrop-test",
  name: "Backdrop test",
  description: "Milestone 4 readiness check for landing.backdrop",
  version: "0.1.0",
  register(ctx) {
    ctx.register({ id: "canvas", area: LANDING_BACKDROP_AREA, render: Backdrop });
  },
};
