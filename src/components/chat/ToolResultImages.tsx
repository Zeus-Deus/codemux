import { useState } from "react";
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
 * click. Sources here are already renderable (`data:` or http(s) URLs
 * built by `extractToolResultImages`), so — unlike the user-attachment
 * path — there is no asset-protocol / IPC-readback fallback to do.
 */
export function ToolResultImages({ images }: { images: ToolResultImage[] }) {
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
        />
      ))}

      <ImageLightbox
        open={lightboxImage !== undefined}
        onOpenChange={(open) => {
          if (!open) setLightboxIndex(null);
        }}
        title="Tool result image"
      >
        {lightboxImage ? <LightboxImage image={lightboxImage} /> : null}
      </ImageLightbox>
    </div>
  );
}

/** A single clickable thumbnail with its own broken-image fallback so
 *  one bad source can't blank the rest of the row. */
function ToolResultThumbnail({
  image,
  onOpen,
}: {
  image: ToolResultImage;
  onOpen: () => void;
}) {
  const [failed, setFailed] = useState(false);

  if (failed) {
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
      aria-label="Open image"
      className="cursor-pointer overflow-hidden rounded-lg border border-border/60 transition-opacity hover:opacity-90 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
    >
      <img
        src={image.src}
        alt={image.mediaType ?? "tool result image"}
        className="max-h-40 max-w-[240px] object-contain"
        onError={() => setFailed(true)}
      />
    </button>
  );
}

/** The blown-up image inside the lightbox. Keeps its own error state so
 *  a source that fails at full size degrades to a placeholder rather than
 *  a browser broken-image icon. */
function LightboxImage({ image }: { image: ToolResultImage }) {
  const [failed, setFailed] = useState(false);

  if (failed) {
    return (
      <div className={IMAGE_LIGHTBOX_FALLBACK_CLASS}>
        <ImageOff className="h-8 w-8 opacity-40" aria-hidden />
        <span className="text-xs">Failed to load image</span>
      </div>
    );
  }

  return (
    <img
      src={image.src}
      alt={image.mediaType ?? "tool result image"}
      className={IMAGE_LIGHTBOX_MEDIA_CLASS}
      onError={() => setFailed(true)}
    />
  );
}
