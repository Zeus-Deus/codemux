/**
 * Shared WebKitGTK detection. Extracted from `webgl-renderer-probe.ts` so
 * both the terminal renderer probe and the chat transcript fade gate can key
 * off the same engine check (issue #129). The terminal probe re-exports it
 * for backwards compatibility.
 */

/**
 * True when running inside Linux WebKitGTK — the Tauri app webview on Linux.
 * WebKit UA without a Chromium token, on a non-mac platform: WebKitGTK is the
 * only such engine Codemux can meet (Chromium-family UAs all carry "Chrome";
 * macOS WKWebView carries "Macintosh"/"Mac OS X").
 */
export function isLinuxWebKitGtk(userAgent: string): boolean {
  return (
    /AppleWebKit/i.test(userAgent) &&
    !/Chrom(e|ium)|Edg\//i.test(userAgent) &&
    !/Macintosh|Mac OS X/i.test(userAgent)
  );
}
