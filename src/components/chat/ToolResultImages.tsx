import { useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { agentChatGetToolResultImage } from "@/tauri/commands";
import type { LazyToolResultStub } from "@/lib/agent-chat/lazy-tool-result";
import { ImageOff } from "lucide-react";

import {
  ImageLightbox,
  IMAGE_LIGHTBOX_FALLBACK_CLASS,
  IMAGE_LIGHTBOX_MEDIA_CLASS,
} from "@/components/chat/ImageLightbox";
import type { ToolResultImage } from "@/lib/agent-chat/tool-result-images";

/**
 * Renders images returned inside a tool result (e.g. a screenshot from
 * `Read` on a PNG, or a browser/screenshot tool). Mirrors the attached-
 * image thumbnail row on the user bubble (`UserMessage.tsx`): a wrapping
 * row of bounded thumbnails, each opening a near-fullscreen lightbox on
 * click. Lazy local sources use the protected asset transport, with the same
 * image-only data-URL fallback when hosted clients cannot reach that asset
 * URL. Its binary Blob cache is bounded by bytes, entries and idle time.
 */
export function ToolResultImages({ images, onImageError }: {
  images: ToolResultImage[];
  onImageError?: () => void;
}) {
  const [lightboxIndex, setLightboxIndex] = useState<number | null>(null);
  const lightboxImage =
    lightboxIndex !== null ? images[lightboxIndex] : undefined;

  if (images.length === 0) return null;

  return (
    <div className="flex flex-wrap gap-2">
      {images.map((image, i) => (
        <ToolResultThumbnail
          key={`${image.src.slice(0, 48)}-${i}`}
          image={image}
          onOpen={() => setLightboxIndex(i)}
          onImageError={onImageError}
        />
      ))}

      <ImageLightbox
        open={lightboxImage !== undefined}
        onOpenChange={(open) => {
          if (!open) setLightboxIndex(null);
        }}
        title="Tool result image"
      >
        {lightboxImage ? <LightboxImage key={`${lightboxImage.src.slice(0, 48)}-${lightboxIndex}`} image={lightboxImage} onImageError={onImageError} /> : null}
      </ImageLightbox>
    </div>
  );
}

/** A single clickable thumbnail with its own broken-image fallback so
 *  one bad source can't blank the rest of the row. */
function ToolResultThumbnail({
  image,
  onOpen,
  onImageError,
}: {
  image: ToolResultImage;
  onOpen: () => void;
  onImageError?: () => void;
}) {
  const [failed, setFailed] = useState(false);

  if (failed) {
    return (
      <div
        className="flex h-20 w-28 items-center justify-center rounded-lg border border-hairline bg-surface-1 text-muted-foreground"
        aria-label="Image failed to load"
      >
        <ImageOff className="size-5 opacity-40" aria-hidden />
      </div>
    );
  }

  return (
    <button
      type="button"
      onClick={onOpen}
      aria-label="Open image"
      className="cursor-pointer overflow-hidden rounded-lg border border-hairline transition-opacity duration-150 hover:opacity-90 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/60"
    >
      <img
        src={image.src}
        alt={image.mediaType ?? "tool result image"}
        className="max-h-40 max-w-[240px] object-contain"
        onError={onImageError ?? (() => setFailed(true))}
      />
    </button>
  );
}

/** The blown-up image inside the lightbox. Keeps its own error state so
 *  a source that fails at full size degrades to a placeholder rather than
 *  a browser broken-image icon. */
function LightboxImage({ image, onImageError }: {
  image: ToolResultImage;
  onImageError?: () => void;
}) {
  const [failed, setFailed] = useState(false);

  if (failed) {
    return (
      <div className={IMAGE_LIGHTBOX_FALLBACK_CLASS}>
        <ImageOff className="size-8 opacity-40" aria-hidden />
        <span className="text-label">Failed to load image</span>
      </div>
    );
  }

  return (
    <img
      src={image.src}
      alt={image.mediaType ?? "tool result image"}
      className={IMAGE_LIGHTBOX_MEDIA_CLASS}
      onError={onImageError ?? (() => setFailed(true))}
    />
  );
}

/** Only mounted (visible) tool bodies request image assets. No full-result
 *  hydration is needed for either the thumbnail or its lightbox. */
export function LazyToolResultImages({ stub }: { stub: LazyToolResultStub }) {
  return (
    <div className="flex flex-wrap gap-2">
      {stub.images?.map(({ pointer, src }) => src ? (
        <ToolResultImages key={pointer} images={[{ src }]} />
      ) : (
        <LazyToolResultImage key={`${stub.row_id}:${pointer}`} stub={stub} pointer={pointer} />
      ))}
    </div>
  );
}

type ImageSource = Awaited<ReturnType<typeof agentChatGetToolResultImage>>;
interface ImageRequest {
  promise: Promise<ImageSource>;
  value?: ImageSource;
  users: number;
  blob?: { src: string; bytes: number };
  inlinePromise?: Promise<ImageSource>;
  idleTimer?: ReturnType<typeof setTimeout>;
}
// Keys live with the actual replay stub: different devices/DBs cannot collide
// on a row id. Raw base64 is never retained after creating a binary Blob.
const imageRequests = new WeakMap<LazyToolResultStub, Map<string, ImageRequest>>();
const idleBlobs = new Map<ImageRequest, true>();
const MAX_BLOB_BYTES = 32 * 1024 * 1024;
const MAX_BLOB_ENTRIES = 32;
const BLOB_IDLE_MS = 60_000;
let blobBytes = 0;

function evictBlob(request: ImageRequest) {
  if (!request.blob || request.users > 0) return;
  URL.revokeObjectURL(request.blob.src);
  blobBytes -= request.blob.bytes;
  request.blob = undefined;
  if (request.idleTimer) clearTimeout(request.idleTimer);
  request.idleTimer = undefined;
  idleBlobs.delete(request);
}

function trimBlobs() {
  for (const request of idleBlobs.keys()) {
    if (blobBytes <= MAX_BLOB_BYTES && idleBlobs.size <= MAX_BLOB_ENTRIES) break;
    evictBlob(request);
  }
}

function releaseImage(request: ImageRequest) {
  request.users -= 1;
  if (request.users === 0 && request.blob) {
    request.idleTimer = setTimeout(() => evictBlob(request), BLOB_IDLE_MS);
    trimBlobs();
  }
}

function inlineImage(stub: LazyToolResultStub, pointer: string, request: ImageRequest) {
  if (request.blob) return Promise.resolve({ src: request.blob.src, local: false });
  if (!request.inlinePromise) {
    request.inlinePromise = agentChatGetToolResultImage(stub.row_id, pointer, true)
      .then(async (source) => {
        const response = await fetch(source.src);
        const blob = await response.blob();
        const src = URL.createObjectURL(blob);
        request.blob = { src, bytes: blob.size };
        blobBytes += blob.size;
        idleBlobs.set(request, true);
        // A request can finish after its body unmounted. Release its unused
        // URL on the same idle lifetime rather than leaking it indefinitely.
        if (request.users === 0) {
          request.idleTimer = setTimeout(() => evictBlob(request), BLOB_IDLE_MS);
        }
        trimBlobs();
        return { src, local: false };
      }).finally(() => { request.inlinePromise = undefined; });
  }
  return request.inlinePromise;
}

function imageRequest(stub: LazyToolResultStub, pointer: string): ImageRequest {
  let requests = imageRequests.get(stub);
  if (!requests) {
    requests = new Map();
    imageRequests.set(stub, requests);
  }
  const existing = requests.get(pointer);
  if (existing) return existing;
  const cache = requests;
  const request: ImageRequest = {
    users: 0,
    promise: agentChatGetToolResultImage(stub.row_id, pointer).then((source) => {
      if (source.local) request.value = source;
      else cache.delete(pointer);
      return source;
    }).catch((error) => { cache.delete(pointer); throw error; }),
  };
  cache.set(pointer, request);
  return request;
}

function LazyToolResultImage({ stub, pointer }: { stub: LazyToolResultStub; pointer: string }) {
  const cached = imageRequests.get(stub)?.get(pointer);
  const [source, setSource] = useState<ImageSource | null>(
    () => cached?.blob ? { src: cached.blob.src, local: false } : cached?.value ?? null,
  );
  const [failed, setFailed] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const mounted = useRef(false);
  const activeRequest = useRef<ImageRequest | null>(null);
  useEffect(() => {
    mounted.current = true;
    const request = imageRequest(stub, pointer);
    activeRequest.current = request;
    request.users += 1;
    if (request.idleTimer) clearTimeout(request.idleTimer);
    request.idleTimer = undefined;
    if (request.blob) {
      idleBlobs.delete(request);
      idleBlobs.set(request, true);
    }
    setSource(request.blob ? { src: request.blob.src, local: false } : request.value ?? null);
    setFailed(false);
    request.promise.then((value) => {
      if (mounted.current) setSource(request.blob ? { src: request.blob.src, local: false } : value);
    }).catch(() => { if (mounted.current) setFailed(true); });
    return () => { mounted.current = false; activeRequest.current = null; releaseImage(request); };
  }, [stub, pointer, attempt]);

  const readFallback = () => {
    if (!source?.local) return;
    const request = activeRequest.current;
    if (!request) return;
    inlineImage(stub, pointer, request).then((value) => {
      if (mounted.current) setSource(value);
    }).catch(() => {
      const cache = imageRequests.get(stub);
      if (cache?.get(pointer) === request) cache.delete(pointer);
      if (mounted.current) setFailed(true);
    });
  };

  if (source && !failed) {
    return <ToolResultImages images={[{ src: source.local ? convertFileSrc(source.src) : source.src }]}
      onImageError={source.local ? readFallback : undefined} />;
  }
  return (
    <button type="button" disabled={!failed} onClick={() => setAttempt((value) => value + 1)}
      className="flex h-20 w-28 items-center justify-center rounded-lg border border-hairline bg-surface-1 text-label text-muted-foreground"
      aria-label={failed ? "Image preview unavailable — retry" : "Loading image"}
      title={failed ? "Retry the image preview, or load full output for the original data" : undefined}>
      {failed ? "Retry image" : "Loading image…"}
    </button>
  );
}
