import { useState } from "react";

import { ArrowUpRight, ArrowDownToLine, HelpCircle, Cloud } from "lucide-react";

import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { Button } from "@/components/ui/button";

const SEEN_KEY = "codemux.workspaces.howItWorksSeen";

function markSeen() {
  try {
    localStorage.setItem(SEEN_KEY, "1");
  } catch {
    /* no-op */
  }
}

function hasSeen(): boolean {
  try {
    return localStorage.getItem(SEEN_KEY) === "1";
  } catch {
    return false;
  }
}

/**
 * "How does this work?" popover in the Workspaces overview header.
 *
 * Three-step explainer of the push → sync → pull model with subtle
 * icons. Lives next to the filter bar so it's always reachable but
 * never demanding. First-time users get a tiny "new" dot on the
 * `?` icon (clears on first open) so the affordance is discoverable
 * without being noisy.
 *
 * Design notes:
 * - Anchored popover (not modal) — explaining the model shouldn't
 *   block what the user was doing.
 * - Each step pairs a small accent-coloured icon with a one-line
 *   description. The icons reinforce the per-row affordances the
 *   user will see in the overview (Push uses the same up-right
 *   arrow the workspace menu does; Pull uses the down-to-line
 *   icon; Cloud is the cross-device-sync glyph).
 * - Closes on outside-click via Radix's default behaviour.
 */
export function HowItWorksPopover() {
  const [hasSeenIt, setHasSeenIt] = useState(() => hasSeen());

  return (
    <Popover
      onOpenChange={(open) => {
        if (open && !hasSeenIt) {
          markSeen();
          setHasSeenIt(true);
        }
      }}
    >
      <PopoverTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          aria-label="How does this work?"
          className="relative size-7 text-muted-foreground/70 hover:text-foreground"
        >
          <HelpCircle className="size-3.5" />
          {!hasSeenIt && (
            <span
              aria-hidden
              className="absolute -top-0.5 -right-0.5 flex size-2"
            >
              <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-status-remote opacity-75" />
              <span className="relative inline-flex size-2 rounded-full bg-status-remote" />
            </span>
          )}
        </Button>
      </PopoverTrigger>
      <PopoverContent
        align="end"
        sideOffset={6}
        className="w-[320px] p-0 overflow-hidden"
      >
        <div className="bg-gradient-to-br from-status-remote/10 via-transparent to-status-open/8 px-4 py-3 border-b border-border/40">
          <p className="text-[12.5px] font-semibold text-foreground">
            How workspaces sync across devices
          </p>
          <p className="mt-0.5 text-[11px] text-muted-foreground/75 leading-relaxed">
            Files live in one place at a time. The registry tells every
            device of your account where each workspace is.
          </p>
        </div>
        <ol className="px-4 py-3 space-y-3 text-[11.5px] leading-relaxed">
          <Step
            n="1"
            icon={<ArrowUpRight className="size-3 text-status-open" />}
            title="Push from any device"
            body="Right-click a workspace → Move to <device>. Files rsync to the destination."
          />
          <Step
            n="2"
            icon={<Cloud className="size-3 text-status-remote" />}
            title="Sync makes it visible everywhere"
            body="The workspace appears in this overview on every device you're signed in to."
          />
          <Step
            n="3"
            icon={
              <ArrowDownToLine className="size-3 text-accent-violet" />
            }
            title="Pull to any device when you want it"
            body="Sibling-device rows offer Pull to this device → files come over via rsync or git clone."
          />
        </ol>
        <div className="px-4 py-2.5 border-t border-border/40 bg-muted/20">
          <p className="text-[10.5px] text-muted-foreground/65 leading-relaxed">
            Every push and pull is undoable for 10 seconds. Workspace
            files never sync silently — they only move when you say so.
          </p>
        </div>
      </PopoverContent>
    </Popover>
  );
}

function Step({
  n,
  icon,
  title,
  body,
}: {
  n: string;
  icon: React.ReactNode;
  title: string;
  body: string;
}) {
  return (
    <li className="flex items-start gap-3">
      <span className="flex size-5 shrink-0 items-center justify-center rounded-full border border-border/50 bg-muted/30 font-mono text-[10px] text-muted-foreground/70 tabular-nums">
        {n}
      </span>
      <div className="min-w-0 flex-1">
        <p className="flex items-center gap-1.5 text-[12px] font-medium text-foreground/95">
          {icon}
          {title}
        </p>
        <p className="mt-0.5 text-[11px] text-muted-foreground/75">
          {body}
        </p>
      </div>
    </li>
  );
}

export const HOW_IT_WORKS_SEEN_KEY = SEEN_KEY;
