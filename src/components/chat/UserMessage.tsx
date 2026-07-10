import { memo, useState } from "react";
import { ImageOff, X } from "lucide-react";

import {
  Dialog,
  DialogContent,
  DialogTitle,
} from "@/components/ui/dialog";
import { resolveAssetSrc } from "@/lib/asset-url";
import type { UserMessageImage, UserMessageItem } from "@/lib/agent-chat/types";
import { cn } from "@/lib/utils";

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
 * greyed-out (reduced opacity + muted foreground) with a small "Queued"
 * pill and, on hover, an X to cancel — cancelling restores the text into
 * the composer (handled by the parent). All colors are theme tokens.
 */
export const UserMessage = memo(function UserMessage({
  item,
  onCancelQueued,
}: {
  item: UserMessageItem;
  onCancelQueued?: (queuedId: string, text: string) => void;
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
            "flex flex-col gap-2 rounded-[14px_14px_5px_14px] border px-[15px] py-[11px] text-[13.5px] leading-[1.55]",
            queued
              ? "border-dashed border-border/50 bg-muted/40 text-muted-foreground opacity-70"
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
            <div className="whitespace-pre-wrap break-words">{item.text}</div>
          ) : null}
        </div>
        {queued ? (
          <div className="flex items-center gap-1.5 pr-0.5">
            <span className="rounded-full bg-muted px-2 py-[1px] text-[10px] font-medium uppercase tracking-wide text-muted-foreground">
              Queued
            </span>
            {onCancelQueued ? (
              <button
                type="button"
                aria-label="Cancel queued message"
                onClick={() => onCancelQueued(queued.queuedId, item.text)}
                className="flex items-center gap-0.5 rounded text-[10px] text-muted-foreground opacity-0 transition-opacity hover:text-foreground focus-visible:opacity-100 group-hover:opacity-100"
              >
                <X className="h-3 w-3" aria-hidden />
                Cancel
              </button>
            ) : null}
          </div>
        ) : null}
      </div>

      {/* Lightbox — reuses the shared Dialog so Esc / click-outside close
          for free (matches project-image-dialog). Near-fullscreen,
          object-contain so no image is cropped. */}
      <Dialog
        open={lightboxImage !== undefined}
        onOpenChange={(open) => {
          if (!open) setLightboxIndex(null);
        }}
      >
        <DialogContent className="max-w-[92vw] border-none bg-transparent p-0 shadow-none sm:max-w-[92vw]">
          <DialogTitle className="sr-only">Attached image</DialogTitle>
          {lightboxImage ? (
            <LightboxImage image={lightboxImage} />
          ) : null}
        </DialogContent>
      </Dialog>
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
  const [errored, setErrored] = useState(false);
  const src = resolveAssetSrc(image.src, null);

  if (errored || !src) {
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
        onError={() => setErrored(true)}
      />
    </button>
  );
}

/** The blown-up image inside the lightbox. Keeps its own error state so
 *  a path that reads at thumbnail size but fails at full size still
 *  degrades to a placeholder rather than a browser broken-image icon. */
function LightboxImage({ image }: { image: UserMessageImage }) {
  const [errored, setErrored] = useState(false);
  const src = resolveAssetSrc(image.src, null);

  if (errored || !src) {
    return (
      <div className="flex h-[60vh] w-full flex-col items-center justify-center gap-2 rounded-lg bg-muted/40 text-muted-foreground">
        <ImageOff className="h-8 w-8 opacity-40" aria-hidden />
        <span className="text-xs">Failed to load image</span>
      </div>
    );
  }

  return (
    <img
      src={src}
      alt={image.mediaType ?? "attached image"}
      className="mx-auto max-h-[88vh] w-auto max-w-full rounded-lg object-contain"
      onError={() => setErrored(true)}
    />
  );
}
