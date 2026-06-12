// Helpers for the browser pane's stream-input protocol (mouse/keyboard
// forwarding over the stream WebSocket) and the stream daemon's HTTP
// sidecar endpoints (`/api/status`, `/api/command`).
//
// Pure functions live at the top so they can be unit-tested without a
// DOM or network; the fetch-based daemon helpers are at the bottom.

// ---------------------------------------------------------------------------
// Mouse buttons
// ---------------------------------------------------------------------------

export type CdpButton = "left" | "middle" | "right" | "none";

/** Map a MouseEvent/PointerEvent `button` index to a CDP button name. */
export function cdpButtonFromEvent(button: number): CdpButton {
  switch (button) {
    case 0:
      return "left";
    case 1:
      return "middle";
    case 2:
      return "right";
    default:
      return "none";
  }
}

/**
 * Map a MouseEvent/PointerEvent `buttons` bitmask to the CDP button that
 * should ride on `mouseMoved` events. The stream daemon forwards `button`
 * verbatim to `Input.dispatchMouseEvent`, and Chromium derives its
 * "button held during move" modifier from that field — so drag gestures
 * (text selection, drag-and-drop) only work when moves carry the held
 * button instead of "none".
 */
export function heldButtonFromButtons(buttons: number): CdpButton {
  if (buttons & 1) return "left";
  if (buttons & 2) return "right";
  if (buttons & 4) return "middle";
  return "none";
}

export function getModifiers(e: {
  shiftKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  metaKey: boolean;
}): number {
  let m = 0;
  if (e.shiftKey) m |= 1;
  if (e.ctrlKey) m |= 2;
  if (e.altKey) m |= 4;
  if (e.metaKey) m |= 8;
  return m;
}

// ---------------------------------------------------------------------------
// Click-count chaining (double / triple click)
// ---------------------------------------------------------------------------

export interface ClickState {
  count: number;
  time: number;
  x: number;
  y: number;
  button: CdpButton;
}

const MULTI_CLICK_MS = 500;
const MULTI_CLICK_SLOP_PX = 5;

/**
 * Compute the CDP `clickCount` for a new press. Rapid same-button presses
 * near the same point chain 1 → 2 → 3 (word / paragraph selection), then
 * wrap back to 1.
 */
export function nextClickState(
  prev: ClickState | null,
  x: number,
  y: number,
  button: CdpButton,
  time: number,
): ClickState {
  if (
    prev &&
    prev.button === button &&
    time - prev.time <= MULTI_CLICK_MS &&
    Math.abs(x - prev.x) <= MULTI_CLICK_SLOP_PX &&
    Math.abs(y - prev.y) <= MULTI_CLICK_SLOP_PX
  ) {
    return { count: prev.count >= 3 ? 1 : prev.count + 1, time, x, y, button };
  }
  return { count: 1, time, x, y, button };
}

// ---------------------------------------------------------------------------
// Coordinate mapping
// ---------------------------------------------------------------------------

export interface ViewportInfo {
  width: number;
  height: number;
}

export interface DrawInfo {
  x: number;
  y: number;
  w: number;
  h: number;
}

/**
 * Map a client-space pointer position to remote-viewport coordinates,
 * accounting for canvas DPI scaling and aspect-ratio letterboxing.
 * Coordinates are clamped to the viewport so captured drags that leave
 * the pane keep producing valid edge positions (which is also what
 * drives Chromium's selection autoscroll).
 */
export function mapToViewport(
  point: { clientX: number; clientY: number },
  rect: { left: number; top: number; width: number; height: number },
  canvasSize: { width: number; height: number },
  viewport: ViewportInfo,
  drawInfo: DrawInfo,
): { x: number; y: number } {
  const scaleX = rect.width > 0 ? canvasSize.width / rect.width : 1;
  const scaleY = rect.height > 0 ? canvasSize.height / rect.height : 1;
  const canvasX = (point.clientX - rect.left) * scaleX;
  const canvasY = (point.clientY - rect.top) * scaleY;
  const x = drawInfo.w > 0 ? ((canvasX - drawInfo.x) / drawInfo.w) * viewport.width : 0;
  const y = drawInfo.h > 0 ? ((canvasY - drawInfo.y) / drawInfo.h) * viewport.height : 0;
  return {
    x: Math.min(Math.max(0, Math.round(x)), Math.max(0, viewport.width - 1)),
    y: Math.min(Math.max(0, Math.round(y)), Math.max(0, viewport.height - 1)),
  };
}

// ---------------------------------------------------------------------------
// Cursor sanitization
// ---------------------------------------------------------------------------

const CURSOR_KEYWORDS = new Set([
  "default",
  "pointer",
  "text",
  "crosshair",
  "move",
  "grab",
  "grabbing",
  "help",
  "wait",
  "progress",
  "not-allowed",
  "no-drop",
  "context-menu",
  "cell",
  "vertical-text",
  "alias",
  "copy",
  "zoom-in",
  "zoom-out",
  "col-resize",
  "row-resize",
  "n-resize",
  "e-resize",
  "s-resize",
  "w-resize",
  "ne-resize",
  "nw-resize",
  "se-resize",
  "sw-resize",
  "ew-resize",
  "ns-resize",
  "nesw-resize",
  "nwse-resize",
  "all-scroll",
  "none",
]);

/**
 * Reduce a computed `cursor` value from the remote page to a safe CSS
 * keyword for the canvas. Custom `url(...)` cursors fall back to their
 * trailing keyword (per CSS cursor syntax) or "default".
 */
export function sanitizeCursor(raw: unknown): string {
  if (typeof raw !== "string" || raw.length === 0 || raw.length > 256) return "default";
  // `cursor: url(a.png), url(b.png), pointer` — keyword is the last entry.
  const parts = raw.split(",");
  const keyword = parts[parts.length - 1].trim().toLowerCase();
  return CURSOR_KEYWORDS.has(keyword) ? keyword : "default";
}

// ---------------------------------------------------------------------------
// Daemon HTTP endpoints (same port as the stream WebSocket)
// ---------------------------------------------------------------------------

/** Derive the daemon's HTTP base URL from the stream WebSocket URL. */
export function httpBaseFromStreamUrl(streamUrl: string | null | undefined): string | null {
  if (!streamUrl || typeof streamUrl !== "string") return null;
  let url: URL;
  try {
    url = new URL(streamUrl);
  } catch {
    return null;
  }
  if (url.protocol === "ws:") url.protocol = "http:";
  else if (url.protocol === "wss:") url.protocol = "https:";
  else if (url.protocol !== "http:" && url.protocol !== "https:") return null;
  return url.origin;
}

/**
 * Unwrap a daemon command response. The daemon envelopes results as
 * `{id, success, data: {result, ...}}`; older shapes use a bare
 * `{result}`. Returns `null` when the command failed.
 */
export function parseDaemonResult(payload: unknown): unknown {
  if (payload === null || typeof payload !== "object") return null;
  const obj = payload as Record<string, unknown>;
  if (obj.success === false) return null;
  const data = obj.data;
  if (data !== null && typeof data === "object" && "result" in (data as object)) {
    return (data as Record<string, unknown>).result;
  }
  if ("result" in obj) return obj.result;
  return null;
}

/**
 * Split text into chunks of at most `size` UTF-16 code units without
 * splitting surrogate pairs (an emoji cut in half pastes as U+FFFD).
 */
export function chunkString(text: string, size: number): string[] {
  const chunks: string[] = [];
  let i = 0;
  while (i < text.length) {
    let end = Math.min(i + size, text.length);
    if (end < text.length) {
      const c = text.charCodeAt(end - 1);
      // High surrogate at the boundary — leave it for the next chunk.
      if (c >= 0xd800 && c <= 0xdbff) end--;
      // Pathological size=1: keep the pair together anyway.
      if (end === i) end = i + 2;
    }
    chunks.push(text.slice(i, end));
    i = end;
  }
  return chunks;
}

// ---------------------------------------------------------------------------
// Page scripts
//
// IMPORTANT: every script POSTed to the daemon's `/api/command` must
// keep the whole HTTP request (headers + body) inside a single TCP
// segment (~1.4KB). The daemon reads the request with one peek; when a
// request spans segments the body arrives after the peek and the
// daemon sees an empty body ("Invalid JSON" 400). Verified empirically:
// bodies ≤ ~870 bytes always survive, ≥ ~970 always fail. Hence the
// minified scripts and the small paste chunk size below.
// ---------------------------------------------------------------------------

/** Max UTF-16 code units per paste chunk — keeps each insertText eval
 *  body small enough for the daemon's single-segment HTTP read even at
 *  worst-case expansion (2× JSON escaping for ASCII, 3 UTF-8 bytes per
 *  unit for CJK). */
export const PASTE_CHUNK_SIZE = 120;

const TEXT_INPUT_TYPES = "text|search|url|tel|email|password|number";

/**
 * Resolve the effective CSS cursor under a remote-viewport point,
 * approximating the UA's `cursor: auto` resolution (links → pointer,
 * editable/text → text, else default). Minified — see module note.
 */
export function buildCursorProbeScript(x: number, y: number): string {
  const px = Math.round(x);
  const py = Math.round(y);
  return (
    `(()=>{const e=document.elementFromPoint(${px},${py});` +
    `if(!e)return"default";` +
    `const c=getComputedStyle(e).cursor;if(c&&c!=="auto")return c;` +
    `for(let n=e;n;n=n.parentElement)if(n.tagName==="A"&&n.hasAttribute("href"))return"pointer";` +
    `if(e.isContentEditable||e.tagName==="TEXTAREA")return"text";` +
    `if(e.tagName==="INPUT")return/^(${TEXT_INPUT_TYPES})$/.test(e.type)?"text":"default";` +
    `const r=document.caretRangeFromPoint?document.caretRangeFromPoint(${px},${py}):null;` +
    `if(r&&r.startContainer.nodeType===3){` +
    `const p=r.startContainer.parentElement,l=p?p.getClientRects():[];` +
    `for(const b of l)if(${px}>=b.left&&${px}<=b.right&&${py}>=b.top&&${py}<=b.bottom)return"text"}` +
    `return"default"})()`
  );
}

/** Read the current selection, including input/textarea selections. */
export const SELECTION_SCRIPT =
  `(()=>{const s=window.getSelection?String(window.getSelection()):"";` +
  `if(s)return s;const a=document.activeElement;` +
  `return a&&(a.tagName==="TEXTAREA"||a.tagName==="INPUT")&&typeof a.selectionStart==="number"&&a.selectionEnd>a.selectionStart` +
  `?a.value.slice(a.selectionStart,a.selectionEnd):""})()`;

/**
 * Insert text at the caret of the focused editable element. Returns
 * false when focus is not editable — Chrome's execCommand reports true
 * even with nothing editable focused, so the script gates on
 * editability itself; the pane falls back to forwarding the raw
 * keystroke in that case.
 */
export function buildInsertTextScript(text: string): string {
  return (
    `(()=>{const a=document.activeElement;if(!a)return false;` +
    `const t=a.tagName,k=a.isContentEditable||t==="TEXTAREA"||` +
    `t==="INPUT"&&!a.readOnly&&!a.disabled&&/^(${TEXT_INPUT_TYPES})$/.test(a.type);` +
    `return k?document.execCommand("insertText",false,${JSON.stringify(text)}):false})()`
  );
}

// ---------------------------------------------------------------------------
// Daemon HTTP helpers (IO)
// ---------------------------------------------------------------------------

async function fetchWithTimeout(input: string, init: RequestInit, timeoutMs: number): Promise<Response> {
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), timeoutMs);
  try {
    return await fetch(input, { ...init, signal: ctrl.signal });
  } finally {
    clearTimeout(timer);
  }
}

/**
 * Check whether the stream daemon is alive. Used by the pane's liveness
 * check instead of treating a quiet screencast as a dead connection —
 * CDP only emits frames on visual change, so static pages legitimately
 * produce none.
 */
export async function probeDaemon(httpBase: string, timeoutMs = 3000): Promise<boolean> {
  try {
    const res = await fetchWithTimeout(`${httpBase}/api/status`, {}, timeoutMs);
    return res.ok;
  } catch {
    return false;
  }
}

let evalSeq = 0;

/**
 * Evaluate a script on the remote page via the daemon's HTTP command
 * relay. This is the same daemon socket the CLI/agent path uses, but
 * without a process spawn per call — cheap enough for hover-driven
 * cursor probes. Returns the unwrapped result, or `null` on failure.
 */
export async function evalOnDaemon(httpBase: string, script: string, timeoutMs = 3000): Promise<unknown> {
  try {
    const res = await fetchWithTimeout(
      `${httpBase}/api/command`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ id: `pane-${++evalSeq}`, action: "evaluate", script }),
      },
      timeoutMs,
    );
    if (!res.ok) return null;
    return parseDaemonResult(await res.json());
  } catch {
    return null;
  }
}
