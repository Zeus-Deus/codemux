import { useState, useRef, useCallback } from "react";
import {
  SidebarGroup,
  SidebarGroupContent,
} from "@/components/ui/sidebar";
import { useProjectGroupedWorkspaces, useAppStore } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import { SidebarProjectGroup } from "./sidebar-project-group";
import { NewWorkspaceDialog } from "@/components/overlays/new-workspace-dialog";
import { reorderWorkspaces } from "@/tauri/commands";

interface DragState {
  type: "workspace" | "project";
  id: string;
  sourceProjectPath: string | null;
}

interface DropTarget {
  index: number;
}

export function SidebarWorkspaceList() {
  const appState = useAppStore((s) => s.appState);
  const projectGroups = useProjectGroupedWorkspaces(appState?.workspaces ?? []);
  const activeWorkspaceId = appState?.active_workspace_id ?? "";
  const showDialog = useUIStore((s) => s.showNewWorkspaceDialog);
  const setShowDialog = useUIStore((s) => s.setShowNewWorkspaceDialog);

  const [dragState, setDragState] = useState<DragState | null>(null);
  const [dropIndicatorY, setDropIndicatorY] = useState<number | null>(null);
  const dropTargetRef = useRef<DropTarget | null>(null);
  const listRef = useRef<HTMLDivElement>(null);

  const clearDrag = useCallback(() => {
    setDragState(null);
    setDropIndicatorY(null);
    dropTargetRef.current = null;
  }, []);

  const handleWorkspaceDragStart = useCallback(
    (workspaceId: string, projectPath: string | null) =>
      (e: React.DragEvent) => {
        setDragState({ type: "workspace", id: workspaceId, sourceProjectPath: projectPath });
        e.dataTransfer.effectAllowed = "move";
        e.dataTransfer.setData("text/plain", workspaceId);
      },
    [],
  );

  const handleProjectDragStart = useCallback(
    (projectPath: string) => (e: React.DragEvent) => {
      setDragState({ type: "project", id: projectPath, sourceProjectPath: null });
      e.dataTransfer.effectAllowed = "move";
      e.dataTransfer.setData("text/plain", projectPath);
    },
    [],
  );

  const findInsertionPoint = useCallback(
    (rows: NodeListOf<HTMLElement>, clientY: number, listRect: DOMRect) => {
      if (rows.length === 0) return null;

      let closestEl: HTMLElement | null = null;
      let closestDist = Infinity;
      let insertBefore = true;

      for (const row of rows) {
        const rect = row.getBoundingClientRect();
        const midY = rect.top + rect.height / 2;
        const dist = Math.abs(clientY - midY);
        if (dist < closestDist) {
          closestDist = dist;
          closestEl = row;
          insertBefore = clientY < midY;
        }
      }

      if (!closestEl) return null;

      const indexInZone = parseInt(closestEl.dataset.wsIndex ?? "0", 10);
      const targetIndex = insertBefore ? indexInZone : indexInZone + 1;
      const elRect = closestEl.getBoundingClientRect();
      const indicatorY = (insertBefore ? elRect.top : elRect.bottom) - listRect.top;

      return { targetIndex, indicatorY };
    },
    [],
  );

  const computeWorkspaceDropTarget = useCallback(
    (clientY: number): "allowed" | "blocked" => {
      const listEl = listRef.current;
      if (!listEl) return "blocked";
      const listRect = listEl.getBoundingClientRect();

      const sourceProject = dragState?.sourceProjectPath;
      const projectZones = listEl.querySelectorAll<HTMLElement>("[data-drop-zone-project]");

      for (const zone of projectZones) {
        const rect = zone.getBoundingClientRect();
        if (clientY >= rect.top && clientY <= rect.bottom) {
          const zoneProject = zone.dataset.dropZoneProject;
          if (sourceProject !== null && zoneProject !== sourceProject) {
            dropTargetRef.current = null;
            setDropIndicatorY(null);
            return "blocked";
          }
          const rows = zone.querySelectorAll<HTMLElement>("[data-ws-id]");
          if (rows.length === 0) {
            dropTargetRef.current = { index: 0 };
            setDropIndicatorY(rect.bottom - listRect.top);
            return "allowed";
          }
          const result = findInsertionPoint(rows, clientY, listRect);
          if (!result) return "blocked";
          dropTargetRef.current = { index: result.targetIndex };
          setDropIndicatorY(result.indicatorY);
          return "allowed";
        }
      }

      // Fallback: find closest same-project zone
      let closestZone: HTMLElement | null = null;
      let closestDist = Infinity;
      for (const zone of projectZones) {
        const zoneProject = zone.dataset.dropZoneProject;
        if (sourceProject !== null && zoneProject !== sourceProject) continue;
        const rect = zone.getBoundingClientRect();
        const dist = Math.min(Math.abs(clientY - rect.top), Math.abs(clientY - rect.bottom));
        if (dist < closestDist) {
          closestDist = dist;
          closestZone = zone;
        }
      }

      if (!closestZone) {
        dropTargetRef.current = null;
        setDropIndicatorY(null);
        return "blocked";
      }

      const rows = closestZone.querySelectorAll<HTMLElement>("[data-ws-id]");
      if (rows.length === 0) {
        dropTargetRef.current = { index: 0 };
        const zoneRect = closestZone.getBoundingClientRect();
        setDropIndicatorY(zoneRect.bottom - listRect.top);
        return "allowed";
      }
      const result = findInsertionPoint(rows, clientY, listRect);
      if (!result) return "blocked";
      dropTargetRef.current = { index: result.targetIndex };
      setDropIndicatorY(result.indicatorY);
      return "allowed";
    },
    [dragState, findInsertionPoint],
  );

  const computeProjectDropTarget = useCallback(
    (clientY: number, draggedProjectPath: string) => {
      const listEl = listRef.current;
      if (!listEl) return;
      const listRect = listEl.getBoundingClientRect();
      const headers = listEl.querySelectorAll<HTMLElement>("[data-project-header-path]");
      if (headers.length === 0) return;

      let closestHeader: HTMLElement | null = null;
      let closestDist = Infinity;
      let insertBefore = true;

      for (const header of headers) {
        if (header.getAttribute("data-project-header-path") === draggedProjectPath) continue;
        const rect = header.getBoundingClientRect();
        const midY = rect.top + rect.height / 2;
        const dist = Math.abs(clientY - midY);
        if (dist < closestDist) {
          closestDist = dist;
          closestHeader = header;
          insertBefore = clientY < midY;
        }
      }

      if (!closestHeader) return;
      const targetPath = closestHeader.getAttribute("data-project-header-path") ?? "";
      const targetIdx = projectGroups.findIndex((g) => g.projectPath === targetPath);
      if (targetIdx < 0) return;

      const targetIndex = insertBefore ? targetIdx : targetIdx + 1;
      dropTargetRef.current = { index: targetIndex };
      const elRect = closestHeader.getBoundingClientRect();
      setDropIndicatorY((insertBefore ? elRect.top : elRect.bottom) - listRect.top);
    },
    [projectGroups],
  );

  const handleDragOver = useCallback(
    (e: React.DragEvent) => {
      if (!dragState || !listRef.current) return;
      e.preventDefault();

      if (dragState.type === "workspace") {
        const result = computeWorkspaceDropTarget(e.clientY);
        e.dataTransfer.dropEffect = result === "allowed" ? "move" : "none";
      } else if (dragState.type === "project") {
        e.dataTransfer.dropEffect = "move";
        computeProjectDropTarget(e.clientY, dragState.id);
      }
    },
    [dragState, computeWorkspaceDropTarget, computeProjectDropTarget],
  );

  const handleDrop = useCallback(
    async (e: React.DragEvent) => {
      e.preventDefault();
      const ds = dragState;
      const dt = dropTargetRef.current;
      if (!ds || !dt) {
        clearDrag();
        return;
      }

      try {
        if (ds.type === "workspace") {
          // Reorder within project group, then rebuild flat list
          const newIds: string[] = [];
          for (const group of projectGroups) {
            const groupIds = group.workspaces
              .map((w) => w.workspace_id)
              .filter((id) => id !== ds.id);

            if (group.projectPath === ds.sourceProjectPath) {
              const idx = Math.min(dt.index, groupIds.length);
              groupIds.splice(idx, 0, ds.id);
            }

            newIds.push(...groupIds);
          }

          if (!newIds.includes(ds.id)) {
            newIds.push(ds.id);
          }

          await reorderWorkspaces(newIds);
        } else if (ds.type === "project") {
          // Reorder project folders
          const currentPaths = projectGroups.map((g) => g.projectPath);
          const dragIdx = currentPaths.indexOf(ds.id);
          if (dragIdx >= 0 && dt.index !== undefined) {
            const newPaths = [...currentPaths];
            newPaths.splice(dragIdx, 1);
            const adjusted = dt.index > dragIdx ? dt.index - 1 : dt.index;
            newPaths.splice(Math.min(adjusted, newPaths.length), 0, ds.id);

            const groupMap = new Map(projectGroups.map((g) => [g.projectPath, g]));
            const newIds: string[] = [];
            for (const path of newPaths) {
              const group = groupMap.get(path);
              if (group) {
                newIds.push(...group.workspaces.map((w) => w.workspace_id));
              }
            }

            await reorderWorkspaces(newIds);
          }
        }
      } catch (err) {
        console.error("Drop failed:", err);
      }

      clearDrag();
    },
    [dragState, projectGroups, clearDrag],
  );

  const handleDragEnd = useCallback(() => {
    clearDrag();
  }, [clearDrag]);

  return (
    <SidebarGroup className="p-0">
      <SidebarGroupContent>
        <div
          ref={listRef}
          className="relative"
          onDragOver={handleDragOver}
          onDrop={handleDrop}
          onDragEnd={handleDragEnd}
        >
          {/* Drop indicator line */}
          {dropIndicatorY !== null && (
            <div
              className="absolute left-2 right-2 h-0.5 bg-primary rounded-full z-30 pointer-events-none"
              style={{ top: dropIndicatorY }}
            />
          )}

          {/* Workspaces grouped by project */}
          {projectGroups.map((group, idx) => (
            <div key={group.projectPath}>
              {idx > 0 && (
                <div className="mx-3 h-px bg-muted-foreground/20" />
              )}
              <div
                data-drop-zone-project={group.projectPath}
                className={dragState?.type === "project" && dragState.id === group.projectPath ? "opacity-40" : ""}
              >
                <SidebarProjectGroup
                  projectName={group.projectName}
                  projectPath={group.projectPath}
                  workspaces={group.workspaces}
                  activeWorkspaceId={activeWorkspaceId}
                  onWorkspaceDragStart={handleWorkspaceDragStart}
                  onProjectDragStart={handleProjectDragStart(group.projectPath)}
                  dragStateId={dragState?.id ?? null}
                />
              </div>
            </div>
          ))}
        </div>
      </SidebarGroupContent>
      <NewWorkspaceDialog open={showDialog} onOpenChange={setShowDialog} />
    </SidebarGroup>
  );
}
