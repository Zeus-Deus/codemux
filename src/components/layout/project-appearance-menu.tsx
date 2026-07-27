import { Check, Image as ImageIcon } from "lucide-react";
import {
  ContextMenuItem,
  ContextMenuLabel,
  ContextMenuSeparator,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
} from "@/components/ui/context-menu";
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

  return (
    <ContextMenuSub>
      <ContextMenuSubTrigger aria-label={`Project ${projectName}`}>
        Project &ldquo;{projectName}&rdquo;
      </ContextMenuSubTrigger>
      <ContextMenuSubContent className="w-44">
        <ContextMenuItem onClick={onRequestImageDialog}>
          <ImageIcon className="mr-2 h-3.5 w-3.5" />
          {imageUrl ? "Change image…" : "Set image…"}
        </ContextMenuItem>

        <ContextMenuSeparator />
        <ContextMenuLabel className="text-[10px] font-medium uppercase tracking-wide text-muted-foreground/70">
          Color
        </ContextMenuLabel>
        <ContextMenuItem onClick={() => setColor(projectPath, null)}>
          <span className="mr-2 size-3.5 shrink-0 rounded-full border border-border bg-background" />
          Default
          {!customColor && <Check className="ml-auto h-3.5 w-3.5" />}
        </ContextMenuItem>
        {PROJECT_COLORS.map((color) => (
          <ContextMenuItem
            key={color.value}
            onClick={() => setColor(projectPath, color.value)}
          >
            <span
              className="mr-2 size-3.5 shrink-0 rounded-full border border-border/50"
              style={{ backgroundColor: color.value }}
            />
            {color.name}
            {customColor === color.value && (
              <Check className="ml-auto h-3.5 w-3.5" />
            )}
          </ContextMenuItem>
        ))}
      </ContextMenuSubContent>
    </ContextMenuSub>
  );
}
