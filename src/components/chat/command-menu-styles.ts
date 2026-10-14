/**
 * Shared look for the composer's command surfaces: the `+` menu
 * ({@link ComposerCommandMenu}) and the typed `/` and `@` popup
 * ({@link SlashCommandPopup}). Both float over the composer card, so
 * they reuse its surface, border and shadow and read as part of it.
 */

/** Composer-matched surface. Keep the background in sync with the
 *  composer card in `Composer.tsx`. */
export const COMMAND_MENU_SURFACE = [
  "rounded-2xl border border-border/80",
  "bg-[color-mix(in_oklab,var(--muted)_40%,var(--background))]",
  "text-popover-foreground",
  "shadow-[0_18px_40px_-12px] shadow-black/55",
].join(" ");

/** Quiet section headings for cmdk groups. */
export const COMMAND_MENU_GROUP_HEADING = [
  "**:[[cmdk-group-heading]]:px-2.5",
  "**:[[cmdk-group-heading]]:pt-2.5 **:[[cmdk-group-heading]]:pb-1",
  "**:[[cmdk-group-heading]]:text-caption",
  "**:[[cmdk-group-heading]]:font-medium",
  "**:[[cmdk-group-heading]]:uppercase",
  "**:[[cmdk-group-heading]]:tracking-[0.08em]",
  "**:[[cmdk-group-heading]]:text-muted-foreground/60",
].join(" ");

/** Row chrome. The highlighted row gets a faint fill plus a short
 *  ember bar on its left edge, so the keyboard position stays visible
 *  without colouring every row. Pair with `group/row` children via
 *  {@link COMMAND_MENU_ICON}. */
export const COMMAND_MENU_ROW = [
  "group/row relative flex items-center gap-2.5 rounded-lg px-2.5",
  "cursor-pointer outline-none select-none",
  "data-[selected=true]:bg-surface-2",
  "before:absolute before:inset-y-2 before:left-0 before:w-0.5 before:rounded-full",
  "before:bg-accent-ember before:opacity-0 data-[selected=true]:before:opacity-100",
].join(" ");

/** Default icon colour: muted at rest, ember on the highlighted row. */
export const COMMAND_MENU_ICON =
  "text-muted-foreground group-data-[selected=true]/row:text-accent-ember";
