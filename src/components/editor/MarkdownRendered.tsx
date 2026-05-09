import { memo, useMemo } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";

import { resolveAssetSrc } from "@/lib/asset-url";

interface Props {
  content: string;
  /**
   * Absolute path of the markdown file being rendered. Used to resolve
   * relative image paths (e.g. `./screenshot.png`) against the file's
   * own directory. Optional — when omitted, only absolute paths and
   * remote URLs render.
   */
  filePath?: string | null;
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
function MarkdownRenderedImpl({ content, filePath }: Props) {
  const plugins = useMemo(() => [remarkGfm], []);

  const components = useMemo<Components>(
    () => ({
      img: ({ src, alt, ...rest }) => (
        // eslint-disable-next-line jsx-a11y/alt-text
        <img src={resolveAssetSrc(src, filePath)} alt={alt ?? ""} {...rest} />
      ),
    }),
    [filePath],
  );

  return (
    <div className="flex-1 min-h-0 overflow-auto bg-[var(--background)]">
      <div className="markdown-rendered max-w-3xl px-8 py-6">
        <ReactMarkdown
          remarkPlugins={plugins}
          components={components}
          // react-markdown's default urlTransform strips data: URIs and
          // other non-http schemes for safety. The content here is the
          // user's own local markdown — we resolve filesystem paths via
          // `resolveAssetSrc` and want data: image URIs to render. An
          // identity transform restores both.
          urlTransform={(url) => url}
        >
          {content}
        </ReactMarkdown>
      </div>
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
