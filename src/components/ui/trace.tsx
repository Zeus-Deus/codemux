import { ChevronRightIcon } from "lucide-react";
import * as React from "react";

import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { cn } from "@/lib/utils";

/**
 * shadcn/ui Trace (October 2026): one line shape for every step an agent
 * takes — reasoning, tool calls, subagent runs. Each line is an icon, a
 * title, and optional description and status; a line expands, links out or
 * opens something by being wrapped in that thing's trigger.
 *
 * Adapted from the radix-nova registry item to Codemux's tokens: the rem
 * type scale instead of raw Tailwind sizes, the 100/150/250 duration ladder, the app's
 * focus ring, the attention status colour for failures, and the running
 * shimmer from `globals.css` (a plain class there, so the selector lives in
 * that file rather than as a Tailwind variant here).
 */

function Trace({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="trace"
      className={cn(
        "flex w-full min-w-0 flex-col gap-1.5 *:has-[>[data-slot=trace-line]]:flex *:has-[>[data-slot=trace-line]]:w-fit *:has-[>[data-slot=trace-line]]:max-w-full *:has-[>[data-slot=trace-line]]:min-w-0 *:has-[>[data-slot=trace-line]]:rounded-md *:has-[>[data-slot=trace-line]]:text-left *:has-[>[data-slot=trace-line]]:outline-none *:has-[>[data-slot=trace-line]]:focus-visible:ring-2 *:has-[>[data-slot=trace-line]]:focus-visible:ring-ring/60",
        className,
      )}
      {...props}
    />
  );
}

type TraceStatus = "running" | "error" | "pending";

function TraceLine({
  className,
  status,
  ...props
}: React.ComponentProps<"span"> & { status?: TraceStatus }) {
  return (
    <span
      data-slot="trace-line"
      data-status={status}
      className={cn(
        "flex w-fit max-w-full min-w-0 items-center gap-2 text-body-sm leading-5 text-muted-foreground transition-colors duration-150 in-[a,button]:cursor-pointer in-[a:hover,button:hover]:text-foreground data-[status=error]:text-status-attention data-[status=error]:in-[a:hover,button:hover]:text-status-attention data-[status=pending]:opacity-60",
        className,
      )}
      {...props}
    />
  );
}

function TraceIcon({ className, ...props }: React.ComponentProps<"span">) {
  return (
    <span
      data-slot="trace-icon"
      aria-hidden="true"
      className={cn(
        "flex size-4 shrink-0 items-center justify-center [&_svg]:pointer-events-none [&_svg:not([class*='size-'])]:size-3.5 [[data-slot=trace-title]~&]:-ml-1",
        className,
      )}
      {...props}
    />
  );
}

function TraceTitle({ className, ...props }: React.ComponentProps<"span">) {
  return (
    <span
      data-slot="trace-title"
      className={cn("min-w-0 truncate", className)}
      {...props}
    />
  );
}

function TraceDescription({
  className,
  ...props
}: React.ComponentProps<"span">) {
  return (
    <span
      data-slot="trace-description"
      className={cn(
        "shrink-0 whitespace-nowrap text-muted-foreground/60 tabular-nums",
        className,
      )}
      {...props}
    />
  );
}

function TraceCollapsible({
  className,
  ...props
}: Omit<React.ComponentProps<typeof Collapsible>, "asChild" | "open"> &
  (
    | { open?: undefined }
    | {
        open: boolean;
        onOpenChange: NonNullable<
          React.ComponentProps<typeof Collapsible>["onOpenChange"]
        >;
      }
  )) {
  return (
    <Collapsible
      data-slot="trace-collapsible"
      className={cn("flex w-full min-w-0 flex-col", className)}
      {...props}
    />
  );
}

function TraceCollapsibleTrigger({
  className,
  children,
  ...props
}: Omit<React.ComponentProps<typeof CollapsibleTrigger>, "asChild">) {
  return (
    <CollapsibleTrigger
      data-slot="trace-collapsible-trigger"
      className={cn(
        "group/trace-collapsible-trigger flex w-fit max-w-full min-w-0 cursor-pointer items-center gap-1 rounded-md text-left text-muted-foreground transition-colors duration-150 outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60 has-[[data-status=error]]:text-status-attention",
        className,
      )}
      {...props}
    >
      {children}
      <TraceIcon className="opacity-60 transition-transform duration-150 ease-drawer group-aria-expanded/trace-collapsible-trigger:rotate-90">
        <ChevronRightIcon className="size-3.5" />
      </TraceIcon>
    </CollapsibleTrigger>
  );
}

function TraceCollapsibleContent({
  className,
  children,
  ...props
}: Omit<React.ComponentProps<typeof CollapsibleContent>, "asChild">) {
  return (
    <CollapsibleContent
      data-slot="trace-collapsible-content"
      className="min-w-0 overflow-hidden duration-150 ease-drawer data-[state=open]:animate-collapsible-down data-[state=closed]:animate-collapsible-up data-[state=closed]:duration-100 data-[state=closed]:ease-out"
      {...props}
    >
      <div
        data-slot="trace-collapsible-content-inner"
        className={cn(
          "mt-1.5 mb-0.5 ml-[calc(--spacing(2)-0.5px)] min-w-0 border-l border-border/60 pr-1 pl-4 leading-relaxed wrap-break-word text-muted-foreground",
          className,
        )}
      >
        {children}
      </div>
    </CollapsibleContent>
  );
}

export {
  Trace,
  TraceCollapsible,
  TraceCollapsibleContent,
  TraceCollapsibleTrigger,
  TraceDescription,
  TraceIcon,
  TraceLine,
  TraceTitle,
  type TraceStatus,
};
