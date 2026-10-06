import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Copy, Check, MessageSquarePlus, X } from "lucide-react";
import type { ElementInfo } from "./inspector";

interface Props {
  element: ElementInfo;
  /** Title of the pane "Send to agent" would deliver to; null when the
   *  workspace has no agent chat or terminal to receive it. */
  agentTargetTitle: string | null;
  onDismiss: () => void;
  onTellAgent: (element: ElementInfo) => void;
}

export function InspectorPanel({ element, agentTargetTitle, onDismiss, onTellAgent }: Props) {
  const [copied, setCopied] = useState(false);

  const copySelector = async () => {
    await navigator.clipboard.writeText(element.selector);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  const sendTitle = agentTargetTitle
    ? `Send to agent (${agentTargetTitle})`
    : "No agent in this workspace";

  return (
    <div className="flex shrink-0 items-center gap-2 border-b border-border/50 bg-card px-2 py-1 motion-safe:animate-in motion-safe:fade-in motion-safe:slide-in-from-top-1 motion-safe:duration-150">
      <div className="flex min-w-0 flex-1 items-center gap-2">
        <span className="shrink-0 rounded-sm bg-primary/10 px-1.5 py-0.5 font-mono text-label font-semibold text-primary">
          {element.tag}
          {element.id && <span className="text-muted-foreground">#{element.id}</span>}
        </span>
        {element.classes.length > 0 && (
          <span className="truncate text-label text-muted-foreground">
            .{element.classes.join(".")}
          </span>
        )}
        <span
          className="truncate font-mono text-label text-muted-foreground/70"
          title={element.selector}
        >
          {element.selector}
        </span>
      </div>
      <div className="flex shrink-0 items-center gap-0.5">
        <Button
          variant="ghost"
          size="icon-xs"
          aria-label="Copy Selector"
          title="Copy Selector"
          onClick={copySelector}
        >
          {copied ? <Check className="size-3 text-success" /> : <Copy className="size-3" />}
        </Button>
        {/* The wrapper carries the tooltip: a disabled button gets no hover. */}
        <span title={sendTitle} className="inline-flex">
          <Button
            variant="ghost"
            size="icon-xs"
            aria-label="Send to agent"
            disabled={!agentTargetTitle}
            onClick={() => onTellAgent(element)}
          >
            <MessageSquarePlus className="size-3" />
          </Button>
        </span>
        <Button
          variant="ghost"
          size="icon-xs"
          aria-label="Dismiss"
          title="Dismiss"
          onClick={onDismiss}
        >
          <X className="size-3" />
        </Button>
      </div>
    </div>
  );
}
