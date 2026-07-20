import { useState } from "react";

import { isImageBlock } from "@/lib/agent-chat/tool-result-images";
import { cn } from "@/lib/utils";

const PREVIEW_LINES = 12;

interface Props {
  content: unknown;
  error?: boolean;
  /** Override the auto-stringified text (e.g. a tool input payload). */
  text?: string;
}

/**
 * Tier-3 content block. Recessed monospace surface. No header bar, no
 * syntax highlighting. Diffs stay neutral per the skill's explicit
 * "no green/red" rule.
 */
export function ToolCallBlock({ content, error, text }: Props) {
  const [expanded, setExpanded] = useState(false);
  const body = text ?? contentToString(content);
  // An image-only tool result stringifies to nothing (image blocks are
  // filtered out and rendered as thumbnails elsewhere). Render nothing
  // rather than an empty recessed box.
  if (body.trim() === "") return null;
  const lines = body.split("\n");
  const showToggle = lines.length > PREVIEW_LINES;
  const visible =
    showToggle && !expanded ? lines.slice(0, PREVIEW_LINES).join("\n") : body;
  const hiddenCount = lines.length - PREVIEW_LINES;

  return (
    <div
      className={cn(
        "rounded-md bg-muted/40 font-mono text-[11.5px] leading-5",
        "text-foreground whitespace-pre-wrap break-words",
        error && "text-foreground",
      )}
    >
      <div className="px-3 py-2">
        {visible}
        {showToggle && !expanded && (
          <span className="text-muted-foreground/60">{"\n"}…</span>
        )}
      </div>
      {showToggle && (
        <button
          type="button"
          onClick={() => setExpanded((v) => !v)}
          className="block w-full border-t border-border/40 px-3 py-1.5 text-left text-[11px] text-muted-foreground hover:text-foreground"
        >
          {expanded ? "Show less" : `Show ${hiddenCount} more line${hiddenCount === 1 ? "" : "s"}`}
        </button>
      )}
    </div>
  );
}

function contentToString(content: unknown): string {
  if (content == null) return "";
  if (typeof content === "string") return content;
  if (Array.isArray(content)) {
    return content
      .filter((entry) => !isImageBlock(entry))
      .map((entry) => {
        if (entry == null) return "";
        if (typeof entry === "string") return entry;
        if (
          typeof entry === "object" &&
          "text" in entry &&
          typeof (entry as { text: unknown }).text === "string"
        ) {
          return (entry as { text: string }).text;
        }
        return JSON.stringify(entry, null, 2);
      })
      .join("\n");
  }
  if (typeof content === "object") {
    const maybeText = (content as { text?: unknown }).text;
    if (typeof maybeText === "string") return maybeText;
    return JSON.stringify(content, null, 2);
  }
  return String(content);
}
