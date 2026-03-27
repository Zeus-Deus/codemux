import { useState, useEffect } from "react";
import { SidebarWorkspaceRow } from "./sidebar-workspace-row";
import {
  ContextMenu,
  ContextMenuTrigger,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSub,
  ContextMenuSubTrigger,
  ContextMenuSubContent,
} from "@/components/ui/context-menu";
import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { ChevronRight, Plus, Check } from "lucide-react";
import { cn } from "@/lib/utils";
import { dbGetUiState, dbSetUiState } from "@/tauri/commands";
import { useUIStore } from "@/stores/ui-store";
import type { WorkspaceSnapshot } from "@/tauri/types";

const PROJECT_COLORS = [
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

function hexToRgba(hex: string, alpha: number): string {
  const r = parseInt(hex.slice(1, 3), 16);
  const g = parseInt(hex.slice(3, 5), 16);
  const b = parseInt(hex.slice(5, 7), 16);
  return `rgba(${r}, ${g}, ${b}, ${alpha})`;
}

interface Props {
  projectName: string;
  projectPath: string;
  workspaces: WorkspaceSnapshot[];
  activeWorkspaceId: string;
  onWorkspaceDragStart?: (workspaceId: string, projectPath: string | null) => (e: React.DragEvent) => void;
  onProjectDragStart?: (e: React.DragEvent) => void;
  dragStateId?: string | null;
}

export function SidebarProjectGroup({
  projectName,
  projectPath,
  workspaces,
  activeWorkspaceId,
  onWorkspaceDragStart,
  onProjectDragStart,
  dragStateId,
}: Props) {
  const [collapsed, setCollapsed] = useState(false);
  const [customColor, setCustomColor] = useState<string | null>(null);
  const setShowNewWorkspaceDialog = useUIStore((s) => s.setShowNewWorkspaceDialog);

  useEffect(() => {
    dbGetUiState(`collapsed:project:${projectPath}`).then((val) => {
      if (val === "true") setCollapsed(true);
    }).catch(() => {});
    dbGetUiState(`project.color:${projectPath}`).then((val) => {
      if (val) setCustomColor(val);
    }).catch(() => {});
  }, [projectPath]);

  const handleToggle = () => {
    const next = !collapsed;
    setCollapsed(next);
    dbSetUiState(`collapsed:project:${projectPath}`, String(next)).catch(console.error);
  };

  const handleColorChange = (color: string | null) => {
    setCustomColor(color);
    if (color) {
      dbSetUiState(`project.color:${projectPath}`, color).catch(console.error);
    } else {
      dbSetUiState(`project.color:${projectPath}`, "").catch(console.error);
    }
  };

  const hasColor = !!customColor;
  const letter = projectName.charAt(0).toUpperCase();

  return (
    <div className="py-1.5">
      {/* Project header */}
      <ContextMenu>
        <ContextMenuTrigger asChild>
          <div
            className="flex items-center w-full pl-3 pr-2 py-1.5 text-sm font-medium hover:bg-muted/50 transition-colors cursor-pointer"
            draggable={!!onProjectDragStart}
            onDragStart={onProjectDragStart}
            data-project-header-path={projectPath}
          >
            {/* Letter avatar — neutral by default, colored only if user picks one */}
            <div
              className={cn(
                "size-6 rounded flex items-center justify-center shrink-0 mr-2.5 text-xs font-medium border-[1.5px]",
                !hasColor && "bg-muted text-muted-foreground border-border",
              )}
              style={hasColor ? {
                borderColor: hexToRgba(customColor!, 0.6),
                backgroundColor: hexToRgba(customColor!, 0.15),
                color: customColor!,
              } : undefined}
            >
              {letter}
            </div>

            <Button
              variant="ghost"
              className="flex-1 justify-start gap-1.5 min-w-0 text-left h-auto p-0 hover:bg-transparent"
              onClick={handleToggle}
            >
              <span className="truncate text-foreground">{projectName}</span>
              <span className="text-xs text-muted-foreground tabular-nums font-normal">
                ({workspaces.length})
              </span>
            </Button>

            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  variant="ghost"
                  size="icon-xs"
                  className="shrink-0 ml-1"
                  aria-label="New workspace"
                  onClick={(e) => { e.stopPropagation(); setShowNewWorkspaceDialog(true, projectPath); }}
                >
                  <Plus className="h-4 w-4 text-muted-foreground" />
                </Button>
              </TooltipTrigger>
              <TooltipContent side="bottom" sideOffset={4}>
                New workspace
              </TooltipContent>
            </Tooltip>

            <Button
              variant="ghost"
              size="icon-xs"
              className="shrink-0 ml-1"
              onClick={(e) => { e.stopPropagation(); handleToggle(); }}
            >
              <ChevronRight
                className={cn(
                  "h-3.5 w-3.5 text-muted-foreground transition-transform duration-150",
                  !collapsed && "rotate-90",
                )}
              />
            </Button>
          </div>
        </ContextMenuTrigger>
        <ContextMenuContent>
          <ContextMenuSub>
            <ContextMenuSubTrigger>Change Color</ContextMenuSubTrigger>
            <ContextMenuSubContent className="w-36">
              <ContextMenuItem onClick={() => handleColorChange(null)}>
                <span className="size-3.5 rounded-full border border-border bg-background shrink-0 mr-2" />
                Default
                {!customColor && <Check className="ml-auto h-3.5 w-3.5" />}
              </ContextMenuItem>
              {PROJECT_COLORS.map((color) => (
                <ContextMenuItem key={color.value} onClick={() => handleColorChange(color.value)}>
                  <span
                    className="size-3.5 rounded-full shrink-0 mr-2 border border-border/50"
                    style={{ backgroundColor: color.value }}
                  />
                  {color.name}
                  {customColor === color.value && <Check className="ml-auto h-3.5 w-3.5" />}
                </ContextMenuItem>
              ))}
            </ContextMenuSubContent>
          </ContextMenuSub>
        </ContextMenuContent>
      </ContextMenu>

      {/* Workspace rows */}
      {!collapsed && workspaces.map((ws, idx) => (
        <div
          key={ws.workspace_id}
          data-ws-id={ws.workspace_id}
          data-ws-index={idx}
          draggable={!!onWorkspaceDragStart}
          onDragStart={onWorkspaceDragStart?.(ws.workspace_id, projectPath)}
          className={dragStateId === ws.workspace_id ? "opacity-40" : ""}
        >
          <SidebarWorkspaceRow
            workspace={ws}
            isActive={ws.workspace_id === activeWorkspaceId}
          />
        </div>
      ))}
    </div>
  );
}
