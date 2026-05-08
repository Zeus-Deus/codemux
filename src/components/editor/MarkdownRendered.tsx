import { useMemo } from "react";
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

export function MarkdownRendered({ content, filePath }: Props) {
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
