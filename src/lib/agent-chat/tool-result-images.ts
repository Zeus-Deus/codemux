import { lazyToolResultStub } from "./lazy-tool-result";

/**
 * Pull renderable images out of a tool-result `content` payload.
 *
 * Agents return screenshots and other images inside a `tool_result`
 * whose `content` is an array of blocks. Supported shapes include:
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
 *   3. OpenCode native file block:
 *        { type: "file", uri: "data:image/png;base64,…", mime: "image/png" }
 *
 *   4. Codex native MCP / dynamic output blocks:
 *        { type: "image", data: "<b64>", mimeType: "image/png" }
 *        { type: "inputImage", imageUrl: "data:image/png;base64,…" }
 *
 * Full provider content is opaque JSON. Oversized results can instead carry
 * lazy image handles; `LazyToolResultImages` resolves those independently of
 * the full body. This helper normalizes loaded blocks to image sources.
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

  // Codex dynamic tool output uses inputImage even in a result envelope.
  if (entry.type === "inputImage") {
    const url = str(entry.imageUrl);
    const safe = url && safeImageSrc(url);
    return safe ? { src: safe } : null;
  }

  if (entry.type === "file") {
    const mediaType = str(entry.mime);
    const uri = str(entry.uri);
    if (!mediaType || !/^image\//i.test(mediaType) || !uri) return null;
    const safe = safeImageSrc(uri);
    return safe ? { src: safe, mediaType } : null;
  }

  if (entry.type !== "image") return null;

  const source = entry.source;
  // Native MCP image blocks carry their MIME type beside the base64 data.
  if (source === undefined) {
    const data = str(entry.data);
    const mediaType = str(entry.mimeType);
    return data && mediaType && /^image\//i.test(mediaType)
      ? { src: `data:${mediaType};base64,${data}`, mediaType }
      : null;
  }
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
  const out: ToolResultImage[] = [];
  visitImageContent(content, (image) => {
    out.push(image);
    return false;
  });
  return out;
}

/** Provider envelopes wrap blocks under these result containers. Never scan
 *  arbitrary arguments or metadata for things that merely look like images.
 *  Returning true from the visitor stops the walk, for presence checks. */
function visitImageContent(
  content: unknown,
  visit: (image: ToolResultImage, pointer: string) => boolean,
  pointer = "",
): boolean {
  const image = imageFromEntry(content);
  if (image) {
    return visit(image, pointer);
  } else if (Array.isArray(content)) {
    return content.some((entry, index) =>
      visitImageContent(entry, visit, `${pointer}/${index}`),
    );
  } else if (isRecord(content)) {
    for (const key of ["result", "content", "contentItems"]) {
      if (key in content && visitImageContent(content[key], visit, `${pointer}/${key}`)) {
        return true;
      }
    }
  }
  return false;
}

/** Mirrors the Rust wire projection for the browser mock. */
export function toolResultImagePointers(
  content: unknown,
): { pointer: string; src?: string }[] {
  const pointers: { pointer: string; src?: string }[] = [];
  visitImageContent(content, (image, pointer) => {
    const src = image.src;
    const match = /^data:image\/(?:png|jpe?g|gif|webp);base64,([^]*)$/i.exec(src);
    const materializable = match &&
      match[1].length <= Math.ceil(25 * 1024 * 1024 / 3) * 4;
    pointers.push(
      src.length <= 32 * 1024 || !materializable ? { pointer, src } : { pointer },
    );
    return false;
  });
  return pointers;
}

/** Render projection only: the store and full-result copy retain every byte. */
export function withoutToolResultImages(content: unknown): unknown {
  if (isRenderableImageBlock(content)) return null;
  if (Array.isArray(content)) {
    return content
      .filter((entry) => !isRenderableImageBlock(entry))
      .map(withoutToolResultImages);
  }
  if (isRecord(content)) {
    const out = { ...content };
    for (const key of ["result", "content", "contentItems"]) {
      if (key in out) out[key] = withoutToolResultImages(out[key]);
    }
    return out;
  }
  return content;
}

/** Presence check includes lazy image metadata so disclosure/turn folding stays
 *  identical before and after loading the full result. */
export function hasToolResultImages(content: unknown): boolean {
  const stub = lazyToolResultStub(content);
  if (stub) return stub.has_images;
  return visitImageContent(content, () => true);
}
