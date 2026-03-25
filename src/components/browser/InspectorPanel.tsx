import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Copy, Check, Terminal, X } from "lucide-react";
import type { ElementInfo } from "./inspector";

interface Props {
  element: ElementInfo;
  onDismiss: () => void;
  onTellAgent: (selector: string) => void;
}

export function InspectorPanel({ element, onDismiss, onTellAgent }: Props) {
  const [copied, setCopied] = useState(false);

  const copySelector = async () => {
    await navigator.clipboard.writeText(element.selector);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  return (
    <div className="flex shrink-0 items-center gap-2 border-b border-border/50 bg-card px-2 py-1">
      <div className="flex min-w-0 flex-1 items-center gap-2">
        <span className="shrink-0 rounded bg-primary/10 px-1.5 py-0.5 font-mono text-[11px] font-semibold text-primary">
          {element.tag}
          {element.id && <span className="text-muted-foreground">#{element.id}</span>}
        </span>
        {element.classes.length > 0 && (
          <span className="truncate text-[11px] text-muted-foreground">
            .{element.classes.join(".")}
          </span>
        )}
        <span
          className="truncate font-mono text-[11px] text-muted-foreground/70"
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
          {copied ? <Check className="h-3 w-3 text-success" /> : <Copy className="h-3 w-3" />}
        </Button>
        <Button
          variant="ghost"
          size="icon-xs"
          aria-label="Tell Agent"
          title="Tell Agent"
          onClick={() => onTellAgent(element.selector)}
        >
          <Terminal className="h-3 w-3" />
        </Button>
        <Button
          variant="ghost"
          size="icon-xs"
          aria-label="Dismiss"
          title="Dismiss"
          onClick={onDismiss}
        >
          <X className="h-3 w-3" />
        </Button>
      </div>
    </div>
  );
}
