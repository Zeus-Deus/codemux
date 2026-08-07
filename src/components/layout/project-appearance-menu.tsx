import { Image as ImageIcon } from "lucide-react";
import {
  ContextMenuItem,
  ContextMenuLabel,
  ContextMenuSeparator,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
} from "@/components/ui/context-menu";
import {
  MENU_ROW,
  MENU_SECTION_LABEL,
  MENU_SEPARATOR,
} from "@/components/ui/menu-chrome";
import { cn } from "@/lib/utils";
import { useProjectAppearance } from "./use-project-appearance";
import { useProjectAppearanceStore } from "@/stores/project-appearance-store";

/** The fixed accent palette offered for project avatars. Values are literal
 *  hex rather than theme tokens on purpose: this is user-chosen project
 *  identity (like a folder color), not themed UI chrome, so it must stay
 *  stable across light/dark and every theme variant. */
export const PROJECT_COLORS = [
  { name: "Red", value: "#ef4444" },
  { name: "Orange", value: "#f97316" },
  { name: "Yellow", value: "#eab308" },
  { name: "Lime", value: "#84cc16" },
  { name: "Green", value: "#22c55e" },
  { name: "Teal", value: "#14b8a6" },
  { name: "Cyan", value: "#06b6d4" },
  { name: "Blue", value: "#3b82f6" },
  { name: "Indigo", value: "#6366f1" },
  { name: "Purple", value: "#a855f7" },
  { name: "Pink", value: "#ec4899" },
  { name: "Slate", value: "#64748b" },
];

interface Props {
  /** Display name of the project this workspace belongs to — used as the
   *  submenu label so the menu states which project it will affect. */
  projectName: string;
  /** Absolute project root path — the key every appearance value is stored
   *  under, and what identifies the project to the image dialog. */
  projectPath: string;
  /** Open the image/URL dialog. The dialog is owned by the menu's parent
   *  because it must live outside the `ContextMenu` subtree — the context
   *  menu unmounts on select, which would take the dialog with it. */
  onRequestImageDialog: () => void;
}

/**
 * The project-level section of a workspace context menu: set a custom image
 * (direct URL, data URL, or any website whose favicon is derived) and pick an
 * accent color.
 *
 * Both settings apply to the whole **project**, not the single workspace that
 * was right-clicked — hence the project name in the submenu label. This is the
 * re-homed version of the old sidebar project-group menu, which went
 * unreachable when the nested project tree was replaced by the flat workspace
 * inbox.
 */
export function ProjectAppearanceMenu({
  projectName,
  projectPath,
  onRequestImageDialog,
}: Props) {
  const { customColor, imageUrl } = useProjectAppearance(projectPath);
  const setColor = useProjectAppearanceStore((s) => s.setColor);

  const selected =
    PROJECT_COLORS.find((c) => c.value === customColor) ?? null;

  return (
    <ContextMenuSub>
      <ContextMenuSubTrigger
        aria-label={`Project ${projectName}`}
        className={MENU_ROW}
      >
        <ImageIcon />
        <span className="min-w-0 flex-1 truncate">
          Project &ldquo;{projectName}&rdquo;
        </span>
        {/* The current accent, so the submenu's answer is visible without
            opening it. */}
        <span
          aria-hidden
          className={cn(
            "size-[9px] shrink-0 rounded-[3px]",
            !selected && "bg-foreground/20",
          )}
          style={selected ? { backgroundColor: selected.value } : undefined}
        />
      </ContextMenuSubTrigger>
      <ContextMenuSubContent sideOffset={-4} alignOffset={-6} className="w-[274px]">
        <ContextMenuItem className={MENU_ROW} onClick={onRequestImageDialog}>
          <ImageIcon />
          <span className="flex-1">
            {imageUrl ? "Change image…" : "Set image…"}
          </span>
        </ContextMenuItem>

        <ContextMenuSeparator className={MENU_SEPARATOR} />
        <ContextMenuLabel className={MENU_SECTION_LABEL}>Color</ContextMenuLabel>
        {/* A grid, not thirteen rows. The colours are the whole content of
            this choice, so showing them as a palette lets the eye pick one
            directly instead of reading a list of colour names — and it turns
            a submenu that used to be taller than its parent into four lines.
            Each swatch stays a real menuitem, so arrow-key navigation and the
            colour's accessible name both survive the change. */}
        <div className="grid grid-cols-7 gap-1.5 px-2 pt-1 pb-2">
          <ColorSwatch
            name="Default"
            selected={!customColor}
            onSelect={() => setColor(projectPath, null)}
          />
          {PROJECT_COLORS.map((color) => (
            <ColorSwatch
              key={color.value}
              name={color.name}
              value={color.value}
              selected={customColor === color.value}
              onSelect={() => setColor(projectPath, color.value)}
            />
          ))}
        </div>
        <div className="flex items-center gap-[7px] px-[9px] pt-0.5 pb-2">
          <span
            aria-hidden
            className={cn(
              "size-2 shrink-0 rounded-[3px]",
              !selected && "bg-foreground/20",
            )}
            style={selected ? { backgroundColor: selected.value } : undefined}
          />
          <span className="text-[11.5px] text-muted-foreground">
            {selected?.name ?? "Default"}
          </span>
          <span className="flex-1" />
          {/* Says out loud what the submenu title only implies: this is a
              project setting reached from one workspace's row. */}
          <span className="font-mono text-[9.5px] text-muted-foreground/60">
            applies to all workspaces
          </span>
        </div>
      </ContextMenuSubContent>
    </ContextMenuSub>
  );
}

/** One swatch in the colour grid. A real menuitem (keyboard-navigable) whose
 *  accessible name is the colour, since the tile itself carries no text. */
function ColorSwatch({
  name,
  value,
  selected,
  onSelect,
}: {
  name: string;
  /** Absent for "Default", which clears the custom colour. */
  value?: string;
  selected: boolean;
  onSelect: () => void;
}) {
  return (
    <ContextMenuItem
      aria-label={name}
      title={name}
      data-selected={selected ? "true" : undefined}
      onClick={onSelect}
      className={cn(
        "h-[26px] justify-center rounded-[7px] p-0",
        // Selection is a ring held off the tile by a popover-coloured gap, so
        // it reads on a dark swatch and a light one alike — a check glyph
        // would have to fight whichever colour it lands on. The keyboard/hover
        // state is the same ring in a quieter tone, so a swatch under the
        // cursor is never mistaken for the chosen one.
        selected
          ? "shadow-[0_0_0_2px_var(--popover),0_0_0_3.5px_var(--foreground)]"
          : "shadow-[inset_0_0_0_1px_color-mix(in_oklch,var(--foreground)_7%,transparent)]",
        "data-highlighted:shadow-[0_0_0_2px_var(--popover),0_0_0_3.5px_var(--muted-foreground)]",
        "focus:shadow-[0_0_0_2px_var(--popover),0_0_0_3.5px_var(--muted-foreground)]",
      )}
      style={{
        // "Default" has no chosen colour, so it shows the neutral the avatar
        // falls back to. Inline rather than a utility so the menu's shared
        // highlight fill can't repaint a swatch on hover.
        backgroundColor:
          value ?? "color-mix(in oklch, var(--foreground) 22%, transparent)",
      }}
    />
  );
}
