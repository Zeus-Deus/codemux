import { memo, useCallback, useMemo, useState } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import rehypeRaw from "rehype-raw";
import rehypeSanitize, { defaultSchema, type Options as SanitizeSchema } from "rehype-sanitize";
import remarkGfm from "remark-gfm";
import type { PluggableList } from "unified";

import { isAbsoluteFsPath, isRemoteOrDataUrl, resolveAssetSrc } from "@/lib/asset-url";
import { openExternalUrl } from "@/lib/open-url";
import { ImageLightbox } from "./image-lightbox";

/**
 * react-markdown's default `urlTransform` would strip the `data:` and
 * resolved `asset:` URLs we depend on, so we override it. But because
 * `rehypeRaw` lets a document's own raw HTML through, the transform is
 * still the right place to neutralise dangerous link schemes
 * (`javascript:`, `vbscript:`) that could otherwise ride in on a raw
 * `<a href>`. Everything else — http(s), data, blob, asset, tauri,
 * relative and absolute filesystem paths — passes through untouched so
 * `resolveAssetSrc` can do its job on image `src`s.
 */
const DANGEROUS_URL = /^\s*(javascript|vbscript):/i;

function safeUrlTransform(url: string): string {
  return DANGEROUS_URL.test(url) ? "" : url;
}

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
   * own directory. Optional — when omitted, only absolute paths and
   * remote URLs render.
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
   * is always sanitized (bots lean on `<details>` and `<img>`, so it
   * still renders, but a `<style>`, `<iframe>` or `<base>` cannot reach
   * the app's DOM); this additionally drops the local-only image
   * sources a remote author has no business using.
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
    () => [rehypeRaw, untrusted ? rehypeSanitize : [rehypeSanitize, LOCAL_FILE_SCHEMA]],
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
        let imagePath = src;
        if (src && !isRemoteOrDataUrl(src) && (filePath || isAbsoluteFsPath(src))) {
          // Markdown destinations are URLs, but the asset protocol expects a
          // filesystem path and encodes it again. Decode only the destination,
          // leaving the base file's literal directory names intact.
          try {
            imagePath = decodeURIComponent(src);
          } catch {
            // Raw HTML can contain a literal '%' instead of a URL escape.
          }
        }
        const resolved = resolveAssetSrc(imagePath, filePath);
        const label = alt ?? "";
        return (
          <img
            {...rest}
            src={resolved}
            alt={label}
            loading="lazy"
            data-testid="markdown-image"
            className="my-2 max-h-[360px] w-auto max-w-full cursor-zoom-in rounded-md border border-border/60 object-contain"
            onClick={() => {
              if (!resolved) return;
              setZoomed({
                src: resolved,
                alt: label,
                href: typeof src === "string" ? src : null,
              });
            }}
          />
        );
      },
      /**
       * Links go somewhere deliberate rather than nowhere.
       *
       * Without this override an `<a href>` here would try to navigate
       * the webview itself — unloading the single-page app, or being
       * dropped silently. So the click is taken over: a pull request in
       * an open project opens the Pull Requests page, and everything
       * else (and any shift-click) goes to the system browser.
       *
       * This is the second of the two shared markdown surfaces. It
       * renders the PR description in the Review panel, which is exactly
       * where "see #482" tends to be written.
       */
      a: ({ href, children, ...rest }) => (
        <a
          {...rest}
          href={href}
          rel="noopener noreferrer"
          onClick={(event) => {
            if (href?.startsWith("#")) {
              // Sanitizing prefixes every `id` and `name` so a document
              // cannot shadow the app's own, which leaves an in-page link
              // pointing at the unprefixed one. Resolve it here.
              event.preventDefault();
              scrollToFragment(event.currentTarget, href.slice(1));
              return;
            }
            if (!href || !/^https?:\/\//i.test(href)) return;
            event.preventDefault();
            void openExternalUrl(href, { event });
          }}
        >
          {children}
        </a>
      ),
    }),
    [filePath],
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
