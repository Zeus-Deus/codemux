import { ChevronDown, Inbox } from "lucide-react";
import { useMemo, useState } from "react";

import { parseWake, wakeSummary } from "@/lib/agent-chat/delegation";
import { cn } from "@/lib/utils";

import { ProviderLogo } from "./provider-logo";

/**
 * The turn Codemux posts into the parent when a round of delegated tasks
 * reports back. The user didn't type it, so it reads as a quiet hairline
 * divider like the automatic usage-limit resume — "Delegated results ·
 * Codex, Claude · completed" — and expands to the exact text the model
 * received, which can run to tens of thousands of characters, so it opens
 * into a bounded, scrolling block rather than a wall of centered prose.
 */
export function DelegationResultsDivider({ text }: { text: string }) {
  const [open, setOpen] = useState(false);
  const summary = useMemo(() => wakeSummary(parseWake(text)), [text]);
  return (
    <div data-testid="delegation-results-divider">
      <button
        type="button"
        aria-expanded={open}
        aria-label={`${summary.label}. ${open ? "Hide" : "Show"} the reports`}
        onClick={() => setOpen((cur) => !cur)}
        className="group flex w-full items-center gap-3 rounded-sm text-muted-foreground/70 outline-none transition-colors duration-150 hover:text-muted-foreground focus-visible:ring-1 focus-visible:ring-ring"
      >
        <span className="h-px flex-1 bg-border/60" />
        <span className="inline-flex min-w-0 items-center gap-1.5 font-mono text-label font-medium tracking-wide">
          {summary.providers.length > 0 ? (
            <span className="flex shrink-0 items-center gap-1" aria-hidden>
              {summary.providers.map((provider) => (
                <ProviderLogo
                  key={provider}
                  provider={provider}
                  className="size-3 opacity-80 transition-opacity duration-150 group-hover:opacity-100"
                />
              ))}
            </span>
          ) : (
            <Inbox className="size-3 shrink-0" aria-hidden />
          )}
          <span className="truncate">
            Delegated results
            {summary.names ? ` · ${summary.names}` : null}
            {summary.outcome ? (
              <span
                data-testid="delegation-results-outcome"
                className={cn(summary.failed && "text-status-attention")}
              >
                {` · ${summary.outcome}`}
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
