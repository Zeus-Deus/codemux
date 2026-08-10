import { useCallback, useRef, useState, type ReactNode } from "react";
import { ImageIcon, ImageOff, Maximize2 } from "lucide-react";

import {
  ImageLightbox,
  IMAGE_LIGHTBOX_FALLBACK_CLASS,
  IMAGE_LIGHTBOX_MEDIA_CLASS,
} from "@/components/chat/ImageLightbox";
import { resolveAssetSrc } from "@/lib/asset-url";
import { readLocalChatImage } from "@/tauri/commands";

const blobUrlCache = new Map<string, string>();
const blobRequestCache = new Map<string, Promise<string>>();
const BLOB_CACHE_MAX = 64;

function revokeBlobUrl(url: string) {
  if (typeof URL.revokeObjectURL === "function") URL.revokeObjectURL(url);
}

function cacheBlobUrl(path: string, url: string) {
  const previous = blobUrlCache.get(path);
  if (previous && previous !== url) revokeBlobUrl(previous);
  blobUrlCache.delete(path);
  blobUrlCache.set(path, url);

  while (blobUrlCache.size > BLOB_CACHE_MAX) {
    const oldest = blobUrlCache.entries().next().value as
      | [string, string]
      | undefined;
    if (!oldest) break;
    blobUrlCache.delete(oldest[0]);
    revokeBlobUrl(oldest[1]);
  }
}

function requestBlobUrl(path: string): Promise<string> {
  const cached = blobUrlCache.get(path);
  if (cached) return Promise.resolve(cached);

  const pending = blobRequestCache.get(path);
  if (pending) return pending;

  const request = readLocalChatImage(path)
    .then(({ bytes, media_type }) => {
      const url = URL.createObjectURL(new Blob([bytes], { type: media_type }));
      cacheBlobUrl(path, url);
      return url;
    })
    .finally(() => {
      blobRequestCache.delete(path);
    });
  blobRequestCache.set(path, request);
  return request;
}

function basename(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

/**
 * Desktop WebKit can load a converted asset URL directly. Browser/dev-mock
 * clients cannot reach the host filesystem through that protocol, so a failed
 * load falls back to a bounded IPC read and a blob URL.
 */
function useLocalImageSource(path: string) {
  const currentPath = useRef(path);
  currentPath.current = path;
  const [blobState, setBlobState] = useState<{ path: string; src: string | null }>(
    () => ({ path, src: blobUrlCache.get(path) ?? null }),
  );
  const [failedPath, setFailedPath] = useState<string | null>(null);
  const blobSrc =
    blobState.path === path ? blobState.src : (blobUrlCache.get(path) ?? null);
  const failed = failedPath === path;

  const onError = useCallback(() => {
    if (blobSrc) {
      setFailedPath(path);
      return;
    }
    const cached = blobUrlCache.get(path);
    if (cached) {
      setBlobState({ path, src: cached });
      return;
    }
    void requestBlobUrl(path)
      .then((url) => {
        if (currentPath.current === path) {
          setBlobState({ path, src: url });
          setFailedPath(null);
        }
      })
      .catch(() => {
        if (currentPath.current === path) setFailedPath(path);
      });
  }, [blobSrc, path]);

  return { src: blobSrc ?? resolveAssetSrc(path, null), failed, onError };
}

/** Test seam for the module-level blob cache. */
export function resetLocalImageBlobCache() {
  for (const url of blobUrlCache.values()) revokeBlobUrl(url);
  blobUrlCache.clear();
  blobRequestCache.clear();
}

/**
 * Inline visual proof card for agent-authored local screenshot links. It uses
 * only phrasing elements so it remains valid when Markdown places it inside a
 * paragraph or list item; the dialog itself portals outside that structure.
 */
export function MarkdownLocalImage({
  path: rawPath,
  caption: rawCaption,
  children,
}: Record<string, unknown> & { children?: ReactNode }) {
  const path = typeof rawPath === "string" ? rawPath : "";
  const caption =
    typeof rawCaption === "string" && rawCaption.trim()
      ? rawCaption.trim()
      : basename(path);
  const [open, setOpen] = useState(false);
  const thumbnail = useLocalImageSource(path);

  if (!path) return null;

  return (
    <span
      className="not-prose my-3 block max-w-2xl overflow-hidden rounded-xl border border-border/70 bg-card shadow-sm"
      data-chat-local-image
    >
      <button
        type="button"
        className="group block w-full cursor-zoom-in text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
        aria-label={`Open ${caption}`}
        onClick={() => setOpen(true)}
      >
        <span className="flex min-h-28 max-h-72 w-full items-center justify-center overflow-hidden bg-muted/35">
          {thumbnail.failed || !thumbnail.src ? (
            <span className="flex flex-col items-center gap-2 py-10 text-muted-foreground">
              <ImageOff className="size-6 opacity-50" aria-hidden />
              <span className="text-xs">Image unavailable</span>
            </span>
          ) : (
            <img
              src={thumbnail.src}
              alt={caption}
              loading="lazy"
              className="max-h-72 w-full object-contain transition-transform duration-200 group-hover:scale-[1.01]"
              onError={thumbnail.onError}
            />
          )}
        </span>
        <span className="flex min-w-0 items-center gap-2 border-t border-border/60 px-3 py-2 text-xs">
          <ImageIcon className="size-3.5 shrink-0 text-muted-foreground" aria-hidden />
          <span className="min-w-0 flex-1 truncate font-medium text-foreground">
            {children ?? caption}
          </span>
          <span className="flex shrink-0 items-center gap-1 text-[11px] text-muted-foreground">
            <Maximize2 className="size-3" aria-hidden />
            Expand
          </span>
        </span>
      </button>

      <ImageLightbox
        open={open}
        onOpenChange={setOpen}
        title={caption}
        description={`Full-size preview of ${caption}`}
      >
        <ExpandedImage path={path} caption={caption} />
      </ImageLightbox>
    </span>
  );
}

function ExpandedImage({ path, caption }: { path: string; caption: string }) {
  const image = useLocalImageSource(path);

  if (image.failed || !image.src) {
    return (
      <span className={IMAGE_LIGHTBOX_FALLBACK_CLASS}>
        <ImageOff className="size-8 opacity-50" aria-hidden />
        <span className="text-sm">Failed to load {caption}</span>
      </span>
    );
  }

  return (
    <img
      src={image.src}
      alt={caption}
      className={IMAGE_LIGHTBOX_MEDIA_CLASS}
      onError={image.onError}
    />
  );
}
