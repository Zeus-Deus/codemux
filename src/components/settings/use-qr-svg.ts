/**
 * Render a string to a self-contained QR-code SVG, client-side.
 *
 * Uses the `qrcode` package's `toString` (pure string generation — no
 * canvas), so it works in the desktop WebView, a plain browser, and jsdom
 * tests alike. QR modules are always dark-on-white regardless of the app
 * theme: phone cameras need high contrast, so the caller renders the SVG
 * on a white plate rather than inheriting the theme background.
 */
import { useEffect, useState } from "react";
import QRCode from "qrcode";

export function useQrSvg(text: string | null): string | null {
  const [svg, setSvg] = useState<string | null>(null);

  useEffect(() => {
    if (!text) {
      setSvg(null);
      return;
    }
    let cancelled = false;
    QRCode.toString(text, {
      type: "svg",
      margin: 1,
      errorCorrectionLevel: "M",
      color: { dark: "#0a0a0a", light: "#ffffff" },
    })
      .then((out) => {
        if (!cancelled) setSvg(out);
      })
      .catch((err) => {
        console.error("[remote-access] QR render failed:", err);
        if (!cancelled) setSvg(null);
      });
    return () => {
      cancelled = true;
    };
  }, [text]);

  return svg;
}
