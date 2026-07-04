/**
 * Single source of truth for "am I the web remote client?".
 *
 * `src/remote/shim.ts` sets `window.__CODEMUX_REMOTE__ = true` before the
 * app mounts when the UI is served to a browser on another machine. Every
 * Stage 3b fallback (web path picker, hidden window chrome, Web
 * Notifications) keys off this flag so desktop (Tauri) behavior stays
 * byte-identical — the flag is never set inside the real Tauri webview.
 */
export function isRemoteClient(): boolean {
  return (
    typeof window !== "undefined" &&
    (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__ === true
  );
}
