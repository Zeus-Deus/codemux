import { useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { agentChatGetToolResultImage, readChatImage } from "@/tauri/commands";
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
 * bounded IPC image reader as user attachments when hosted clients cannot
 * reach that asset URL.
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
        {lightboxImage ? <LightboxImage key={lightboxImage.src} image={lightboxImage} onImageError={onImageError} /> : null}
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
interface ImageRequest { promise: Promise<ImageSource>; value?: ImageSource }
// Keys have the lifetime of the actual replay stub, so separate devices/DBs
// cannot collide on a row id. Cache only tiny local paths, never image data.
const imageRequests = new WeakMap<LazyToolResultStub, Map<string, ImageRequest>>();

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
  const [source, setSource] = useState<ImageSource | null>(
    () => imageRequests.get(stub)?.get(pointer)?.value ?? null,
  );
  const [failed, setFailed] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const mounted = useRef(false);
  const blobUrl = useRef<string | null>(null);
  const reading = useRef(false);
  useEffect(() => {
    mounted.current = true;
    setSource(imageRequests.get(stub)?.get(pointer)?.value ?? null);
    setFailed(false);
    imageRequest(stub, pointer).promise.then((value) => {
      if (mounted.current) setSource(value);
    }).catch(() => { if (mounted.current) setFailed(true); });
    return () => {
      mounted.current = false;
      if (blobUrl.current) URL.revokeObjectURL(blobUrl.current);
      blobUrl.current = null;
    };
  }, [stub, pointer, attempt]);

  const readFallback = () => {
    if (!source?.local || reading.current) return;
    reading.current = true;
    readChatImage(source.src).then(({ bytes, media_type }) => {
      if (!mounted.current) return;
      const url = URL.createObjectURL(new Blob([bytes], { type: media_type }));
      blobUrl.current = url;
      setSource({ src: url, local: false });
    }).catch(() => {
      imageRequests.get(stub)?.delete(pointer);
      if (mounted.current) setFailed(true);
    }).finally(() => { reading.current = false; });
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
