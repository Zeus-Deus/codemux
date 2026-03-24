import {
  SidebarMenuItem,
  SidebarMenuButton,
  SidebarMenuSub,
  SidebarMenuSubItem,
} from "@/components/ui/sidebar";
import {
  ContextMenu,
  ContextMenuTrigger,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuSub,
  ContextMenuSubTrigger,
  ContextMenuSubContent,
} from "@/components/ui/context-menu";
import { SidebarWorkspaceRow } from "./sidebar-workspace-row";
import { ChevronRight } from "lucide-react";
import {
  toggleSectionCollapsed,
  renameSection,
  deleteSection,
  setSectionColor,
} from "@/tauri/commands";
import { SECTION_PRESET_COLORS } from "@/tauri/types";
import type { WorkspaceSectionSnapshot, WorkspaceSnapshot } from "@/tauri/types";

interface Props {
  section: WorkspaceSectionSnapshot;
  workspaces: WorkspaceSnapshot[];
  activeWorkspaceId: string;
  onWorkspaceDragStart?: (workspaceId: string, sectionId: string | null) => (e: React.DragEvent) => void;
  onSectionDragStart?: (e: React.DragEvent) => void;
  dragStateId?: string | null;
}

export function SidebarSectionGroup({
  section,
  workspaces,
  activeWorkspaceId,
  onWorkspaceDragStart,
  onSectionDragStart,
  dragStateId,
}: Props) {
  const handleToggle = () => {
    toggleSectionCollapsed(section.section_id).catch(console.error);
  };

  const handleRename = () => {
    const newName = window.prompt("Rename section", section.name);
    if (newName && newName !== section.name) {
      renameSection(section.section_id, newName).catch(console.error);
    }
  };

  const handleDelete = () => {
    deleteSection(section.section_id).catch(console.error);
  };

  const handleColorChange = (color: string) => {
    setSectionColor(section.section_id, color).catch(console.error);
  };

  return (
    <SidebarMenuItem>
      <ContextMenu>
        <ContextMenuTrigger asChild>
          <SidebarMenuButton
            size="sm"
            onClick={handleToggle}
            className="gap-1.5"
            draggable={!!onSectionDragStart}
            onDragStart={onSectionDragStart}
            data-section-header-id={section.section_id}
          >
            <span
              className="inline-block h-1.5 w-1.5 shrink-0 rounded-full"
              style={{ backgroundColor: section.color }}
            />
            <ChevronRight
              className={`h-3 w-3 shrink-0 transition-transform ${
                !section.collapsed ? "rotate-90" : ""
              }`}
            />
            <span className="truncate text-[10px] font-normal uppercase tracking-widest text-muted-foreground/60">
              {section.name}
            </span>
            <span className="ml-auto text-[10px] tabular-nums text-muted-foreground/40">
              {workspaces.length}
            </span>
          </SidebarMenuButton>
        </ContextMenuTrigger>
        <ContextMenuContent>
          <ContextMenuItem onClick={handleRename}>
            Rename section
          </ContextMenuItem>
          <ContextMenuSub>
            <ContextMenuSubTrigger>Change color</ContextMenuSubTrigger>
            <ContextMenuSubContent>
              <div className="grid grid-cols-4 gap-1 p-1">
                {SECTION_PRESET_COLORS.map((color) => (
                  <button
                    key={color}
                    className="h-5 w-5 rounded-full border border-border hover:scale-110 transition-transform"
                    style={{ backgroundColor: color, outline: color === section.color ? "2px solid currentColor" : "none", outlineOffset: "2px" }}
                    onClick={() => handleColorChange(color)}
                  />
                ))}
              </div>
            </ContextMenuSubContent>
          </ContextMenuSub>
          <ContextMenuSeparator />
          <ContextMenuItem
            className="text-destructive focus:text-destructive"
            onClick={handleDelete}
          >
            Delete section
          </ContextMenuItem>
        </ContextMenuContent>
      </ContextMenu>
      {!section.collapsed && workspaces.length > 0 && (
        <SidebarMenuSub>
          {workspaces.map((ws, idx) => (
            <SidebarMenuSubItem key={ws.workspace_id}>
              <div
                data-ws-id={ws.workspace_id}
                data-ws-index={idx}
                draggable={!!onWorkspaceDragStart}
                onDragStart={onWorkspaceDragStart?.(ws.workspace_id, section.section_id)}
                className={dragStateId === ws.workspace_id ? "opacity-40" : ""}
              >
                <SidebarWorkspaceRow
                  workspace={ws}
                  isActive={ws.workspace_id === activeWorkspaceId}
                  nested
                />
              </div>
            </SidebarMenuSubItem>
          ))}
        </SidebarMenuSub>
      )}
    </SidebarMenuItem>
  );
}
