/**
 * The deck's action-button primitive.
 *
 * The deck used to stack three horizontal bands above the first line of
 * content: the window tab bar, the panel's own tab strip, and a
 * breadcrumb row that repeated the workspace name and the pane name. The
 * breadcrumb row is gone — panes now hand their controls to the tab row's
 * right-hand slot (`PaneTabStrip`'s `actions` prop), so the deck is one
 * band of chrome, not two.
 *
 * Both kinds of control in that row use this button: the *pane* actions
 * that swap when you switch tabs (Files' refresh, the doc pane's wrap
 * toggle, the browser's back/forward) and the *panel* controls pinned
 * after the divider (expand, close). One size, one hover, one radius —
 * a pane that needs a new control adds it to its `actions` node rather
 * than inventing chrome of its own.
 */
import type { LucideIcon } from "lucide-react";

import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

export function PaneActionButton({
  label,
  icon: Icon,
  onClick,
  active = false,
  disabled = false,
  testId,
}: {
  label: string;
  icon: LucideIcon;
  onClick: () => void;
  active?: boolean;
  disabled?: boolean;
  testId?: string;
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          aria-label={label}
          aria-pressed={active}
          disabled={disabled}
          data-testid={testId}
          onClick={onClick}
          className={cn(
            "flex size-6 shrink-0 items-center justify-center rounded-md",
            "transition-colors duration-[120ms]",
            "disabled:pointer-events-none disabled:opacity-40",
            active
              ? "bg-foreground/10 text-foreground"
              : "text-foreground/42 hover:bg-foreground/8 hover:text-foreground/80",
          )}
        >
          <Icon className="size-[13px]" strokeWidth={1.6} />
        </button>
      </TooltipTrigger>
      <TooltipContent side="bottom" sideOffset={4}>
        {label}
      </TooltipContent>
    </Tooltip>
  );
}
