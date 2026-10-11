/**
 * What an open, empty right panel shows.
 *
 * Closing the last tab used to collapse the whole panel, and opening a panel
 * whose deck was empty gave you a blank column with a one-line apology in
 * the middle of it ("No panes open — use + to add one"), pointing at a `+`
 * the eye had no reason to have found yet. Both now land here: a titled,
 * compact list of the surfaces this panel can actually open. It used to be
 * a grid of described cards, which read heavier than anything else in the
 * panel; each surface's description is now its tooltip.
 *
 * The rows are not a second menu. They render the exact `SurfaceAction`
 * array the `+` menu renders (built in `right-panel.tsx`), so availability
 * rules and handlers can't diverge — a Browser row opens the docked
 * browser pane, a Terminal row opens a real terminal in the main area,
 * same as the menu items.
 */
import type { SurfaceAction } from "./surface-actions";

export function PanePicker({ surfaces }: { surfaces: SurfaceAction[] }) {
  return (
    <div
      data-testid="right-panel-picker"
      // The bottom padding matches the tab row above, so the list sits at
      // the optical centre of the whole panel rather than of its body.
      className="flex h-full min-h-0 items-center justify-center overflow-y-auto px-6 pb-10"
    >
      <div className="w-full max-w-[280px] py-6">
        <h3 className="mb-3 text-center text-body font-medium text-foreground">
          Open a surface
        </h3>
        <div className="flex flex-col gap-0.5">
          {surfaces.map((surface) => (
            <button
              key={surface.id}
              type="button"
              data-testid={`right-panel-picker-${surface.id}`}
              title={surface.description}
              onClick={surface.onOpen}
              className="flex h-8 w-full items-center gap-2.5 rounded-md px-2.5 text-left text-body text-foreground/70 transition-colors duration-100 hover:bg-surface-2 hover:text-foreground"
            >
              <surface.icon className="size-4 shrink-0" />
              <span className="min-w-0 flex-1 truncate">{surface.label}</span>
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
