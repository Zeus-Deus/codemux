import { useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { agentChatGetToolResultImage } from "@/tauri/commands";
import type { LazyToolResultStub } from "@/lib/agent-chat/lazy-tool-result";
import { ImageIcon, ImageOff } from "lucide-react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";

import {
  ImageLightbox,
  IMAGE_LIGHTBOX_FALLBACK_CLASS,
  IMAGE_LIGHTBOX_MEDIA_CLASS,
} from "@/components/chat/ImageLightbox";
import type { ToolResultImage } from "@/lib/agent-chat/tool-result-images";

function needsRemoteConfirmation(image: ToolResultImage): boolean {
  return !image.trustedLocal && /^https?:\/\//i.test(image.src.trim());
}

/**
 * Renders images returned inside a tool result (e.g. a screenshot from
 * `Read` on a PNG, or a browser/screenshot tool). Mirrors the attached-
 * image thumbnail row on the user bubble (`UserMessage.tsx`): a wrapping
 * row of bounded thumbnails, each opening a near-fullscreen lightbox on
 * click. Lazy local sources use the protected asset transport, with the same
 * image-only data-URL fallback when hosted clients cannot reach that asset
 * URL. Its binary Blob cache is bounded by bytes, entries and idle time.
 */
interface LazyImageIdentity {
  stub: LazyToolResultStub;
  pointer: string;
}

export function ToolResultImages({ images, onImageError, lazyImageIdentity, onRetry, pending = false }: {
  images: ToolResultImage[];
  onImageError?: () => void;
  lazyImageIdentity?: LazyImageIdentity;
  onRetry?: () => void;
  pending?: boolean;
}) {
  const [lightboxSelection, setLightboxSelection] = useState<{
    index: number;
    src: string;
    localIdentity?: LazyImageIdentity;
  } | null>(null);
  const [approvedRemoteSources, setApprovedRemoteSources] = useState<Record<number, string>>({});
  const selectedImage = lightboxSelection ? images[lightboxSelection.index] : undefined;
  const selectedSrc = selectedImage?.src;
  const selectedSourceMatches = Boolean(lightboxSelection && selectedImage && (
    selectedSrc === lightboxSelection.src || (
      lightboxSelection.localIdentity && lazyImageIdentity &&
      lightboxSelection.localIdentity.stub === lazyImageIdentity.stub &&
      lightboxSelection.localIdentity.pointer === lazyImageIdentity.pointer &&
      selectedSrc?.startsWith("blob:")
    )
  ));
  const lightboxImage = !pending && !onRetry && lightboxSelection && selectedImage && selectedSourceMatches && (
    !needsRemoteConfirmation(selectedImage) || approvedRemoteSources[lightboxSelection.index] === selectedSrc
  ) ? selectedImage : undefined;

  useEffect(() => {
    if (lightboxSelection && (pending || onRetry || !selectedSourceMatches)) {
      setLightboxSelection(null);
    }
  }, [lightboxSelection, onRetry, pending, selectedSourceMatches]);

  const thumbnails: Array<ToolResultImage | null> = images.length === 0 && lazyImageIdentity ? [null] : images;
  if (thumbnails.length === 0) return null;

  return (
    <div className="flex flex-wrap gap-2">
      {thumbnails.map((image, i) => (
        <ToolResultThumbnail
          key={lazyImageIdentity ? i : `${image?.src}-${i}`}
          image={image}
          pending={pending}
          approved={!image || !needsRemoteConfirmation(image) || approvedRemoteSources[i] === image.src}
          onConfirm={() => {
            if (image) setApprovedRemoteSources((current) => ({ ...current, [i]: image.src }));
          }}
          onOpen={() => {
            if (image) setLightboxSelection({
              index: i,
              src: image.src,
              localIdentity: image.trustedLocal ? lazyImageIdentity : undefined,
            });
          }}
          onImageError={onImageError}
          onRetry={onRetry}
        />
      ))}

      <ImageLightbox
        open={lightboxImage !== undefined}
        onOpenChange={(open) => {
          if (!open) setLightboxSelection(null);
        }}
        title="Tool result image"
      >
        {lightboxImage ? <LightboxImage key={`${lightboxImage.src}-${lightboxSelection?.index}`} image={lightboxImage} onImageError={onImageError} /> : null}
      </ImageLightbox>
    </div>
  );
}

/** A single clickable thumbnail with its own broken-image fallback so
 *  one bad source can't blank the rest of the row. */
function ToolResultThumbnail({
  image,
  approved,
  onConfirm,
  onOpen,
  onImageError,
  onRetry,
  pending,
}: {
  image: ToolResultImage | null;
  approved: boolean;
  onConfirm: () => void;
  onOpen: () => void;
  onImageError?: () => void;
  onRetry?: () => void;
  pending: boolean;
}) {
  const [failed, setFailed] = useState(false);
  const [confirmationSrc, setConfirmationSrc] = useState<string | null>(null);
  const approvedButtonRef = useRef<HTMLButtonElement>(null);
  const focusApprovedRef = useRef(false);

  useEffect(() => {
    setFailed(false);
  }, [image?.src]);

  useEffect(() => {
    // Approval removes the dialog trigger; focus its replacement after React mounts it.
    if (!approved || !focusApprovedRef.current) return;
    focusApprovedRef.current = false;
    approvedButtonRef.current?.focus({ preventScroll: true });
  }, [approved]);

  if (image && !pending && !approved) {
    let host: string;
    try {
      host = new URL(image.src).host || "Invalid address";
    } catch {
      host = "Invalid address";
    }
    return (
      <AlertDialog
        open={confirmationSrc === image.src}
        onOpenChange={(open) => setConfirmationSrc(open ? image.src : null)}
      >
        <AlertDialogTrigger asChild>
          <button
            type="button"
            className="flex h-20 w-28 cursor-pointer flex-col items-center justify-center gap-1 rounded-lg border border-hairline bg-surface-1 text-label text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus-ring"
            aria-label="Load remote image"
          >
            <ImageIcon className="size-5" aria-hidden />
            Load remote image
          </button>
        </AlertDialogTrigger>
        <AlertDialogContent className="flex max-h-[calc(100dvh-2rem)] flex-col overflow-hidden">
          <div role="region" aria-label="Image confirmation details" className="thin-scrollbar min-h-0 space-y-4 overflow-y-auto" tabIndex={0}>
            <AlertDialogHeader>
              <AlertDialogTitle>Load remote image?</AlertDialogTitle>
              <AlertDialogDescription>
                Loading this image will contact its host. Check the address before continuing.
              </AlertDialogDescription>
            </AlertDialogHeader>
            <p className="break-all text-label text-muted-foreground">Host: <span className="font-medium text-foreground">{host}</span></p>
            <div
              role="region"
              aria-label="Full image URL"
              tabIndex={0}
              className="thin-scrollbar max-h-48 overflow-y-auto select-text break-all rounded-lg bg-muted px-3 py-2 font-mono text-label text-muted-foreground"
            >
              {image.src}
            </div>
          </div>
          <AlertDialogFooter className="shrink-0">
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction onClick={() => {
              focusApprovedRef.current = true;
              onConfirm();
              setConfirmationSrc(null);
            }}>
              Load image
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    );
  }

  if (pending || !image || failed || onRetry) {
    const loading = pending || (!image && !onRetry);
    return (
      <button
        ref={approvedButtonRef}
        type="button"
        onClick={loading ? undefined : () => onRetry ? onRetry() : setFailed(false)}
        className="flex h-20 w-28 cursor-pointer flex-col items-center justify-center gap-1 rounded-lg border border-hairline bg-surface-1 text-label text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus-ring"
        aria-label={loading ? "Loading image" : onRetry ? "Image preview unavailable — retry" : "Retry image"}
        aria-disabled={loading || undefined}
        aria-busy={loading || undefined}
      >
        {loading ? "Loading image…" : <><ImageOff className="size-5 opacity-40" aria-hidden />Retry image</>}
      </button>
    );
  }

  return (
    <button
      ref={approvedButtonRef}
      type="button"
      onClick={onOpen}
      aria-label="Open image"
      className="cursor-pointer overflow-hidden rounded-lg border border-hairline transition-opacity duration-150 hover:opacity-90 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus-ring"
    >
      <img
        src={image.src}
        referrerPolicy="no-referrer"
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
      referrerPolicy="no-referrer"
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
  const [pending, setPending] = useState(() => !cached?.blob && !cached?.value);
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
    const current = request.blob ? { src: request.blob.src, local: false } : request.value ?? null;
    setSource(current);
    setPending(current === null);
    setFailed(false);
    request.promise.then((value) => {
      if (mounted.current) {
        setSource(request.blob ? { src: request.blob.src, local: false } : value);
        setPending(false);
      }
    }).catch(() => {
      if (mounted.current) {
        setFailed(true);
        setPending(false);
      }
    });
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

  return <ToolResultImages images={source ? [{
      src: source.local ? convertFileSrc(source.src) : source.src,
      trustedLocal: source.local,
    }] : []}
      lazyImageIdentity={{ stub, pointer }}
      pending={pending}
      onRetry={failed ? () => {
        setPending(true);
        setAttempt((value) => value + 1);
      } : undefined}
      onImageError={source?.local ? readFallback : undefined} />;
}
