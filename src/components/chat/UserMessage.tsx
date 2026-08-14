import { memo, useCallback, useState } from "react";
import { CornerDownLeft, ImageOff, Undo2, X } from "lucide-react";

import {
  ImageLightbox,
  IMAGE_LIGHTBOX_FALLBACK_CLASS,
  IMAGE_LIGHTBOX_MEDIA_CLASS,
} from "@/components/chat/ImageLightbox";
import { isAbsoluteFsPath, resolveAssetSrc } from "@/lib/asset-url";
import { readChatImage } from "@/tauri/commands";
import type { UserMessageImage, UserMessageItem } from "@/lib/agent-chat/types";
import { cn } from "@/lib/utils";

/**
 * Blob URLs for images that couldn't load via the asset protocol and
 * had to be read back over IPC (`readChatImage`). Keyed by the original
 * absolute fs path so every card referencing the same hydrated image
 * shares one object URL. Deliberately not revoked (a shared URL a live
 * `<img>` may still point at); bounded so a very long session can't leak
 * unboundedly.
 */
const blobUrlCache = new Map<string, string>();
const BLOB_CACHE_MAX = 64;

/**
 * Resolve an image `src` to something the webview can render, with an
 * IPC-read fallback. `data:` URLs and successfully asset-converted paths
 * render directly; when an absolute fs path fails to load (dev mock /
 * web-remote can't reach the asset protocol) we read the bytes over IPC
 * and render a blob URL instead, only surfacing the broken-image
 * placeholder when that also fails.
 */
function useImageWithFallback(rawSrc: string): {
  src: string | undefined;
  failed: boolean;
  onError: () => void;
} {
  const resolved = resolveAssetSrc(rawSrc, null);
  const [blobSrc, setBlobSrc] = useState<string | null>(
    () => blobUrlCache.get(rawSrc) ?? null,
  );
  const [failed, setFailed] = useState(false);

  const onError = useCallback(() => {
    // The blob URL itself failed, or the source isn't a local path we can
    // read back — give up and show the placeholder.
    if (blobSrc || !isAbsoluteFsPath(rawSrc)) {
      setFailed(true);
      return;
    }
    const cached = blobUrlCache.get(rawSrc);
    if (cached) {
      setBlobSrc(cached);
      return;
    }
    void (async () => {
      try {
        const { bytes, media_type } = await readChatImage(rawSrc);
        const url = URL.createObjectURL(new Blob([bytes], { type: media_type }));
        if (blobUrlCache.size >= BLOB_CACHE_MAX) {
          const oldest = blobUrlCache.keys().next().value;
          if (oldest !== undefined) blobUrlCache.delete(oldest);
        }
        blobUrlCache.set(rawSrc, url);
        setBlobSrc(url);
      } catch {
        setFailed(true);
      }
    })();
  }, [rawSrc, blobSrc]);

  return { src: blobSrc ?? resolved, failed, onError };
}

/**
 * Right-aligned user bubble (design D3). Card fill, soft border, the
 * asymmetric `14px 14px 5px 14px` radius that tucks the bottom-right
 * corner toward the composer, and pre-wrapped text so pasted snippets
 * keep their line breaks. Width is capped so long turns don't span the
 * whole 760px column.
 *
 * Attached images (paste / drop / picker) render as a wrapping
 * thumbnail row above the text; clicking one opens it near-fullscreen
 * in a lightbox. Sources are either `data:` URLs (fresh optimistic
 * send) or absolute filesystem paths (hydrated) — `resolveAssetSrc`
 * normalises both into a webview-loadable URL, so the bubble never has
 * to know which form it got.
 *
 * Follow-up queueing: while `item.queued` is set the bubble renders
 * visually muted with a quiet "Queued" footer anchored to the bubble's
 * right edge. On hover or keyboard focus, two compact actions appear to the
 * footer's left without moving the status: "Send now" soft-interrupts the
 * active turn and dispatches this message immediately (keeping all progress),
 * while X cancels and restores the text into the composer. Both are handled
 * by the parent. All colors are theme tokens.
 */
export const UserMessage = memo(function UserMessage({
  item,
  onCancelQueued,
  onSendQueuedNow,
  onRevert,
  reverting = false,
}: {
  item: UserMessageItem;
  onCancelQueued?: (queuedId: string, text: string) => void;
  onSendQueuedNow?: (queuedId: string) => void;
  onRevert?: () => void;
  reverting?: boolean;
}) {
  const queued = item.queued;
  const images = item.images ?? [];
  // Index of the image currently shown in the lightbox, or `null` when
  // it's closed. Tracked here (not per-thumbnail) so the same dialog
  // instance serves every image in the turn.
  const [lightboxIndex, setLightboxIndex] = useState<number | null>(null);
  const lightboxImage =
    lightboxIndex !== null ? images[lightboxIndex] : undefined;

  return (
    <div className="group flex justify-end">
      <div className="flex max-w-[82%] flex-col items-end gap-1">
        <div
          className={cn(
            "conversation-text flex flex-col gap-2 rounded-[14px_14px_5px_14px] border px-[15px] py-[11px] leading-relaxed",
            queued
              ? "border-border/35 bg-muted/30 text-muted-foreground/75"
              : "border-border/60 bg-card text-foreground",
          )}
        >
          {images.length > 0 ? (
            <div className="flex flex-wrap justify-end gap-2">
              {images.map((image, i) => (
                <ImageThumbnail
                  key={`${image.src}-${i}`}
                  image={image}
                  onOpen={() => setLightboxIndex(i)}
                />
              ))}
            </div>
          ) : null}
          {item.text ? (
            <div className="select-text whitespace-pre-wrap break-words">{item.text}</div>
          ) : null}
        </div>
        {queued ? (
          <div className="relative flex h-4 items-center justify-end pr-0.5">
            <div className="pointer-events-none absolute right-full mr-1 flex h-4 translate-x-0.5 items-center gap-px opacity-0 transition-[opacity,transform] duration-100 group-focus-within:pointer-events-auto group-focus-within:translate-x-0 group-focus-within:opacity-100 group-hover:pointer-events-auto group-hover:translate-x-0 group-hover:opacity-100">
              {onSendQueuedNow ? (
                <button
                  type="button"
                  aria-label="Send now"
                  title="Interrupt current work and send this message now — progress so far is kept"
                  onClick={() => onSendQueuedNow(queued.queuedId)}
                  className="inline-flex h-4 w-4 items-center justify-center rounded-[4px] text-muted-foreground/55 transition-colors hover:bg-muted/60 hover:text-foreground/80 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                >
                  <CornerDownLeft className="h-2.5 w-2.5" aria-hidden />
                </button>
              ) : null}
              {onCancelQueued ? (
                <button
                  type="button"
                  aria-label="Cancel queued message"
                  title="Remove from queue and return to the composer"
                  onClick={() => onCancelQueued(queued.queuedId, item.text)}
                  className="inline-flex h-4 w-4 items-center justify-center rounded-[4px] text-muted-foreground/55 transition-colors hover:bg-destructive/10 hover:text-destructive/80 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                >
                  <X className="h-2.5 w-2.5" aria-hidden />
                </button>
              ) : null}
            </div>
            <span className="inline-flex h-4 items-center gap-1 text-[10px] font-medium text-muted-foreground/55">
              <span className="h-1 w-1 rounded-full bg-muted-foreground/45" aria-hidden />
              Queued
            </span>
          </div>
        ) : onRevert ? (
          <button
            type="button"
            aria-label="Revert to before this turn"
            title="Revert this turn and everything after it"
            disabled={reverting}
            onClick={onRevert}
            className="flex items-center gap-0.5 rounded text-[10px] text-muted-foreground opacity-0 transition-opacity hover:text-foreground focus-visible:opacity-100 disabled:cursor-wait disabled:opacity-60 group-hover:opacity-100"
            data-testid="revert-turn-button"
          >
            <Undo2 className="h-3 w-3" aria-hidden />
            {reverting ? "Reverting…" : "Revert"}
          </button>
        ) : null}
      </div>

      <ImageLightbox
        open={lightboxImage !== undefined}
        onOpenChange={(open) => {
          if (!open) setLightboxIndex(null);
        }}
        title="Attached image"
      >
        {lightboxImage ? <LightboxImage image={lightboxImage} /> : null}
      </ImageLightbox>
    </div>
  );
});

/** A single clickable thumbnail. Owns its own broken-image state so one
 *  unreadable path can't blank the others; the fallback keeps the same
 *  footprint so the row layout doesn't jump. */
function ImageThumbnail({
  image,
  onOpen,
}: {
  image: UserMessageImage;
  onOpen: () => void;
}) {
  const { src, failed, onError } = useImageWithFallback(image.src);

  if (failed || !src) {
    return (
      <div
        className="flex h-20 w-28 items-center justify-center rounded-lg border border-border/60 bg-muted/40 text-muted-foreground"
        aria-label="Image failed to load"
      >
        <ImageOff className="h-5 w-5 opacity-40" aria-hidden />
      </div>
    );
  }

  return (
    <button
      type="button"
      onClick={onOpen}
      aria-label="Open attached image"
      className="cursor-pointer overflow-hidden rounded-lg border border-border/60 transition-opacity hover:opacity-90 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
    >
      <img
        src={src}
        alt={image.mediaType ?? "attached image"}
        className="max-h-40 max-w-[200px] object-contain"
        onError={onError}
      />
    </button>
  );
}

/** The blown-up image inside the lightbox. Keeps its own error state so
 *  a path that reads at thumbnail size but fails at full size still
 *  degrades to a placeholder rather than a browser broken-image icon. */
function LightboxImage({ image }: { image: UserMessageImage }) {
  const { src, failed, onError } = useImageWithFallback(image.src);

  if (failed || !src) {
    return (
      <div className={IMAGE_LIGHTBOX_FALLBACK_CLASS}>
        <ImageOff className="h-8 w-8 opacity-40" aria-hidden />
        <span className="text-xs">Failed to load image</span>
      </div>
    );
  }

  return (
    <img
      src={src}
      alt={image.mediaType ?? "attached image"}
      className={IMAGE_LIGHTBOX_MEDIA_CLASS}
      onError={onError}
    />
  );
}
