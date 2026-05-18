/**
 * Project avatar image resolution.
 *
 * Users can paste either:
 *   - a direct image URL (e.g. https://example.com/logo.png)
 *   - a data: URL
 *   - a website URL or bare domain (e.g. https://codemux.com or codemux.com)
 *
 * Direct images render as-is. For websites, we route through Google's
 * favicon service which handles redirects, multiple favicon sizes, and
 * sites that don't expose /favicon.ico at the root.
 */

const IMAGE_EXTENSIONS = /\.(png|jpe?g|gif|svg|webp|ico|avif|bmp)(\?.*)?$/i;

export interface ResolvedImage {
  /** The final URL to put on an <img src>. Empty when input is blank. */
  url: string;
  /** Whether we treated the input as a website and derived a favicon. */
  isFavicon: boolean;
  /** The domain we derived, if `isFavicon`. Useful for previews. */
  domain: string | null;
}

export function resolveImageUrl(input: string): ResolvedImage {
  const trimmed = input.trim();
  if (!trimmed) return { url: "", isFavicon: false, domain: null };

  // Data URLs render directly.
  if (trimmed.startsWith("data:")) {
    return { url: trimmed, isFavicon: false, domain: null };
  }

  // Direct image URL — recognised by extension.
  if (IMAGE_EXTENSIONS.test(trimmed)) {
    return { url: trimmed, isFavicon: false, domain: null };
  }

  // Try to interpret as a website / bare domain.
  const domain = extractDomain(trimmed);
  if (domain) {
    return {
      url: `https://www.google.com/s2/favicons?domain=${encodeURIComponent(domain)}&sz=128`,
      isFavicon: true,
      domain,
    };
  }

  // Fallback: pass through and let the <img onError> fallback handle it.
  return { url: trimmed, isFavicon: false, domain: null };
}

function extractDomain(input: string): string | null {
  let candidate = input;
  if (!/^https?:\/\//i.test(candidate)) {
    candidate = `https://${candidate}`;
  }
  try {
    const url = new URL(candidate);
    if (!url.hostname || !url.hostname.includes(".")) return null;
    return url.hostname.replace(/^www\./, "");
  } catch {
    return null;
  }
}
