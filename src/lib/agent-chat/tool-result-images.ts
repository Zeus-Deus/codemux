/**
 * Pull renderable images out of a tool-result `content` payload.
 *
 * Agents return screenshots and other images inside a `tool_result`
 * whose `content` is an array of blocks. Two shapes occur in the wild:
 *
 *   1. Anthropic-standard base64 image block (what `Read` on a PNG and
 *      most screenshot tools emit):
 *        { type: "image",
 *          source: { type: "base64", media_type: "image/png", data: "<b64>" } }
 *
 *   2. URL image block (Anthropic `source.type: "url"`, or the
 *      OpenAI-style `{ type: "image_url", image_url: { url } }`):
 *        { type: "image", source: { type: "url", url: "https://…" } }
 *
 * The backend forwards these blocks through verbatim (tool-result
 * content is opaque JSON), so the data is already client-side — the
 * transcript renderers just never turned it into an `<img>`. This
 * helper is the single place that normalises both shapes into a
 * webview-loadable `src`, so the tool bodies can render a thumbnail
 * instead of dumping base64 text.
 */

export interface ToolResultImage {
  /** A directly renderable `src` — a `data:` URL or an http(s) URL. */
  src: string;
  /** MIME type when known, used for the `alt` text / lightbox label. */
  mediaType?: string;
}

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function str(v: unknown): string | null {
  return typeof v === "string" && v.length > 0 ? v : null;
}

/**
 * Only allow sources the webview can render without opening an XSS hole.
 * A malicious or buggy tool result could put `javascript:`/`file:` in a
 * URL block; we accept only `data:image/*` and http(s).
 */
function safeImageSrc(raw: string): string | null {
  const url = raw.trim();
  if (/^data:image\//i.test(url)) return url;
  if (/^https?:\/\//i.test(url)) return url;
  return null;
}

/** Normalise one entry to a `ToolResultImage`, or `null` if it isn't a
 *  renderable image. */
function imageFromEntry(entry: unknown): ToolResultImage | null {
  if (!isRecord(entry)) return null;

  // OpenAI-style { type: "image_url", image_url: { url } | url }
  if (entry.type === "image_url") {
    const iu = entry.image_url;
    const url = isRecord(iu) ? str(iu.url) : str(iu);
    if (!url) return null;
    const safe = safeImageSrc(url);
    return safe ? { src: safe } : null;
  }

  if (entry.type !== "image") return null;

  const source = entry.source;
  if (!isRecord(source)) return null;

  if (source.type === "base64") {
    const data = str(source.data);
    const mediaType = str(source.media_type) ?? "image/png";
    if (!data) return null;
    // Reject a non-image media type — the same base64 shape is used for
    // documents (PDFs) which the browser can't render as an <img>.
    if (!/^image\//i.test(mediaType)) return null;
    return { src: `data:${mediaType};base64,${data}`, mediaType };
  }

  if (source.type === "url") {
    const url = str(source.url);
    if (!url) return null;
    const safe = safeImageSrc(url);
    return safe ? { src: safe } : null;
  }

  return null;
}

/**
 * True only when an entry will actually render through
 * `extractToolResultImages`. Callers use this to suppress the matching raw
 * JSON fallback, so a malformed, unsafe, or non-image block must remain
 * false or its payload would disappear from the transcript entirely.
 */
export function isRenderableImageBlock(entry: unknown): boolean {
  return imageFromEntry(entry) !== null;
}

/**
 * Extract every renderable image from a tool-result `content` payload.
 * Returns `[]` for string/empty/non-image content, so callers can treat
 * a non-empty result as "render a thumbnail grid".
 */
export function extractToolResultImages(content: unknown): ToolResultImage[] {
  if (!Array.isArray(content)) return [];
  const out: ToolResultImage[] = [];
  for (const entry of content) {
    const image = imageFromEntry(entry);
    if (image) out.push(image);
  }
  return out;
}

/** Cheap presence check without building the `src` strings. */
export function hasToolResultImages(content: unknown): boolean {
  if (!Array.isArray(content)) return false;
  return content.some((entry) => imageFromEntry(entry) !== null);
}
