import {
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
} from "@/components/ui/dropdown-menu";
import { eyebrowVariants } from "@/components/ui/eyebrow";
import { EditorIcon } from "@/components/icons/editor-icon";
import { groupEditors } from "@/lib/editor-groups";
import { cn } from "@/lib/utils";
import type { EditorInfo } from "@/tauri/types";

/** The "choose an editor" menu body, grouped by editor family. */
export function EditorMenuItems({
  editors,
  defaultEditorId,
  onOpen,
  itemClassName,
}: {
  editors: EditorInfo[];
  defaultEditorId: string | null;
  onOpen: (editorId: string) => void;
  /** Row density override for menus opened from a denser surface. */
  itemClassName?: string;
}) {
  const groupedEditors = groupEditors(editors);
  const showGroupLabels = groupedEditors.length > 1;
  return groupedEditors.map((group, groupIdx) => (
    // Render each family as a labelled section using DropdownMenuGroup for
    // proper radix aria semantics. We only show a header + separator when
    // more than one group is present — a user with only VS Code installed
    // shouldn't see a lonely "VS Code family" header above a single item.
    <DropdownMenuGroup key={group.id}>
      {groupIdx > 0 && <DropdownMenuSeparator />}
      {showGroupLabels && (
        <DropdownMenuLabel className={eyebrowVariants({ tone: "muted" })}>
          {group.label}
        </DropdownMenuLabel>
      )}
      {group.editors.map((editor) => (
        <DropdownMenuItem
          key={editor.id}
          className={cn(itemClassName)}
          onClick={() => onOpen(editor.id)}
        >
          <EditorIcon id={editor.id} className="size-4" />
          <span>{editor.name}</span>
          {editor.id === defaultEditorId && (
            <span className="ml-auto text-caption text-muted-foreground">
              default
            </span>
          )}
        </DropdownMenuItem>
      ))}
    </DropdownMenuGroup>
  ));
}
