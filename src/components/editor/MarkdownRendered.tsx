import { useMemo } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

interface Props {
  content: string;
}

export function MarkdownRendered({ content }: Props) {
  const plugins = useMemo(() => [remarkGfm], []);

  return (
    <div className="flex-1 min-h-0 overflow-auto bg-[var(--background)]">
      <div className="markdown-rendered max-w-3xl px-8 py-6">
        <ReactMarkdown remarkPlugins={plugins}>{content}</ReactMarkdown>
      </div>
    </div>
  );
}
