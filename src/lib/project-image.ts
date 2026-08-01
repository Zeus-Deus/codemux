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

// Allow an optional trailing query string (?v=1) or fragment (#section) after
// the extension — both are valid on a direct image URL and must not cause it to
// be misread as a website and routed through the favicon service.
const IMAGE_EXTENSIONS = /\.(png|jpe?g|gif|svg|webp|ico|avif|bmp)([?#].*)?$/i;

export interface ResolvedImage {
  /** The final URL to put on an <img src>. Empty when input is blank. */
  url: string;
  /** Whether we treated the input as a website and derived a favicon. */
  isFavicon: boolean;
  /** The domain we derived, if `isFavicon`. Useful for previews. */
  domain: string | null;
}

/**
 * Build the shared Google favicon-service URL used by project avatars and
 * rich external links in chat.
 */
export function faviconUrlForDomain(
  domain: string,
  size: number,
  cacheBust?: string | number | null,
): string {
  let url = `https://www.google.com/s2/favicons?domain=${encodeURIComponent(domain)}&sz=${size}`;
  if (cacheBust) {
    url += `&v=${encodeURIComponent(String(cacheBust))}`;
  }
  return url;
}

/**
 * @param input     Raw user input (image URL, data URL, website, or domain).
 * @param cacheBust Optional token appended to derived favicon URLs as `&v=`.
 *   The favicon service URL is otherwise identical for a given domain, so the
 *   WebView's image cache serves the same bytes forever — meaning a site that
 *   changes its favicon never visibly updates. Passing a token that changes
 *   when the user re-saves (or re-opens the picker) forces a fresh fetch.
 *   Only applied to derived favicons; direct/data URLs are passed through
 *   untouched so we never corrupt a signed or already-complete URL.
 */
export function resolveImageUrl(
  input: string,
  cacheBust?: string | number | null,
): ResolvedImage {
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
    const url = faviconUrlForDomain(domain, 128, cacheBust);
    return { url, isFavicon: true, domain };
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
