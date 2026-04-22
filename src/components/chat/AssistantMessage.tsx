import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

import { cn } from "@/lib/utils";
import type { AssistantMessageItem } from "@/lib/agent-chat/types";

import { StreamingIndicator } from "./streaming-indicator";

/**
 * Prose-only rendering. Per the chat-ui skill:
 *  - no container / border / avatar / label
 *  - markdown headers render as emphasized prose, NOT <h1>/<h2>
 *  - inline code keeps the prose font with a subtle background
 *  - fenced code blocks share the same neutral monospace styling as
 *    tool-output blocks (see ToolCallBlock)
 *  - bold/italic only when the source asks for it
 */
export function AssistantMessage({ item }: { item: AssistantMessageItem }) {
  const showIndicator = item.streaming && item.text.length === 0;

  return (
    <div className="text-sm leading-relaxed text-foreground break-words">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          h1: ({ children }) => (
            <p className="font-semibold text-foreground mt-3 mb-2 first:mt-0">
              {children}
            </p>
          ),
          h2: ({ children }) => (
            <p className="font-semibold text-foreground mt-3 mb-2 first:mt-0">
              {children}
            </p>
          ),
          h3: ({ children }) => (
            <p className="font-medium text-foreground mt-2 mb-1 first:mt-0">
              {children}
            </p>
          ),
          h4: ({ children }) => (
            <p className="font-medium text-foreground mt-2 mb-1 first:mt-0">
              {children}
            </p>
          ),
          h5: ({ children }) => (
            <p className="font-medium text-foreground mt-2 mb-1 first:mt-0">
              {children}
            </p>
          ),
          h6: ({ children }) => (
            <p className="font-medium text-foreground mt-2 mb-1 first:mt-0">
              {children}
            </p>
          ),
          p: ({ children }) => (
            <p className="mt-2 first:mt-0 mb-2 last:mb-0">{children}</p>
          ),
          ul: ({ children }) => (
            <ul className="mt-2 mb-2 ml-5 list-disc space-y-1">{children}</ul>
          ),
          ol: ({ children }) => (
            <ol className="mt-2 mb-2 ml-5 list-decimal space-y-1">{children}</ol>
          ),
          li: ({ children }) => <li className="leading-relaxed">{children}</li>,
          a: ({ children, href }) => (
            <a
              href={href}
              target="_blank"
              rel="noreferrer"
              className="text-foreground underline underline-offset-2 decoration-muted-foreground/50 hover:decoration-foreground"
            >
              {children}
            </a>
          ),
          code: ({ className, children, ...props }) => {
            const inline = !className;
            if (inline) {
              return (
                <code
                  className="rounded bg-muted/60 px-1 py-0.5 text-[0.9em] text-foreground"
                  {...props}
                >
                  {children}
                </code>
              );
            }
            return (
              <code className={cn("font-mono text-xs", className)} {...props}>
                {children}
              </code>
            );
          },
          pre: ({ children }) => (
            <pre className="mt-2 mb-2 overflow-x-auto rounded-md bg-muted/40 p-3 font-mono text-xs leading-relaxed text-foreground">
              {children}
            </pre>
          ),
          blockquote: ({ children }) => (
            <blockquote className="mt-2 mb-2 border-l-2 border-muted-foreground/30 pl-3 text-muted-foreground">
              {children}
            </blockquote>
          ),
          hr: () => <div className="my-3 h-px bg-border/60" />,
          table: ({ children }) => (
            <div className="mt-2 mb-2 overflow-x-auto">
              <table className="w-full text-xs">{children}</table>
            </div>
          ),
          th: ({ children }) => (
            <th className="px-2 py-1 text-left font-medium text-foreground">
              {children}
            </th>
          ),
          td: ({ children }) => (
            <td className="px-2 py-1 text-foreground">{children}</td>
          ),
        }}
      >
        {item.text}
      </ReactMarkdown>
      {item.streaming && !showIndicator && (
        <StreamingIndicator className="ml-0.5" />
      )}
      {showIndicator && <StreamingIndicator />}
    </div>
  );
}
