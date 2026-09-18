/**
 * Shared UI for the web-remote bootstrap screens (`bootstrap.tsx` and
 * `hosted-bootstrap.tsx`): splash dismissal, colour fallbacks, form styles and
 * the overlay React root.
 *
 * Keep this module a leaf. `bootstrap.tsx` imports `hosted-bootstrap.tsx`, so
 * importing either of them from here forms a cycle, and module-level styles
 * that read a binding from a partially evaluated module throw at load time.
 */
import React from "react";
import { useMobileViewport } from "@/hooks/use-mobile-layout";
import ReactDOM from "react-dom/client";

export function dismissSplash(): void {
  const splash = document.getElementById("splash");
  if (!splash) return;
  splash.classList.add("fade-out");
  splash.addEventListener("transitionend", () => splash.remove(), { once: true });
}

// ── Styles ──────────────────────────────────────────────────────────

/**
 * Colour fallbacks for the pre-app screens.
 *
 * These render before React mounts but *after* `globals.css` and the inline
 * boot script in `index.html`, so the design tokens below normally resolve
 * and the fallbacks are never used. They still must not assume a dark canvas:
 * the boot script may have painted a stored *light* palette, and a stylesheet
 * that failed to load would leave white-on-white. `--cm-boot-bg` /
 * `--cm-boot-fg` are written by that inline script from whichever palette is
 * stored (Graphite when nothing is), so chaining to them keeps every surface
 * on the correct side of the canvas in either scheme.
 */
const BOOT_BG = "var(--background, var(--cm-boot-bg))";
const BOOT_FG = "var(--foreground, var(--cm-boot-fg))";
const BOOT_SURFACE = "var(--card, var(--cm-boot-bg))";
const BOOT_MUTED_FG =
  "var(--muted-foreground, color-mix(in srgb, var(--cm-boot-fg) 62%, transparent))";
const BOOT_HAIRLINE =
  "var(--border, color-mix(in srgb, var(--cm-boot-fg) 14%, transparent))";
const BOOT_FIELD_BORDER =
  "var(--input, color-mix(in srgb, var(--cm-boot-fg) 20%, transparent))";

export const bootstrapColors = {
  background: BOOT_BG,
  foreground: BOOT_FG,
  surface: BOOT_SURFACE,
  mutedForeground: BOOT_MUTED_FG,
  hairline: BOOT_HAIRLINE,
  fieldBorder: BOOT_FIELD_BORDER,
} as const;

export const overlayStyle: React.CSSProperties = {
  position: "fixed",
  inset: 0,
  zIndex: 2147483645,
  display: "flex",
  // Keep the top reachable on short phone viewports while following the keyboard.
  alignItems: "safe center",
  height: "var(--mobile-height, 100dvh)",
  top: "var(--mobile-top, 0px)",
  justifyContent: "center",
  overflowY: "auto",
  padding: 24,
  background: BOOT_BG,
  color: BOOT_FG,
  fontFamily: "var(--font-sans, system-ui, -apple-system, sans-serif)",
};

export const cardStyle: React.CSSProperties = {
  width: "100%",
  maxWidth: 380,
  background: BOOT_SURFACE,
  border: `1px solid ${BOOT_HAIRLINE}`,
  borderRadius: 14,
  padding: 28,
  // Depth, not a smudge. The stored palette may be light, where a 45%-black
  // drop shadow reads as a grey haze around a white card on a white canvas;
  // this is soft enough to disappear there and still lift the card off a dark
  // one, which the hairline border alone does not.
  boxShadow: "0 10px 30px rgba(0,0,0,0.18)",
};

export const inputStyle: React.CSSProperties = {
  width: "100%",
  boxSizing: "border-box",
  padding: "10px 12px",
  fontSize: 13,
  color: BOOT_FG,
  background: BOOT_BG,
  border: `1px solid ${BOOT_FIELD_BORDER}`,
  borderRadius: 9,
  outline: "none",
};

export const primaryButtonStyle = (busy: boolean): React.CSSProperties => ({
  marginTop: 18,
  width: "100%",
  padding: "10px 14px",
  fontSize: 13.5,
  fontWeight: 600,
  cursor: busy ? "default" : "pointer",
  // Ink for the ember fill, not for the canvas — so it stays dark in both
  // schemes. Ember is a mid-tone in every palette (it is solved for contrast
  // on its own canvas, not against this label), and the same fixed dark ink
  // is what `--selection-foreground` uses over the same colour.
  color: "oklch(0.205 0.006 40)",
  background: "var(--accent-ember, oklch(0.705 0.152 47))",
  border: "none",
  borderRadius: 9,
  opacity: busy ? 0.7 : 1,
});

export const errorStyle: React.CSSProperties = {
  marginTop: 12,
  fontSize: 12.5,
  lineHeight: 1.45,
  color: "var(--status-attention, oklch(0.685 0.17 22))",
};

export const switchLinkStyle: React.CSSProperties = {
  marginTop: 16,
  width: "100%",
  fontSize: 12,
  fontWeight: 500,
  cursor: "pointer",
  color: BOOT_MUTED_FG,
  background: "transparent",
  border: "none",
  textAlign: "center",
  textDecoration: "underline",
  textUnderlineOffset: 2,
};

function BootstrapViewport({ children }: { children: React.ReactNode }) {
  useMobileViewport();
  return <>{children}</>;
}

/** Owns a single React root in a dedicated overlay element so the pairing
 *  UI never fights the app's `#root` React tree. */
export class BootstrapOverlay {
  private container: HTMLDivElement | null = null;
  private root: ReactDOM.Root | null = null;

  private ensure(): ReactDOM.Root {
    if (this.root) return this.root;
    dismissSplash();
    const el = document.createElement("div");
    el.id = "codemux-remote-bootstrap";
    document.body.appendChild(el);
    this.container = el;
    this.root = ReactDOM.createRoot(el);
    return this.root;
  }

  render(node: React.ReactElement): void {
    this.ensure().render(<React.StrictMode><BootstrapViewport>{node}</BootstrapViewport></React.StrictMode>);
  }

  remove(): void {
    this.root?.unmount();
    this.container?.remove();
    this.root = null;
    this.container = null;
  }
}
