import type { ReactNode } from "react";

import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

/** A jargon term with a dotted underline that explains itself on hover or
 *  keyboard focus. A real button, so the explanation is reachable without a
 *  pointer; it carries no button chrome and reads as part of the sentence.
 *  Needs a `TooltipProvider` above it. */
export function TermHint({
  children,
  hint,
  className,
}: {
  children: ReactNode;
  hint: ReactNode;
  className?: string;
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          className={cn(
            // Preflight already strips a button's font, colour and padding.
            "cursor-help rounded-sm underline decoration-muted-foreground/60 decoration-dotted underline-offset-[3px] focus-visible:ring-2 focus-visible:ring-ring/60",
            className,
          )}
        >
          {children}
        </button>
      </TooltipTrigger>
      <TooltipContent side="top" className="max-w-xs text-left">
        {hint}
      </TooltipContent>
    </Tooltip>
  );
}
