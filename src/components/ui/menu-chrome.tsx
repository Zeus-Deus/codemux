import { cn } from "@/lib/utils";
import { useResolvedKeybinds } from "@/hooks/use-resolved-keybinds";

/**
 * Shared chrome for the app's popup menus (workspace right-click, the footer
 * gear menu, the projects `+` menu and their submenus).
 *
 * These menus deliberately read as one surface with the command palette —
 * same radius, hairline border and elevation (`.cm-menu-surface` in
 * `globals.css`), same 13px label rhythm, same mono keycaps. The geometry
 * lives here as class constants rather than in the shadcn primitives because
 * those primitives back ~20 other menus (model pickers, tab menus, resource
 * rows) whose dense lists should NOT grow to 32px rows. The container surface
 * is unified globally; the row rhythm is opt-in per menu.
 *
 * Every colour here is a design token or a token alpha mix.
 */

/** One 32px menu row: 8px radius, 9px side padding, 10px gap, 13px label, and
 *  a 14px muted icon in the leading slot. */
export const MENU_ROW =
  "h-8 gap-2.5 whitespace-nowrap rounded-[8px] px-[9px] text-[13px] [&>svg:first-child]:size-3.5 [&>svg:first-child]:shrink-0 [&>svg:first-child]:text-muted-foreground/70";

/** A row whose label wraps to a second, quieter description line. Taller than
 *  `MENU_ROW`, so the icon is top-aligned rather than centred. */
export const MENU_ROW_TWO_LINE =
  "items-start gap-2.5 rounded-[8px] px-[9px] py-2 text-[13px] [&>svg]:mt-0.5 [&>svg:first-child]:size-3.5 [&>svg:first-child]:shrink-0 [&>svg:first-child]:text-muted-foreground/70";

/** A 36px row carrying a title plus a mono state line (the device list). */
export const MENU_ROW_TWO_LINE_COMPACT =
  "h-9 gap-2.5 whitespace-nowrap rounded-[8px] px-[9px] text-[13px] [&>svg:first-child]:size-3.5 [&>svg:first-child]:shrink-0 [&>svg:first-child]:text-muted-foreground/70";

/**
 * The destructive tail row. Radix's highlight convention is a surface change,
 * so the red is carried by a red-tinted surface plus red ink rather than by an
 * inverted fill — an inverted destructive row reads as "already committed".
 * `!` because the menu container sets a neutral highlight fill on every
 * descendant item.
 */
export const MENU_ROW_DESTRUCTIVE = cn(
  MENU_ROW,
  "text-destructive [&>svg:first-child]:text-destructive! focus:text-destructive data-highlighted:text-destructive",
  "focus:bg-destructive/15! data-highlighted:bg-destructive/15!",
);

/** The 1px rule that still separates the destructive tail (and the gear
 *  menu's groups) where a section label would be too loud. */
export const MENU_SEPARATOR = "mx-[5px] my-1.5 h-px bg-border";

/**
 * A group heading inside a menu. Replaces bare dividers within a menu body:
 * a divider says "these are different", a label says what they are. Rendered
 * through the primitives' Label slot, so it is not a menuitem and keyboard
 * navigation skips it.
 */
export const MENU_SECTION_LABEL =
  "flex h-6 items-center px-[9px] font-mono text-[9px] font-normal uppercase tracking-[0.16em] text-muted-foreground/55";

/** Trailing mono metadata on a row (a username, a device state, a hint). */
export const MENU_ROW_META =
  "ml-auto shrink-0 font-mono text-[9.5px] text-muted-foreground/60";

const IS_MAC =
  typeof navigator !== "undefined" && /mac/i.test(navigator.platform);

/** Mac glyphs for the parts of a combo that have one. */
const MAC_GLYPHS: Record<string, string> = {
  ctrl: "⌘",
  cmd: "⌘",
  meta: "⌘",
  shift: "⇧",
  alt: "⌥",
  option: "⌥",
  backspace: "⌫",
  delete: "⌦",
  enter: "↵",
  escape: "⎋",
};

/** Canonical macOS modifier order (⌃⌥⇧⌘), so "Ctrl+Shift+P" reads "⇧⌘P". */
const MAC_MODIFIER_ORDER = ["⌃", "⌥", "⇧", "⌘"];

/**
 * Render a registry combo the way the host platform writes it. Elsewhere in
 * the app (command palette, shortcut editor) combos are shown verbatim as
 * `Ctrl+K`, which is right everywhere except macOS — where the same binding
 * is physically ⌘ and the plus-joined spelling reads as a foreign convention.
 * Non-mac therefore passes through untouched.
 */
export function formatKeyCombo(combo: string): string {
  if (!combo) return "";
  if (!IS_MAC) return combo;
  const parts = combo.split("+").map((p) => p.trim()).filter(Boolean);
  const mods: string[] = [];
  const rest: string[] = [];
  for (const part of parts) {
    const glyph = MAC_GLYPHS[part.toLowerCase()];
    if (glyph && MAC_MODIFIER_ORDER.includes(glyph)) mods.push(glyph);
    else rest.push(glyph ?? part);
  }
  mods.sort(
    (a, b) => MAC_MODIFIER_ORDER.indexOf(a) - MAC_MODIFIER_ORDER.indexOf(b),
  );
  return [...mods, ...rest].join("");
}

interface KeycapProps {
  /** A `keybind-registry` action id. Resolved through the user's overrides so
   *  a rebound shortcut shows the binding that actually fires. */
  actionId?: string;
  /**
   * A literal combo for a row whose gesture is handled locally rather than
   * through the registry. Only use it for a combo nothing else claims — a
   * display-only keycap that duplicates a registry binding for a different
   * action promises a gesture that does something else entirely, and it can't
   * follow a rebind the way `actionId` does.
   */
  keys?: string;
  className?: string;
}

/**
 * The one keycap used across every restyled menu. Right-aligned, mono, and
 * quiet enough that a menu where only half the rows have a binding still reads
 * as a calm column rather than a ragged one.
 */
export function MenuKeycap({ actionId, keys, className }: KeycapProps) {
  const { getKeysForAction } = useResolvedKeybinds();
  const resolved = actionId ? getKeysForAction(actionId) : keys;
  if (!resolved) return null;
  return (
    <kbd
      // Decorative: the combo is a reminder of a gesture, not part of what
      // the item is called, so it stays out of the row's accessible name.
      aria-hidden
      className={cn(
        "ml-auto shrink-0 font-mono text-[9.5px] font-normal tracking-normal text-muted-foreground/70",
        className,
      )}
    >
      {formatKeyCombo(resolved)}
    </kbd>
  );
}
