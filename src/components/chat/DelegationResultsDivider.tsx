import { ChevronDown, Inbox } from "lucide-react";
import { useState } from "react";

import type { MessageNotification } from "@/tauri/events";
import { cn } from "@/lib/utils";


/** Structured delegated results, with display-only reports in the fold. */
export function DelegationResultsDivider({ text, notification }: { text: string; notification?: MessageNotification }) {
  const [open, setOpen] = useState(false);
  const outcome = notification?.outcome;
  const failed = outcome === "failed";
  return (
    <div data-testid="delegation-results-divider">
      <button
        type="button"
        aria-expanded={open}
        aria-label={`Delegated results. ${open ? "Hide" : "Show"} the reports`}
        onClick={() => setOpen((cur) => !cur)}
        className="group flex w-full items-center gap-3 rounded-sm text-muted-foreground/70 outline-none transition-colors duration-150 hover:text-muted-foreground focus-visible:ring-1 focus-visible:ring-ring"
      >
        <span className="h-px flex-1 bg-border/60" />
        <span className="inline-flex min-w-0 items-center gap-1.5 font-mono text-label font-medium tracking-wide">
          <Inbox className="size-3 shrink-0" aria-hidden />
          <span className="truncate">
            Delegated results
            {outcome ? (
              <span
                data-testid="delegation-results-outcome"
                className={cn(failed && "text-status-attention")}
              >
                {` · ${outcome}`}
              </span>
            ) : null}
          </span>
          <ChevronDown
            className={cn(
              "size-3 shrink-0 opacity-60 transition-transform duration-150",
              open && "rotate-180",
            )}
            aria-hidden
          />
        </span>
        <span className="h-px flex-1 bg-border/60" />
      </button>
      {open && (
        <div
          data-testid="delegation-results-text"
          className="mt-2.5 max-h-[320px] select-text overflow-y-auto whitespace-pre-wrap break-words rounded-md border border-hairline bg-surface-1 px-3 py-2.5 text-left font-mono text-label leading-relaxed text-muted-foreground thin-scrollbar"
        >
          {text}
        </div>
      )}
    </div>
  );
}
