import { memo, useCallback, useMemo, useRef, useState, type ComponentProps } from "react";
import { convertFileSrc, isTauri } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ExternalLink } from "lucide-react";
import ReactMarkdown, { type Components } from "react-markdown";
import rehypeRaw from "rehype-raw";
import rehypeSanitize, { defaultSchema, type Options as SanitizeSchema } from "rehype-sanitize";
import remarkGfm from "remark-gfm";
import type { PluggableList } from "unified";

import { dirname, isAbsoluteFsPath, isRemoteOrDataUrl, joinPath, resolveAssetSrc } from "@/lib/asset-url";
import { openExternalUrl } from "@/lib/open-url";
import { toast } from "@/lib/toast";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { ImageLightbox } from "./image-lightbox";

/**
 * react-markdown's default `urlTransform` would strip the `data:` and
 * resolved `asset:` URLs we depend on, so we override it. But because
 * `rehypeRaw` lets a document's own raw HTML through, the transform is
 * still the right place to neutralise dangerous link schemes
 * (`javascript:`, `vbscript:`) that could otherwise ride in on a raw
 * `<a href>`. Everything else — http(s), data, blob, asset, tauri,
 * relative and absolute filesystem paths — passes through untouched so
 * local image sources can be resolved. The image renderer below keeps
 * remote HTTP(S) sources out of live `<img>` elements.
 */
const DANGEROUS_URL = /^\s*(javascript|vbscript):/i;

function safeUrlTransform(url: string): string {
  return DANGEROUS_URL.test(url) ? "" : url;
}

function webUrl(value: string | undefined): URL | null {
  if (!value || !/^https?:\/\//i.test(value)) return null;
  try {
    return new URL(value);
  } catch {
    return null;
  }
}

/** OS-facing asset URLs also use HTTP(S) on some Tauri platforms. */
function externalLinkUrl(value: string | undefined): string | undefined {
  const url = webUrl(value);
  if (!url || url.origin === window.location.origin) return undefined;
  const host = url.hostname.replace(/\.+$/, "");
  if (host === "asset.localhost" || host === "tauri.localhost") return undefined;
  return url.href;
}

const FORGE_SCHEMA: SanitizeSchema = {
  ...defaultSchema,
  tagNames: defaultSchema.tagNames?.filter((tag) => tag !== "picture" && tag !== "source"),
};

const ID_PREFIX = defaultSchema.clobberPrefix ?? "";

const DRIVE_LETTERS = [..."abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ"];

/**
 * The allowlist for a document from the user's own disk. It is GitHub's
 * allowlist plus the image sources a local file legitimately uses:
 * `data:` and `blob:` images, already-converted asset URLs, and Windows
 * drive paths (`C:\x.png`), which the sanitizer would otherwise read as
 * an unknown `c:` scheme and drop.
 *
 * A local file is still not trusted to run code: a cloned repository's
 * README is somebody else's HTML, and the webview it renders into can
 * call every backend command.
 */
const LOCAL_FILE_SCHEMA: SanitizeSchema = {
  ...defaultSchema,
  // Raw <source srcset> bypasses the img component below and can fetch a
  // remote URL as soon as a local README is opened.
  tagNames: defaultSchema.tagNames?.filter((tag) => tag !== "picture" && tag !== "source"),
  protocols: {
    ...defaultSchema.protocols,
    src: [
      ...(defaultSchema.protocols?.src ?? []),
      "data",
      "blob",
      "asset",
      "tauri",
      ...DRIVE_LETTERS,
    ],
  },
};

function isRemoteImageSrc(src: string): boolean {
  const value = src.trim();
  // Include malformed and protocol-relative web references. A local
  // document must not make a network request while it is displayed.
  return /^https?:/i.test(value) || /^\/\/[^/]/.test(value);
}

function isLocalAssetImageSrc(src: string): boolean {
  // Only the webview whose registered asset endpoint uses this HTTP origin
  // may preview it. Other native platforms and hosted browsers would make a
  // normal network request to this address.
  if (!isTauri() || !/^http:\/\/asset\.localhost(?=\/|[?#]|$)/i.test(src)) return false;
  try {
    return webUrl(convertFileSrc("/__codemux_asset_probe__"))?.origin === "http://asset.localhost";
  } catch {
    return false;
  }
}

/** Inspect URL-encoded and literal separators before the asset handler can
 * resolve a document-supplied path into a network share or device namespace. */
function isNetworkOrDeviceImagePath(value: string): boolean {
  let path = value.trimStart();
  for (let depth = 0; depth < 6; depth += 1) {
    const slashes = path.replace(/\\/g, "/");
    if (slashes.startsWith("//") || slashes.startsWith("/??/")) return true;
    let decoded: string;
    try {
      decoded = decodeURIComponent(path);
    } catch {
      return false;
    }
    if (decoded === path) return false;
    path = decoded;
  }
  return true;
}

/** The path portion is kept raw until the network-prefix check above, since
 * URL parsing may normalize leading separators before the native handler. */
function nativeAssetPath(src: string): string | null {
  const match = /^(?:(?:asset|tauri):\/\/localhost|http:\/\/asset\.localhost)(\/[^?#]*)/i.exec(src);
  return match?.[1] ?? null;
}

function InertImageReference({ src, alt }: { src: string; alt: string }) {
  return <span title={src}>{alt || "Image reference"}</span>;
}

/** A document path can traverse a mapped drive, mount, junction, or symlink.
 * Do not hand it to the asset handler until the reader asks to load it. */
function DeferredLocalImage({
  src,
  alt,
  displayPath,
  originalHref,
  onZoom,
  ...rest
}: ComponentProps<"img"> & {
  src: string;
  displayPath: string;
  originalHref: string | null;
  onZoom: (src: string, alt: string, href: string | null) => void;
}) {
  const [loaded, setLoaded] = useState(false);
  if (!loaded) {
    return (
      <button
        type="button"
        data-testid="markdown-image-load"
        className="my-2 flex max-w-full flex-col items-start gap-1 break-all text-left text-primary underline"
        onClick={(event) => {
          event.preventDefault();
          event.stopPropagation();
          setLoaded(true);
        }}
        onAuxClick={(event) => {
          event.preventDefault();
          event.stopPropagation();
        }}
      >
        <span>{alt ? `Load local image: ${alt}` : "Load local image"}</span>
        <span className="font-mono text-xs text-muted-foreground">{displayPath}</span>
      </button>
    );
  }
  return (
    <img
      {...rest}
      src={src}
      alt={alt}
      loading="lazy"
      data-testid="markdown-image"
      className="my-2 max-h-[360px] w-auto max-w-full cursor-zoom-in rounded-md border border-border/60 object-contain"
      onClick={(event) => {
        event.preventDefault();
        event.stopPropagation();
        onZoom(src, alt ?? "", originalHref);
      }}
    />
  );
}

function RemoteImageReference({ src, alt }: { src: string; alt: string }) {
  const [confirmationOpen, setConfirmationOpen] = useState(false);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const openConfirmedImage = () => {
    void openUrl(src).catch((error) => {
      toast.error("Could not open the image", {
        description: error instanceof Error ? error.message : String(error),
      });
    });
  };

  return (
    <AlertDialog open={confirmationOpen} onOpenChange={setConfirmationOpen}>
      <button
        ref={triggerRef}
        type="button"
        className="my-2 inline-flex max-w-full items-center gap-1.5 break-all text-left text-primary underline"
        onClick={(event) => {
          event.preventDefault();
          event.stopPropagation();
          setConfirmationOpen(true);
        }}
        onAuxClick={(event) => {
          event.preventDefault();
          event.stopPropagation();
        }}
      >
        <ExternalLink className="size-4 shrink-0" aria-hidden="true" />
        {alt ? `Open remote image: ${alt}` : "Open remote image"}
      </button>
      <AlertDialogContent
        onCloseAutoFocus={(event) => {
          event.preventDefault();
          triggerRef.current?.focus();
        }}
        onClick={(event) => event.stopPropagation()}
        onAuxClick={(event) => event.stopPropagation()}
        overlayProps={{
          onClick: (event) => event.stopPropagation(),
          onAuxClick: (event) => event.stopPropagation(),
        }}
      >
        <AlertDialogHeader>
          <AlertDialogTitle>Open remote image?</AlertDialogTitle>
          <AlertDialogDescription>
            This address will open in your system browser.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <div className="select-text break-all rounded-lg bg-muted px-3 py-2 font-mono text-label text-muted-foreground">
          {src}
        </div>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction onClick={openConfirmedImage}>Open image</AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

/**
 * Scroll to the target of an in-page link, within the same rendered
 * document. The sanitizer's prefix is tried first: a raw
 * `<a name="setup">` renders as `user-content-setup`, and a footnote,
 * which is generated already prefixed, ends up prefixed twice.
 */
function scrollToFragment(link: Element, fragment: string): void {
  let name = fragment;
  try {
    name = decodeURIComponent(fragment);
  } catch {
    // A literal '%' rather than a URL escape.
  }
  if (!name) return;
  const root = link.closest(".markdown-rendered");
  if (!root) return;
  const anchors = Array.from(root.querySelectorAll("[id], [name]"));
  for (const candidate of [`${ID_PREFIX}${name}`, name]) {
    const target = anchors.find(
      (el) => el.id === candidate || el.getAttribute("name") === candidate,
    );
    if (target) {
      target.scrollIntoView();
      return;
    }
  }
}

interface Props {
  content: string;
  /**
   * Absolute path of the markdown file being rendered. Used to resolve
   * relative image paths (e.g. `./screenshot.png`) against the file's
   * own directory. Optional — when omitted, absolute paths and embedded
   * data/asset images can still render; remote images become links.
   */
  filePath?: string | null;
  /**
   * Drop the page chrome — the scroll container, the measure cap and the
   * document padding — so the rendered markdown can sit inside a dense
   * surface that owns its own scrolling and spacing (the PR description
   * in the Review panel). The markdown itself renders identically; only
   * the wrapper differs.
   */
  inline?: boolean;
  /**
   * The content came from the forge rather than the local disk — a pull
   * request description or a review comment anyone can post. Raw HTML
   * is always sanitized. Forge images render their descriptions so
   * opening a review cannot make image requests to a contributor's URLs.
   */
  untrusted?: boolean;
}

/**
 * Internal impl. Wrapped in `React.memo` below so the AST re-parse only
 * runs when `content` or `filePath` actually change. Without the memo,
 * every parent re-render of `EditorPane` (which itself re-renders
 * whenever the global `app-state-changed` snapshot ticks — e.g. on every
 * agent token, git poll, or hook event) would force `react-markdown` to
 * walk the entire content string and rebuild the rendered tree. On
 * multi-KB markdown files that is the dominant cause of choppy scrolling
 * while an agent is streaming. The internal `useMemo` for plugins and
 * components is necessary but not sufficient — they're consumed by an
 * unmemoized wrapper, so they only stabilise refs feeding into the
 * non-skipped render. Wrapping the wrapper itself is what skips the
 * `<ReactMarkdown>` call entirely.
 */
function MarkdownRenderedImpl({
  content,
  filePath,
  inline = false,
  untrusted = false,
}: Props) {
  /**
   * The image being read full size, if any.
   *
   * Held here rather than per-`img` so only one overlay can ever be
   * open, and so the state survives the memoized `components` object.
   */
  const [zoomed, setZoomed] = useState<{
    src: string;
    alt: string;
    href: string | null;
  } | null>(null);
  const closeZoom = useCallback(() => setZoomed(null), []);

  const remarkPlugins = useMemo(() => [remarkGfm], []);
  // `rehypeRaw` re-parses the raw HTML that GFM leaves as opaque nodes
  // (e.g. a README's `<div align="center">` wrapper and `<img>` logo) so
  // it renders as real elements instead of escaped text. Raw `<img>`
  // tags become proper `img` nodes, so the `img` override below still
  // routes their `src` through `resolveAssetSrc`. The HTML is sanitized
  // after it is parsed, never before, and on every surface: an
  // `<iframe srcdoc>` that survived would run script in this webview.
  const rehypePlugins = useMemo<PluggableList>(
    () => [rehypeRaw, [rehypeSanitize, untrusted ? FORGE_SCHEMA : LOCAL_FILE_SCHEMA]],
    [untrusted],
  );

  const components = useMemo<Components>(
    () => ({
      /**
       * An embedded image, constrained and inspectable.
       *
       * A bare `<img>` renders a 2000px-wide screenshot at 2000px and
       * pushes everything below it off the surface, so the inline
       * rendering is capped — but a capped screenshot is unreadable,
       * which is why clicking one opens it full size rather than being
       * the end of the story. Nothing is re-encoded on the way, so an
       * animated GIF keeps animating in both places.
       */
      img: ({ src, alt, ...rest }) => {
        const label = alt ?? "";
        if (untrusted) return label || "Image";
        let imagePath = src;
        if (src && !isRemoteOrDataUrl(src)) {
          // Markdown destinations are URLs, but the asset protocol expects a
          // filesystem path and encodes it again. Decode only the destination,
          // leaving the base file's literal directory names intact.
          try {
            imagePath = decodeURIComponent(src);
          } catch {
            // Raw HTML can contain a literal '%' instead of a URL escape.
          }
        }
        // Decode before deciding whether the source is remote: an encoded
        // `https%3A%2F%2F...` destination would otherwise become a live URL
        // in resolveAssetSrc after passing the initial check.
        if (imagePath && isRemoteImageSrc(imagePath) && !isLocalAssetImageSrc(imagePath)) {
          // A protocol-relative reference must open as an absolute HTTPS
          // address. Invalid web URLs stay inert instead of reaching <img>.
          const url = webUrl(imagePath.startsWith("//") ? `https:${imagePath}` : imagePath);
          return url ? <RemoteImageReference src={url.href} alt={label} /> : label || "Image";
        }
        if (!imagePath) return label || "Image";

        const zoom = (resolved: string, alt: string, href: string | null) => {
          setZoomed({ src: resolved, alt, href });
        };
        // Embedded data and existing Blob URLs do not ask the filesystem or
        // network to resolve a document-supplied path.
        if (/^(?:data|blob):/i.test(imagePath)) {
          return (
            <img
              {...rest}
              src={imagePath}
              alt={label}
              loading="lazy"
              data-testid="markdown-image"
              className="my-2 max-h-[360px] w-auto max-w-full cursor-zoom-in rounded-md border border-border/60 object-contain"
              onClick={(event) => {
                event.preventDefault();
                event.stopPropagation();
                zoom(imagePath, label, src ?? null);
              }}
            />
          );
        }

        // An asset URL is still a filesystem path. Its registered origin is
        // not proof that the decoded target is local or that a mapped drive
        // cannot reach a share. Keep it out of <img> until a deliberate click.
        const assetPath = nativeAssetPath(imagePath);
        if (assetPath !== null) {
          // Tauri's asset handler strips the URL's first separator before
          // percent-decoding the filesystem path. Inspect that same path:
          // /%2Ftmp is a POSIX absolute file, but /%2F%2Fhost is a share.
          if (isNetworkOrDeviceImagePath(assetPath.slice(1))) {
            return <InertImageReference src={imagePath} alt={label} />;
          }
          if (/^(?:asset|tauri):/i.test(imagePath)) {
            let registered = false;
            try {
              const scheme = imagePath.slice(0, imagePath.indexOf(":")).toLowerCase();
              registered = isTauri() && convertFileSrc("/__codemux_asset_probe__")
                .toLowerCase().startsWith(`${scheme}://localhost/`);
            } catch {
              // No registered asset endpoint in this webview.
            }
            if (!registered) return <InertImageReference src={imagePath} alt={label} />;
          } else if (!isLocalAssetImageSrc(imagePath)) {
            return <InertImageReference src={imagePath} alt={label} />;
          }
          return (
            <DeferredLocalImage
              key={imagePath}
              {...rest}
              src={imagePath}
              alt={label}
              displayPath={imagePath}
              originalHref={src ?? null}
              onZoom={zoom}
            />
          );
        }
        if (/^[a-z][a-z\d+.-]*:/i.test(imagePath) && !/^[a-z]:[\\/]/i.test(imagePath)) {
          return <InertImageReference src={imagePath} alt={label} />;
        }
        // A relative reference without a file location could become a browser
        // request to the app's own origin; it has no stable local target.
        const localPath = isAbsoluteFsPath(imagePath)
          ? imagePath
          : filePath ? joinPath(dirname(filePath), imagePath) : null;
        if (!localPath || !isAbsoluteFsPath(localPath)
            || isNetworkOrDeviceImagePath(imagePath)
            || isNetworkOrDeviceImagePath(localPath)) {
          return <InertImageReference src={imagePath} alt={label} />;
        }
        const resolved = resolveAssetSrc(localPath, null);
        if (!resolved) return <InertImageReference src={imagePath} alt={label} />;
        return (
          <DeferredLocalImage
            key={resolved}
            {...rest}
            src={resolved}
            alt={label}
            displayPath={localPath}
            originalHref={src ?? null}
            onZoom={zoom}
          />
        );
      },
      /**
       * Links go somewhere deliberate rather than nowhere.
       *
       * Without this override an `<a href>` here could navigate the
       * webview itself. Keep only in-document fragments and absolute
       * external web URLs live, then take over their clicks.
       *
       * This is the second of the two shared markdown surfaces. It
       * renders the PR description in the Review panel, which is exactly
       * where "see #482" tends to be written.
       */
      a: ({ href, children, ...rest }) => {
        const fragment = href?.startsWith("#") ? href.slice(1) : null;
        const external = externalLinkUrl(href);
        return (
          <a
            {...rest}
            href={fragment === null ? external : href}
            target={external ? "_blank" : undefined}
            rel="noopener noreferrer"
            onClick={(event) => {
              event.preventDefault();
              if (fragment !== null) {
                // Sanitizing prefixes id and name, so resolve the link
                // within this rendered document.
                scrollToFragment(event.currentTarget, fragment);
              } else if (external) {
                void openExternalUrl(external, { event });
              }
            }}
            onAuxClick={(event) => {
              // A middle click does not fire onClick, so suppress native
              // navigation for every destination here as well.
              if (event.button !== 1) return;
              event.preventDefault();
              if (external) void openExternalUrl(external, { event });
            }}
          >
            {children}
          </a>
        );
      },
    }),
    [filePath, untrusted],
  );

  const body = (
    <div
      className={
        inline ? "markdown-rendered markdown-rendered-inline" : "markdown-rendered max-w-3xl px-8 py-6"
      }
    >
      <ReactMarkdown
        remarkPlugins={remarkPlugins}
        rehypePlugins={rehypePlugins}
        components={components}
        // react-markdown's default urlTransform strips data: URIs and
        // other non-http schemes for safety. The content here is the
        // user's own local markdown — we resolve filesystem paths via
        // `resolveAssetSrc` and want data: image URIs to render. Our
        // transform keeps those but still blocks executable schemes.
        urlTransform={safeUrlTransform}
      >
        {content}
      </ReactMarkdown>
    </div>
  );

  const overlay = zoomed ? (
    <ImageLightbox
      src={zoomed.src}
      alt={zoomed.alt}
      href={zoomed.href}
      onClose={closeZoom}
    />
  ) : null;

  if (inline) {
    return (
      <>
        {body}
        {overlay}
      </>
    );
  }

  return (
    <div className="flex-1 min-h-0 overflow-auto bg-[var(--background)]">
      {body}
      {overlay}
    </div>
  );
}

/**
 * Default `React.memo` shallow-compare is sufficient: `content` is a
 * primitive string and `filePath` is a primitive string-or-null. Both
 * come from `EditorPane`'s `useEditorStore` slice, which only mutates
 * on local edit / save — it does not churn during agent streaming or
 * git polls. So when the parent re-renders due to backend
 * `app-state-changed` ticks, both props will be reference-equal and the
 * memo skips the inner `<ReactMarkdown>` call entirely.
 */
export const MarkdownRendered = memo(MarkdownRenderedImpl);
