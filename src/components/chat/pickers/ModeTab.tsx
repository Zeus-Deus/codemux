import { useEffect, useState } from "react";
import {
  Bug,
  ListTodo,
  MessageCircleQuestion,
  X,
  type LucideIcon,
} from "lucide-react";

import { cn } from "@/lib/utils";

/** The three concrete modes that render a tab. `default` (Build) shows
 *  nothing. */
export type ActivePillMode = "plan" | "ask" | "debug";

interface ModeConfig {
  label: string;
  /** Tailwind bg class at 15% opacity per the locked Research 2
   *  decision. */
  bg: string;
  /** Foreground text color — the same token the bg derives from,
   *  full opacity. Keeps the tab readable on light + dark themes. */
  text: string;
  /** The tab's hairline, from the same token. */
  border: string;
  icon: LucideIcon;
}

/** Color mapping locked by Research 2: Plan = --primary,
 *  Debug = --danger, Ask = --success. Only the tab is tinted; the
 *  composer card keeps its neutral border. */
export const MODE_CONFIG: Record<ActivePillMode, ModeConfig> = {
  plan: {
    label: "Plan",
    bg: "bg-primary/15",
    text: "text-primary",
    border: "border-primary/30",
    icon: ListTodo,
  },
  ask: {
    label: "Ask",
    bg: "bg-success/15",
    text: "text-success",
    border: "border-success/30",
    icon: MessageCircleQuestion,
  },
  debug: {
    label: "Debug",
    bg: "bg-danger/15",
    text: "text-danger",
    border: "border-danger/30",
    icon: Bug,
  },
};

/** Past the 150ms slide, so the backstop only ever catches a lost
 *  animationend. */
const EXIT_BACKSTOP_MS = 250;

/**
 * Folder tab on the composer's top edge naming the active mode. Rendered
 * directly above the card (or above whatever is docked on it), with no
 * bottom border so it sits flush on that edge.
 *
 * The leading icon doubles as the leave button: hovering the tab, focusing
 * the button, or any touch screen (no hover) swaps the icon for an ×, so
 * the tab never changes size. It slides up out of the edge when a mode
 * starts and back down when it ends; the last mode is held through the
 * exit so the label doesn't vanish mid-slide.
 */
export function ModeTab({
  mode,
  onRemove,
}: {
  /** `null` for Build, which shows no tab. */
  mode: ActivePillMode | null;
  /** Leaves the mode. The parent owns the restore (Plan's permission
   *  mode, Ask / Debug's prompt wrapper). */
  onRemove: () => void;
}) {
  const [shown, setShown] = useState(mode);
  // Adjusting state during render keeps the tab in step with `mode` in the
  // same commit, while a cleared mode leaves `shown` for the exit slide.
  if (mode !== null && mode !== shown) setShown(mode);
  const leaving = mode === null && shown !== null;
  // Backstop for an exit whose animationend never comes (the pane hidden
  // mid-slide), so an empty band can't hold the space.
  useEffect(() => {
    if (!leaving) return;
    const timer = setTimeout(() => setShown(null), EXIT_BACKSTOP_MS);
    return () => clearTimeout(timer);
  }, [leaving]);
  if (shown === null) return null;

  const config = MODE_CONFIG[shown];
  const Icon = config.icon;
  return (
    // The clip box is the tab's slot: its bottom edge is the edge the tab
    // slides through. It holds its height through the exit, then unmounts.
    <div
      data-testid="composer-mode-tab"
      data-leaving={leaving || undefined}
      aria-hidden={leaving || undefined}
      inert={leaving}
      className="flex h-6 overflow-hidden pl-[34px]"
    >
      <div
        role="status"
        aria-label={`${config.label} mode active`}
        onAnimationEnd={() => {
          if (leaving) setShown(null);
        }}
        className={cn(
          "group/tab inline-flex items-center gap-1.5 rounded-t-lg border border-b-0 px-2.5 text-label duration-150 ease-out",
          leaving
            ? "animate-out slide-out-to-bottom fill-mode-forwards"
            : "animate-in slide-in-from-bottom",
          config.bg,
          config.text,
          config.border,
        )}
      >
        <button
          type="button"
          onClick={onRemove}
          aria-label={`Leave ${config.label} mode`}
          className={cn(
            "group/leave relative grid size-3 place-items-center rounded-sm",
            // The phone layout's 44px button floor would stretch the tab;
            // the pseudo-element below gives touch the larger target.
            "in-[[data-mobile]]:min-h-0! in-[[data-mobile]]:min-w-0!",
            // Hit area: the tab's whole leading slot, never the label.
            "after:absolute after:-inset-y-1.5 after:-left-2.5 after:-right-1",
          )}
        >
          <Icon
            aria-hidden
            className={cn(
              "col-start-1 row-start-1 size-3 transition-opacity duration-100",
              "group-hover/tab:opacity-0 group-focus-visible/leave:opacity-0",
              "pointer-coarse:opacity-0 [@media(hover:none)]:opacity-0",
            )}
          />
          <X
            aria-hidden
            className={cn(
              "col-start-1 row-start-1 size-3 opacity-0 transition-opacity duration-100",
              "group-hover/tab:opacity-100 group-focus-visible/leave:opacity-100",
              "pointer-coarse:opacity-100 [@media(hover:none)]:opacity-100",
            )}
          />
        </button>
        <span>{config.label}</span>
      </div>
    </div>
  );
}
