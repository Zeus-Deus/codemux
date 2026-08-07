/**
 * What the right panel can open, as one list.
 *
 * The deck has two "add a surface" affordances — the `+` menu in the tab
 * row and the card grid an empty panel shows — and they must never drift
 * apart. So the action set is built once in `right-panel.tsx` (which owns
 * the availability rules and the handlers) and handed to both renderers.
 * Adding a surface means adding a {@link PaneMeta} to the registry; neither
 * renderer has a list of its own.
 *
 * Terminal is the one entry that is not a deck pane: a terminal is a
 * *workspace* pane, so it routes to the same backend action the main tab
 * strip's `+` uses and opens in the main area. It appears here because
 * "what can I put in this panel?" is the question the user is asking, and
 * answering it with a silent omission would be worse than answering it with
 * a surface that happens to land next door.
 */
import type { LucideIcon } from "lucide-react";

export interface SurfaceAction {
  /** A pane id, or `"terminal"` for the main-area escape hatch. */
  id: string;
  label: string;
  /** One line for the picker card; the menu shows the label alone. */
  description: string;
  icon: LucideIcon;
  onOpen: () => void;
}
