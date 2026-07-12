/**
 * Persistent web-remote session storage.
 *
 * A paired browser stores its `{sessionId, sessionToken}` in
 * `localStorage` (per-origin, so two different desktop servers never
 * share a session). The token is the long-lived bearer credential the
 * transport exchanges for short-lived WebSocket tickets. Revocation on
 * the desktop invalidates it; the transport then clears this store and
 * the bootstrap flow falls back to the pairing screen.
 */

export interface RemoteSession {
  sessionId: string;
  sessionToken: string;
}

const SESSION_KEY = "codemux.web-remote.session";

/** Read the stored session, or `null` if none / malformed. */
export function loadSession(): RemoteSession | null {
  try {
    const raw = localStorage.getItem(SESSION_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Partial<RemoteSession>;
    if (
      typeof parsed.sessionId === "string" &&
      typeof parsed.sessionToken === "string"
    ) {
      return { sessionId: parsed.sessionId, sessionToken: parsed.sessionToken };
    }
    return null;
  } catch {
    return null;
  }
}

/** Persist the session. Best-effort — private-mode storage failures are
 *  swallowed (the in-memory session still drives the current page load). */
export function storeSession(session: RemoteSession): void {
  try {
    localStorage.setItem(SESSION_KEY, JSON.stringify(session));
  } catch {
    /* storage unavailable (private mode / quota) — non-fatal */
  }
}

/** Drop the session (revocation / unauthorized). */
export function clearSession(): void {
  try {
    localStorage.removeItem(SESSION_KEY);
  } catch {
    /* ignore */
  }
}

/**
 * A human-readable device label derived from the browser + OS, sent to
 * the desktop at pairing time so the devices list is legible ("Firefox
 * on Linux"). Deliberately coarse — this is a display label, not a
 * fingerprint.
 */
export function deriveDeviceName(): string {
  const ua = typeof navigator !== "undefined" ? navigator.userAgent || "" : "";
  const browser = /Edg\//.test(ua)
    ? "Edge"
    : /OPR\/|Opera/.test(ua)
      ? "Opera"
      : /Firefox\//.test(ua)
        ? "Firefox"
        : /Chrome\//.test(ua)
          ? "Chrome"
          : /Safari\//.test(ua)
            ? "Safari"
            : "Browser";
  const os = /Android/.test(ua)
    ? "Android"
    : /iPhone|iPad|iPod/.test(ua)
      ? "iOS"
      : /Windows/.test(ua)
        ? "Windows"
        : /Mac OS X|Macintosh/.test(ua)
          ? "macOS"
          : /Linux/.test(ua)
            ? "Linux"
            : typeof navigator !== "undefined" && navigator.platform
              ? navigator.platform
              : "Unknown";
  return `${browser} on ${os}`;
}
