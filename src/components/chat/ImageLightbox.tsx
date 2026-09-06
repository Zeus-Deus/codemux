import type { ReactNode } from "react";
import { X } from "lucide-react";
import { useDismissOnDisconnect } from "./use-dismiss-on-disconnect";

import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog";

/**
 * Shared image viewer for every image source in chat. The dialog shrink-wraps
 * the rendered media instead of occupying most of the viewport, so every
 * visible area around the image remains a clickable dismiss target.
 */
export function ImageLightbox({
  open,
  onOpenChange,
  title,
  description = `Expanded preview of ${title}`,
  children,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description?: string;
  children: ReactNode;
}) {
  useDismissOnDisconnect(open, onOpenChange);
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        showCloseButton={false}
        overlayClassName="bg-black/75 supports-backdrop-filter:backdrop-blur-sm"
        className="w-fit max-w-[calc(100vw-2rem)] gap-0 border-0 bg-transparent p-0 ring-0 shadow-none sm:max-w-[calc(100vw-2rem)]"
        data-image-lightbox
      >
        <DialogTitle className="sr-only">{title}</DialogTitle>
        <DialogDescription className="sr-only">
          {description}
        </DialogDescription>

        <div className="relative w-fit max-w-full">
          <DialogClose asChild>
            <button
              type="button"
              aria-label="Close expanded image"
              title="Click the image to close"
              className="block w-fit max-w-full cursor-zoom-out overflow-hidden rounded-xl bg-[#090909] shadow-2xl ring-1 ring-white/15 outline-none focus-visible:ring-2 focus-visible:ring-white/70"
            >
              {children}
            </button>
          </DialogClose>

          <DialogClose asChild>
            <button
              type="button"
              aria-label="Close image preview"
              title="Close image preview (Esc)"
              className="absolute top-2.5 right-2.5 z-10 flex size-9 items-center justify-center rounded-full bg-black/70 text-white shadow-lg ring-1 ring-white/25 backdrop-blur-md transition-[background-color,transform] hover:bg-black/90 active:scale-95 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white"
            >
              <X className="size-4.5" aria-hidden />
            </button>
          </DialogClose>
        </div>
      </DialogContent>
    </Dialog>
  );
}

/** Viewport-safe sizing shared by all full-size chat images. */
export const IMAGE_LIGHTBOX_MEDIA_CLASS =
  "block h-auto w-auto max-h-[calc(100vh-2rem)] max-w-[calc(100vw-2rem)] object-contain select-none";

/** Stable dimensions for the error state when there is no image to size to. */
export const IMAGE_LIGHTBOX_FALLBACK_CLASS =
  "flex h-[min(60vh,28rem)] w-[calc(100vw-2rem)] max-w-xl flex-col items-center justify-center gap-2 bg-muted text-muted-foreground";
