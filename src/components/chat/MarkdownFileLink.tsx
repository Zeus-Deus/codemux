import { memo, type ReactNode } from "react";

import { FileTypeIcon } from "@/components/icons/file-type-icon";
import type { ChatFileLinkMeta } from "@/lib/agent-chat/file-links";
import { openRightPanelDoc } from "@/lib/open-right-panel-doc";
import { cn } from "@/lib/utils";

import { useChatFileLinkContext } from "./chat-file-link-context";

export const MarkdownFileLink = memo(function MarkdownFileLink({
  meta,
  children,
  className,
  variant = "inline",
}: {
  meta: ChatFileLinkMeta;
  children?: ReactNode;
  className?: string;
  variant?: "inline" | "plain";
}) {
  const { workspaceId } = useChatFileLinkContext();
  const location = meta.line
    ? ` · L${meta.line}${meta.column ? `:C${meta.column}` : ""}`
    : "";
  const label = children || `${meta.basename}${location}`;
  const interactive = !!workspaceId;

  if (!interactive) {
    if (variant === "plain") {
      return <span className={className}>{label}</span>;
    }
    return (
      <code className={className} data-streamdown="inline-code">
        {label}
      </code>
    );
  }

  return (
    <button
      type="button"
      title={meta.filePath + location}
      onClick={() =>
        openRightPanelDoc(
          workspaceId,
          meta.filePath,
          meta.line,
          meta.column,
        )
      }
      className={cn(
        variant === "inline"
          ? "mx-px inline-flex max-w-full translate-y-px cursor-pointer items-center gap-1 align-baseline font-mono text-[0.86em] font-medium leading-[1.45] text-foreground/80 no-underline transition-colors hover:text-foreground focus-visible:rounded-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/70 focus-visible:ring-offset-1 focus-visible:ring-offset-background"
          : "inline-flex min-w-0 max-w-full items-center gap-1 text-left text-inherit transition-colors hover:text-foreground focus-visible:rounded-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/70",
        className,
      )}
    >
      <FileTypeIcon filename={meta.filePath} className="size-3 shrink-0" />
      <span className="min-w-0 truncate">{label}</span>
    </button>
  );
});
