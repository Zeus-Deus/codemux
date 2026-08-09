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
  size = "pane",
}: {
  label: string;
  icon: LucideIcon;
  onClick: () => void;
  active?: boolean;
  disabled?: boolean;
  testId?: string;
  /** Titlebar controls match its 28px sidebar toggle; pane chrome stays dense. */
  size?: "pane" | "titlebar";
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
            "flex shrink-0 items-center justify-center",
            size === "titlebar"
              ? "size-7 rounded-[min(var(--radius-md),12px)]"
              : "size-6 rounded-md",
            "transition-colors duration-[120ms]",
            "disabled:pointer-events-none disabled:opacity-40",
            active
              ? "bg-foreground/10 text-foreground"
              : size === "titlebar"
                ? "text-muted-foreground hover:bg-muted hover:text-foreground dark:hover:bg-muted/50"
                : "text-foreground/42 hover:bg-foreground/8 hover:text-foreground/80",
          )}
        >
          <Icon
            className={size === "titlebar" ? "size-3.5" : "size-[13px]"}
            strokeWidth={size === "titlebar" ? 2 : 1.6}
          />
        </button>
      </TooltipTrigger>
      <TooltipContent side="bottom" sideOffset={4}>
        {label}
      </TooltipContent>
    </Tooltip>
  );
}
