import { useEffect } from "react";
import { createPortal } from "react-dom";
import { ExternalLink, X } from "lucide-react";

import { openExternalUrl } from "@/lib/open-url";

export interface ImageLightboxProps {
  /** Already resolved — the overlay does no URL work of its own. */
  src: string;
  alt: string;
  /** The URL as written in the source, for the browser affordance. When
   *  the image is a local asset there is nothing useful to open. */
  href?: string | null;
  onClose: () => void;
}

/**
 * An embedded image, full size.
 *
 * PR descriptions in this app are usually a sentence and two
 * screenshots, and a screenshot you can't enlarge is a screenshot you
 * have to go to the browser to read — which is the one thing this
 * surface exists to avoid.
 *
 * Deliberately not the dialog primitive: this has no title, no padding
 * and no chrome, and everything it would inherit from `Dialog` would
 * have to be overridden back off. Escape and a backdrop click are the
 * whole interaction.
 */
export function ImageLightbox({ src, alt, href, onClose }: ImageLightboxProps) {
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      // Captured and stopped here: the Pull Requests page closes itself
      // on Escape too, and closing the image should not also close the
      // page you were reading it on.
      event.stopPropagation();
      event.preventDefault();
      onClose();
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [onClose]);

  return createPortal(
    <div
      data-testid="image-lightbox"
      role="dialog"
      aria-modal="true"
      aria-label={alt || "Image"}
      onClick={onClose}
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/80 p-6"
    >
      <img
        src={src}
        alt={alt}
        data-testid="image-lightbox-image"
        // The image itself is not a dismiss target — you click it to
        // look closer, not to leave.
        onClick={(event) => event.stopPropagation()}
        className="max-h-[92vh] max-w-[95vw] object-contain"
      />

      <div
        className="absolute right-4 top-4 flex items-center gap-1.5"
        onClick={(event) => event.stopPropagation()}
      >
        {href && /^https?:\/\//i.test(href) && (
          <button
            type="button"
            data-testid="image-lightbox-open-external"
            onClick={() => void openExternalUrl(href)}
            className="inline-flex h-[30px] items-center gap-1.5 rounded-md border-0 bg-white/10 px-3 text-[12px] font-medium text-white transition-colors hover:bg-white/20"
          >
            <ExternalLink className="size-3.5" />
            Open in browser
          </button>
        )}
        <button
          type="button"
          aria-label="Close"
          data-testid="image-lightbox-close"
          onClick={onClose}
          className="inline-flex size-[30px] items-center justify-center rounded-md border-0 bg-white/10 text-white transition-colors hover:bg-white/20"
        >
          <X className="size-4" />
        </button>
      </div>
    </div>,
    document.body,
  );
}
