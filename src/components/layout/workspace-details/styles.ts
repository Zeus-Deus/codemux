/**
 * Row recipe for the Workspace details card (`workspace-details-panel.tsx`).
 *
 * Every row is 32px with a 16px muted icon, a 13px regular-weight label and
 * an optional right-aligned trailing element. A *split* row is one hover surface with
 * two hit targets — the primary action and a 32px chevron that opens a
 * menu — divided by a short hairline, so "Open in Cursor ▾" and "Run ▾"
 * read as one control rather than two buttons side by side.
 */

/** A plain full-width row: a button, or a static `div` for read-only facts. */
export const DETAILS_ROW =
  "flex h-8 w-full min-w-0 items-center gap-2.5 rounded-lg px-2.5 text-left text-body-sm text-foreground outline-none transition-colors duration-100 hover:bg-surface-2 focus-visible:bg-surface-2 disabled:pointer-events-none disabled:opacity-50 data-[state=open]:bg-surface-2";

/** A row that only reports something — no hover, since it does nothing. */
export const DETAILS_STATIC_ROW =
  "flex h-8 w-full min-w-0 items-center gap-2.5 rounded-lg px-2.5 text-body-sm text-foreground";

export const DETAILS_ICON = "size-4 shrink-0 text-muted-foreground";

export const DETAILS_LABEL = "min-w-0 flex-1 truncate";

/** Right-aligned counts and states: data, but not code, so sans with
 *  tabular figures. Diff stats opt back into mono (`DETAILS_DIFF_STAT`). */
export const DETAILS_TRAILING =
  "flex shrink-0 items-center gap-1 text-label tabular-nums text-muted-foreground";

/** `+N −N`: diff stats stay mono so the figures line up like a diff. */
export const DETAILS_DIFF_STAT = "font-mono";

/** The split row's shared hover surface. Stays lit while its menu is open. */
export const DETAILS_SPLIT_GROUP =
  "flex w-full min-w-0 items-center rounded-lg transition-colors duration-100 hover:bg-surface-2 has-[[data-state=open]]:bg-surface-2";

export const DETAILS_SPLIT_PRIMARY =
  "flex h-8 min-w-0 flex-1 items-center gap-2.5 rounded-l-lg px-2.5 text-left text-body-sm text-foreground outline-none focus-visible:bg-surface-2 disabled:pointer-events-none disabled:opacity-50";

export const DETAILS_SPLIT_SEPARATOR = "h-4 w-px shrink-0 bg-border";

export const DETAILS_SPLIT_SECONDARY =
  "flex h-8 w-8 shrink-0 items-center justify-center rounded-r-lg text-muted-foreground outline-none hover:text-foreground focus-visible:bg-surface-2 disabled:pointer-events-none disabled:opacity-50";

export const DETAILS_CHEVRON = "size-3.5 shrink-0";

/** Menu rows opened from the card: the card's own 32px / 13px density, so
 *  a dropdown reads as part of it rather than a denser surface on top. */
export const DETAILS_MENU_ITEM = "h-8 gap-2.5 px-2 text-body-sm";

/** Icons inside those menu rows, matching the card's muted 16px icons. */
export const DETAILS_MENU_ICON = "size-4 text-muted-foreground";
