/**
 * Remote-client connection indicator.
 *
 * A persistent, subtle pill anchored to the bottom-left of the viewport,
 * shown ONLY when the app is running as the web-remote client. It reads
 * "Remote — <host>" while connected, degrades to an amber "Reconnecting…"
 * during backoff, and turns red on a revoked/offline session before the
 * bootstrap falls back to the pairing screen.
 *
 * Deliberately dependency-free (plain DOM, no React, no app stores) so it
 * renders regardless of the app's mount state — the bootstrap drives it
 * both before the app mounts and after it goes live. It styles itself from
 * the app's shared CSS custom properties so it tracks the active theme.
 *
 * (This replaces the Stage-1 top-of-viewport reconnecting banner.)
 */

export type RemoteIndicatorState = "live" | "reconnecting" | "offline";

export interface RemoteIndicator {
  /** Persistent connected state: "Remote — <host>". */
  setLive(host: string): void;
  /** Backoff/reconnect state: amber, pulsing. */
  setReconnecting(host: string): void;
  /** Session revoked / gone: red, pulsing. */
  setOffline(message?: string): void;
  /** Remove the indicator entirely. */
  hide(): void;
}

const CONTAINER_STYLE = [
  "position:fixed",
  "left:14px",
  "bottom:14px",
  "z-index:2147483000",
  "display:none",
  "align-items:center",
  "gap:8px",
  "box-sizing:border-box",
  "padding:5px 11px 5px 9px",
  "border-radius:999px",
  "border:1px solid var(--border, rgba(255,255,255,0.12))",
  "box-shadow:0 2px 12px rgba(0,0,0,0.28)",
  "font:500 11.5px/1 var(--font-sans, system-ui, -apple-system, sans-serif)",
  "color:var(--muted-foreground, #9a9a97)",
  "letter-spacing:0.01em",
  "white-space:nowrap",
  "user-select:none",
  "pointer-events:none", // purely informational — never eats clicks
  "backdrop-filter:blur(8px)",
  "-webkit-backdrop-filter:blur(8px)",
].join(";");

const DOT_STYLE = [
  "width:7px",
  "height:7px",
  "border-radius:50%",
  "flex:none",
].join(";");

const PULSE = "codemux-remote-pulse 1.4s ease-in-out infinite";

const STATE_COLOR: Record<RemoteIndicatorState, string> = {
  // sky — "on another device / remote", matches --status-remote in the app.
  live: "var(--status-remote, #5aa2e6)",
  // amber — matches --status-working.
  reconnecting: "var(--status-working, #d4a84b)",
  // red — matches --status-attention.
  offline: "var(--status-attention, #cc4444)",
};

function ensureKeyframes(): void {
  if (document.getElementById("codemux-remote-indicator-style")) return;
  const style = document.createElement("style");
  style.id = "codemux-remote-indicator-style";
  style.textContent =
    "@keyframes codemux-remote-pulse{0%,100%{opacity:1}50%{opacity:0.35}}";
  document.head.appendChild(style);
}

export function createRemoteIndicator(): RemoteIndicator {
  let el: HTMLDivElement | null = null;
  let dot: HTMLSpanElement | null = null;
  let label: HTMLSpanElement | null = null;

  function ensure(): void {
    if (el) return;
    ensureKeyframes();
    const node = document.createElement("div");
    node.id = "codemux-remote-indicator";
    node.setAttribute("role", "status");
    node.setAttribute("aria-live", "polite");
    node.style.cssText = CONTAINER_STYLE;
    // Layered background: solid fallback first, then a theme-aware
    // translucent card (ignored by engines that can't parse color-mix).
    node.style.background = "rgba(22,22,24,0.86)";
    node.style.background = "color-mix(in oklab, var(--card, #1a1a1c) 88%, transparent)";

    const d = document.createElement("span");
    d.style.cssText = DOT_STYLE;

    const t = document.createElement("span");

    node.appendChild(d);
    node.appendChild(t);
    document.body.appendChild(node);
    el = node;
    dot = d;
    label = t;
  }

  function paint(
    state: RemoteIndicatorState,
    text: string,
    strong: boolean,
  ): void {
    ensure();
    if (!el || !dot || !label) return;
    const color = STATE_COLOR[state];
    dot.style.background = color;
    dot.style.animation = state === "live" ? "none" : PULSE;
    label.textContent = text;
    // Reconnecting / offline read as the active concern, so their label
    // takes the state colour; the steady live state stays muted + subtle.
    label.style.color = strong ? color : "var(--muted-foreground, #9a9a97)";
    el.style.display = "flex";
  }

  return {
    setLive(host: string): void {
      paint("live", host ? `Remote — ${host}` : "Remote", false);
    },
    setReconnecting(host: string): void {
      paint(
        "reconnecting",
        host ? `Reconnecting to ${host}…` : "Reconnecting…",
        true,
      );
    },
    setOffline(message?: string): void {
      paint("offline", message ?? "Remote — offline", true);
    },
    hide(): void {
      if (el) el.style.display = "none";
    },
  };
}
