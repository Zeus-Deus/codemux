/**
 * What an open, empty right panel shows.
 *
 * Closing the last tab used to collapse the whole panel, and opening a panel
 * whose deck was empty gave you a blank column with a one-line apology in
 * the middle of it ("No panes open — use + to add one"), pointing at a `+`
 * the eye had no reason to have found yet. Both now land here: a titled
 * placeholder over a grid of the surfaces this panel can actually open.
 *
 * The cards are not a second menu. They render the exact `SurfaceAction`
 * array the `+` menu renders (built in `right-panel.tsx`), so availability
 * rules and handlers can't diverge — a Browser card opens the docked
 * browser pane, a Terminal card opens a real terminal in the main area,
 * same as the menu items.
 *
 * The grid is container-relative, not viewport-relative: the panel can be
 * 240px or 1400px wide inside the same window, so a `sm:` breakpoint would
 * be measuring the wrong box.
 */
import { cn } from "@/lib/utils";

import type { SurfaceAction } from "./surface-actions";

export function PanePicker({ surfaces }: { surfaces: SurfaceAction[] }) {
  return (
    <div
      data-testid="right-panel-picker"
      className="@container flex h-full min-h-0 items-center justify-center overflow-y-auto p-6"
    >
      <div className="w-full max-w-[520px]">
        <div className="mb-4 text-center">
          <h3 className="text-[13px] font-medium text-foreground">
            Open a surface
          </h3>
          <p className="mt-1 text-[11px] text-muted-foreground">
            Choose what to show in the right panel.
          </p>
        </div>
        <div className="grid grid-cols-1 gap-2 @[380px]:grid-cols-2">
          {surfaces.map((surface) => (
            <button
              key={surface.id}
              type="button"
              data-testid={`right-panel-picker-${surface.id}`}
              onClick={surface.onOpen}
              className={cn(
                "flex w-full flex-col items-start rounded-xl border border-border/70 bg-card p-3 text-left",
                "transition-colors duration-[120ms]",
                "hover:border-border hover:bg-foreground/5",
              )}
            >
              <surface.icon
                className="mb-2 size-[18px] text-foreground/70"
                strokeWidth={1.5}
              />
              <span className="text-[13px] font-medium text-foreground">
                {surface.label}
              </span>
              <span className="mt-0.5 text-[11px] leading-relaxed text-muted-foreground">
                {surface.description}
              </span>
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
