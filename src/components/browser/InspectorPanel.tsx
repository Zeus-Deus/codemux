import { useId, useState } from "react";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { Copy, Check, MessageSquarePlus, X } from "lucide-react";
import { COPY_FAILED_MESSAGE, copyToClipboard } from "@/lib/clipboard";
import { toast } from "@/lib/toast";
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
  const noAgentId = useId();

  const copySelector = async () => {
    if (!(await copyToClipboard(element.selector))) {
      toast.error(COPY_FAILED_MESSAGE);
      return;
    }
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
        {/* The wrapper is the tooltip trigger: a disabled button gets no hover,
            and the reason it is disabled must still be readable. */}
        <Tooltip>
          <TooltipTrigger asChild>
            <span className="inline-flex" tabIndex={agentTargetTitle ? undefined : 0}>
              <Button
                variant="ghost"
                size="xs"
                aria-describedby={agentTargetTitle ? undefined : noAgentId}
                disabled={!agentTargetTitle}
                onClick={() => onTellAgent(element)}
              >
                <MessageSquarePlus className="size-3" aria-hidden />
                Send to agent
              </Button>
            </span>
          </TooltipTrigger>
          <TooltipContent>{sendTitle}</TooltipContent>
        </Tooltip>
        {!agentTargetTitle && (
          <span id={noAgentId} className="sr-only">
            {sendTitle}
          </span>
        )}
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
