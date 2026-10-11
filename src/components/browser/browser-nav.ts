/**
 * The browser's navigation verbs, in one place.
 *
 * Two surfaces drive the same Chromium — the main-area pane's
 * {@link BrowserToolbar} and the right-panel deck's browser pane bar — and
 * both go through the agent-browser command channel rather than the
 * `browser_history_back`/`browser_reload` Tauri commands, so a click and an
 * agent's `codemux browser back` take the identical path.
 *
 * `sessionId` is always the session's `cli_session_name` where one exists
 * (see `effectiveSessionId` in `BrowserPane`), never the raw `browser-N` id.
 */
import { agentBrowserRun } from "@/tauri/commands";

export type BrowserNavAction = "back" | "forward" | "reload";

export function runBrowserNav(
  sessionId: string,
  action: BrowserNavAction,
): Promise<unknown> {
  return agentBrowserRun(sessionId, action, {});
}

interface ShortcutKeys {
  key: string;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}

/**
 * The browser-chrome shortcut a keydown on the page canvas means, if any:
 * Alt+←/→ for history, Ctrl+R / F5 for reload, Ctrl+L for the address bar.
 * Everything else belongs to the page.
 */
export function browserChromeShortcut(
  e: ShortcutKeys,
): BrowserNavAction | "address" | null {
  const mod = e.ctrlKey || e.metaKey;
  const altOnly = e.altKey && !mod && !e.shiftKey;
  if (altOnly && e.key === "ArrowLeft") return "back";
  if (altOnly && e.key === "ArrowRight") return "forward";
  const modOnly = mod && !e.altKey && !e.shiftKey;
  const key = e.key.toLowerCase();
  if (modOnly && key === "r") return "reload";
  if (e.key === "F5" && !mod && !e.altKey) return "reload";
  if (modOnly && key === "l") return "address";
  return null;
}

// Hosts that only ever serve plain http in practice: loopback, mDNS and
// the reserved dev TLDs. A dev server on one of these never has a cert,
// so guessing https turns `localhost:5173` into a TLS error.
const LOCAL_HOST = /^(?:localhost|0\.0\.0\.0|\[::1\]|.+\.(?:local|localhost|test))$/i;
const IPV4 = /^\d{1,3}(?:\.\d{1,3}){3}$/;

export const BROWSER_SEARCH_URL = "https://duckduckgo.com/?q=";

/**
 * What the user typed, as something the browser can actually open.
 *
 * Explicit schemes pass through. Local hosts, IP literals and any bare
 * `host:port` get `http://`; other dotted hosts get `https://`. Anything
 * that cannot be a host (spaces, or a single word with no dot or port)
 * is a search.
 */
export function normalizeBrowserUrl(input: string): string {
  const url = input.trim();
  if (url.includes("://") || /^(?:data|about):/i.test(url)) return url;
  const search = `${BROWSER_SEARCH_URL}${encodeURIComponent(url)}`;
  if (/\s/.test(url)) return search;

  const authority = url.split(/[/?#]/, 1)[0];
  const match = authority.match(/^(\[[^\]]+\]|[^:]+)(?::(\d+))?$/);
  if (!match) return `https://${url}`;
  const [, host, port] = match;
  if (LOCAL_HOST.test(host) || IPV4.test(host) || port) return `http://${url}`;
  if (!host.includes(".")) return search;
  return `https://${url}`;
}

/**
 * The URL as a breadcrumb: scheme and `www.` dropped, trailing slash gone.
 *
 * The deck's pane bar gives the crumb a single truncating line, so the
 * host has to survive; `https://` in front of it is 8 characters of noise
 * that would push the host out first on a narrow panel.
 */
export function browserUrlCrumb(url: string | null | undefined): string {
  if (!url || url === "about:blank") return "about:blank";
  const stripped = url.replace(/^[a-z]+:\/\//i, "").replace(/^www\./i, "");
  const trimmed = stripped.replace(/\/$/, "");
  return trimmed || url;
}

/** Viewport sizes the toolbar offers. `fit` follows the pane's own size. */
export const BROWSER_VIEWPORT_PRESETS = [
  { id: "fit", label: "Fit pane", size: null },
  { id: "desktop", label: "Desktop", size: { width: 1280, height: 800 } },
  { id: "tablet", label: "Tablet", size: { width: 768, height: 1024 } },
  { id: "mobile", label: "Mobile", size: { width: 390, height: 844 } },
] as const;

export type BrowserViewportPresetId = (typeof BROWSER_VIEWPORT_PRESETS)[number]["id"];

export function browserViewportPresetSize(
  id: BrowserViewportPresetId,
): { width: number; height: number } | null {
  return BROWSER_VIEWPORT_PRESETS.find((preset) => preset.id === id)?.size ?? null;
}
