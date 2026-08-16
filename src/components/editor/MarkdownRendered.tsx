import { memo, useMemo } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import rehypeRaw from "rehype-raw";
import remarkGfm from "remark-gfm";

import { resolveAssetSrc } from "@/lib/asset-url";

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
function MarkdownRenderedImpl({ content, filePath, inline = false }: Props) {
  const remarkPlugins = useMemo(() => [remarkGfm], []);
  // `rehypeRaw` re-parses the raw HTML that GFM leaves as opaque nodes
  // (e.g. a README's `<div align="center">` wrapper and `<img>` logo) so
  // it renders as real elements instead of escaped text. Raw `<img>`
  // tags become proper `img` nodes, so the `img` override below still
  // routes their `src` through `resolveAssetSrc`.
  const rehypePlugins = useMemo(() => [rehypeRaw], []);

  const components = useMemo<Components>(
    () => ({
      img: ({ src, alt, ...rest }) => (
        // eslint-disable-next-line jsx-a11y/alt-text
        <img src={resolveAssetSrc(src, filePath)} alt={alt ?? ""} {...rest} />
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

  if (inline) return body;

  return (
    <div className="flex-1 min-h-0 overflow-auto bg-[var(--background)]">{body}</div>
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
