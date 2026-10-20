import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { ImageIcon, ImageOff, Maximize2 } from "lucide-react";

import {
  ImageLightbox,
  IMAGE_LIGHTBOX_FALLBACK_CLASS,
  IMAGE_LIGHTBOX_MEDIA_CLASS,
} from "@/components/chat/ImageLightbox";
import { resolveAssetSrc } from "@/lib/asset-url";
import { readChatImage, readLocalChatImage } from "@/tauri/commands";
import {
  onAgentChatReplyImagesKept,
  type RetainedReplyImage,
} from "@/tauri/events";

type ImageReader = typeof readLocalChatImage;

/** One place an image may load from, with the IPC reader that serves it
 *  where the asset protocol cannot. */
interface ImageCandidate {
  path: string;
  read: ImageReader;
  /** The reply's kept copy, which may still be landing. */
  kept?: boolean;
}

/** Cards waiting for a kept copy to land, by the copy's path. */
const keptCopyWaiters = new Map<string, Set<() => void>>();
let keptCopyListener: Promise<unknown> | null = null;

/** Call `onLanded` when the backend records the copy at `path`. One
 *  app-wide listener serves every waiting card. */
function whenKeptCopyLands(path: string, onLanded: () => void): () => void {
  keptCopyListener ??= onAgentChatReplyImagesKept((paths) => {
    for (const landed of paths) {
      for (const notify of keptCopyWaiters.get(landed) ?? []) notify();
    }
  }).catch(() => null);
  const waiters = keptCopyWaiters.get(path) ?? new Set();
  waiters.add(onLanded);
  keptCopyWaiters.set(path, waiters);
  return () => {
    waiters.delete(onLanded);
    if (!waiters.size) keptCopyWaiters.delete(path);
  };
}

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

function requestBlobUrl({ path, read }: ImageCandidate): Promise<string> {
  const cached = blobUrlCache.get(path);
  if (cached) return Promise.resolve(cached);

  const pending = blobRequestCache.get(path);
  if (pending) return pending;

  const request = read(path)
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

/** Conversation-owned copies of a reply's linked images, by the path the
 *  reply wrote. */
const RetainedImagesContext = createContext<ReadonlyMap<string, string> | null>(
  null,
);

/** Let the local-image cards inside one reply load its kept copies. */
export function RetainedImagesProvider({
  images,
  children,
}: {
  images: readonly RetainedReplyImage[] | undefined;
  children: ReactNode;
}) {
  const retained = useMemo(
    () =>
      images?.length
        ? new Map(images.map((image) => [image.source, image.path]))
        : null,
    [images],
  );
  return (
    <RetainedImagesContext.Provider value={retained}>
      {children}
    </RetainedImagesContext.Provider>
  );
}

/** Where `path` may load from: the reply's kept copy first (it outlives the
 *  original, but may still be landing), then the path the reply wrote. */
function useImageCandidates(path: string): readonly ImageCandidate[] {
  const retained = useContext(RetainedImagesContext)?.get(path);
  return useMemo(
    () => [
      ...(retained ? [{ path: retained, read: readChatImage, kept: true }] : []),
      { path, read: readLocalChatImage },
    ],
    [path, retained],
  );
}

interface SourceState {
  key: string;
  index: number;
  blob: string | null;
  failed: boolean;
}

function firstSource(
  key: string,
  candidates: readonly ImageCandidate[],
): SourceState {
  const first = candidates[0];
  return {
    key,
    index: 0,
    blob: first ? (blobUrlCache.get(first.path) ?? null) : null,
    failed: !first,
  };
}

/**
 * Desktop WebKit can load a converted asset URL directly. Browser/dev-mock
 * clients cannot reach the host filesystem through that protocol, so a failed
 * load falls back to a bounded IPC read and a blob URL. Each candidate gets
 * both attempts before the next one is tried.
 */
function useLocalImageSource(candidates: readonly ImageCandidate[]) {
  const key = candidates.map((candidate) => candidate.path).join("\0");
  const currentKey = useRef(key);
  currentKey.current = key;
  const [stored, setStored] = useState(() => firstSource(key, candidates));
  const state = stored.key === key ? stored : firstSource(key, candidates);
  const candidate = candidates[state.index];

  const onError = useCallback(() => {
    if (!candidate || state.failed) return;
    const advance = () => {
      const index = state.index + 1;
      const next = candidates[index];
      setStored(
        next
          ? {
              ...state,
              index,
              blob: blobUrlCache.get(next.path) ?? null,
            }
          : { ...state, failed: true },
      );
    };
    if (state.blob) {
      advance();
      return;
    }
    void requestBlobUrl(candidate)
      .then((url) => {
        if (currentKey.current === key) setStored({ ...state, blob: url });
      })
      .catch(() => {
        if (currentKey.current === key) advance();
      });
  }, [candidate, candidates, key, state]);

  // Every source failed, but the kept copy may only have been landing (its
  // background copy can hold the bytes after the original is gone): load it
  // when the backend records it. The immediate read covers a copy that
  // landed before this card started waiting.
  useEffect(() => {
    const kept = candidates[0];
    if (!state.failed || !kept?.kept) return;
    const load = () => {
      requestBlobUrl(kept)
        .then((url) => {
          if (currentKey.current === key) {
            setStored({ key, index: 0, blob: url, failed: false });
          }
        })
        .catch(() => {});
    };
    const stop = whenKeptCopyLands(kept.path, load);
    load();
    return stop;
  }, [candidates, key, state.failed]);

  return {
    src:
      state.failed || !candidate
        ? null
        : (state.blob ?? resolveAssetSrc(candidate.path, null)),
    failed: state.failed,
    onError,
  };
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
  const candidates = useImageCandidates(path);
  const thumbnail = useLocalImageSource(candidates);

  if (!path) return null;

  return (
    <span
      className="not-prose my-3 block max-w-2xl overflow-hidden rounded-lg border border-hairline-strong bg-card shadow-sm"
      data-chat-local-image
    >
      <button
        type="button"
        className="group block w-full cursor-zoom-in text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/60"
        aria-label={`Open ${caption}`}
        onClick={() => setOpen(true)}
      >
        <span className="flex min-h-28 max-h-72 w-full items-center justify-center overflow-hidden bg-surface-1">
          {thumbnail.failed || !thumbnail.src ? (
            <span className="flex flex-col items-center gap-2 py-10 text-muted-foreground">
              <ImageOff className="size-6 opacity-50" aria-hidden />
              <span className="text-label">Image unavailable</span>
            </span>
          ) : (
            <img
              src={thumbnail.src}
              alt={caption}
              loading="lazy"
              className="max-h-72 w-full object-contain transition-transform duration-150 group-hover:scale-[1.01]"
              onError={thumbnail.onError}
            />
          )}
        </span>
        <span className="flex min-w-0 items-center gap-2 border-t border-hairline px-3 py-2 text-label">
          <ImageIcon className="size-3.5 shrink-0 text-muted-foreground" aria-hidden />
          <span className="min-w-0 flex-1 truncate font-medium text-foreground">
            {children ?? caption}
          </span>
          <span className="flex shrink-0 items-center gap-1 text-label text-muted-foreground">
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
        <ExpandedImage candidates={candidates} caption={caption} />
      </ImageLightbox>
    </span>
  );
}

function ExpandedImage({
  candidates,
  caption,
}: {
  candidates: readonly ImageCandidate[];
  caption: string;
}) {
  const image = useLocalImageSource(candidates);

  if (image.failed || !image.src) {
    return (
      <span className={IMAGE_LIGHTBOX_FALLBACK_CLASS}>
        <ImageOff className="size-8 opacity-50" aria-hidden />
        <span className="text-body">Failed to load {caption}</span>
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
