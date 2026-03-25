import { useState, useRef, useCallback } from "react";
import {
  SidebarGroup,
  SidebarGroupContent,
  SidebarMenu,
} from "@/components/ui/sidebar";
import { useSectionedWorkspaces, useProjectGroupedWorkspaces, useAppStore } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import { SidebarSectionGroup } from "./sidebar-section-group";
import { SidebarProjectGroup } from "./sidebar-project-group";
import { NewWorkspaceDialog } from "@/components/overlays/new-workspace-dialog";
import {
  reorderWorkspaces,
  moveWorkspaceToSection,
  reorderSections,
} from "@/tauri/commands";

interface DragState {
  type: "workspace" | "section" | "project";
  id: string;
  sourceSectionId: string | null;
  sourceProjectPath: string | null;
}

interface DropTarget {
  sectionId: string | null;
  index: number;
}

export function SidebarWorkspaceList() {
  const { sectionGroups, unsorted } = useSectionedWorkspaces();
  const projectGroups = useProjectGroupedWorkspaces(unsorted);
  const appState = useAppStore((s) => s.appState);
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
    (workspaceId: string, sectionId: string | null, projectPath: string | null) =>
      (e: React.DragEvent) => {
        setDragState({ type: "workspace", id: workspaceId, sourceSectionId: sectionId, sourceProjectPath: projectPath });
        e.dataTransfer.effectAllowed = "move";
        e.dataTransfer.setData("text/plain", workspaceId);
      },
    [],
  );

  const handleSectionDragStart = useCallback(
    (sectionId: string) => (e: React.DragEvent) => {
      setDragState({ type: "section", id: sectionId, sourceSectionId: null, sourceProjectPath: null });
      e.dataTransfer.effectAllowed = "move";
      e.dataTransfer.setData("text/plain", sectionId);
    },
    [],
  );

  const handleProjectDragStart = useCallback(
    (projectPath: string) => (e: React.DragEvent) => {
      setDragState({ type: "project", id: projectPath, sourceSectionId: null, sourceProjectPath: null });
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

      // Check section zones first
      const sectionZones = listEl.querySelectorAll<HTMLElement>("[data-drop-zone-section]");
      for (const zone of sectionZones) {
        const rect = zone.getBoundingClientRect();
        if (clientY >= rect.top && clientY <= rect.bottom) {
          const rows = zone.querySelectorAll<HTMLElement>("[data-ws-id]");
          if (rows.length === 0) {
            dropTargetRef.current = { sectionId: zone.dataset.dropZoneSection || null, index: 0 };
            setDropIndicatorY(rect.bottom - listRect.top);
            return "allowed";
          }
          const result = findInsertionPoint(rows, clientY, listRect);
          if (!result) return "blocked";
          dropTargetRef.current = { sectionId: zone.dataset.dropZoneSection || null, index: result.targetIndex };
          setDropIndicatorY(result.indicatorY);
          return "allowed";
        }
      }

      // Check project zones — constrain to same project
      const sourceProject = dragState?.sourceProjectPath;
      const projectZones = listEl.querySelectorAll<HTMLElement>("[data-drop-zone-project]");

      for (const zone of projectZones) {
        const rect = zone.getBoundingClientRect();
        if (clientY >= rect.top && clientY <= rect.bottom) {
          const zoneProject = zone.dataset.dropZoneProject;
          // Block cross-project drops (unless coming from a section where sourceProjectPath is null)
          if (sourceProject !== null && zoneProject !== sourceProject) {
            dropTargetRef.current = null;
            setDropIndicatorY(null);
            return "blocked";
          }
          const rows = zone.querySelectorAll<HTMLElement>("[data-ws-id]");
          if (rows.length === 0) {
            dropTargetRef.current = { sectionId: null, index: 0 };
            setDropIndicatorY(rect.bottom - listRect.top);
            return "allowed";
          }
          const result = findInsertionPoint(rows, clientY, listRect);
          if (!result) return "blocked";
          dropTargetRef.current = { sectionId: null, index: result.targetIndex };
          setDropIndicatorY(result.indicatorY);
          return "allowed";
        }
      }

      // Fallback: find closest zone
      const allZones: Array<{ el: HTMLElement; sectionId: string | null }> = [];
      for (const zone of sectionZones) {
        allZones.push({ el: zone, sectionId: zone.dataset.dropZoneSection || null });
      }
      for (const zone of projectZones) {
        const zoneProject = zone.dataset.dropZoneProject;
        if (sourceProject !== null && zoneProject !== sourceProject) continue;
        allZones.push({ el: zone, sectionId: null });
      }

      let closestZone: { el: HTMLElement; sectionId: string | null } | null = null;
      let closestDist = Infinity;
      for (const zone of allZones) {
        const rect = zone.el.getBoundingClientRect();
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

      const rows = closestZone.el.querySelectorAll<HTMLElement>("[data-ws-id]");
      if (rows.length === 0) {
        dropTargetRef.current = { sectionId: closestZone.sectionId, index: 0 };
        const zoneRect = closestZone.el.getBoundingClientRect();
        setDropIndicatorY(zoneRect.bottom - listRect.top);
        return "allowed";
      }
      const result = findInsertionPoint(rows, clientY, listRect);
      if (!result) return "blocked";
      dropTargetRef.current = { sectionId: closestZone.sectionId, index: result.targetIndex };
      setDropIndicatorY(result.indicatorY);
      return "allowed";
    },
    [dragState, findInsertionPoint],
  );

  const computeSectionDropTarget = useCallback(
    (clientY: number, draggedId: string) => {
      const listEl = listRef.current;
      if (!listEl) return;
      const listRect = listEl.getBoundingClientRect();
      const headers = listEl.querySelectorAll<HTMLElement>("[data-section-header-id]");
      if (headers.length === 0) return;

      let closestHeader: HTMLElement | null = null;
      let closestDist = Infinity;
      let insertBefore = true;

      for (const header of headers) {
        if (header.getAttribute("data-section-header-id") === draggedId) continue;
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
      const sectionId = closestHeader.getAttribute("data-section-header-id") ?? "";
      const sectionIdx = sectionGroups.findIndex((g) => g.section.section_id === sectionId);
      if (sectionIdx < 0) return;

      const targetIndex = insertBefore ? sectionIdx : sectionIdx + 1;
      dropTargetRef.current = { sectionId: null, index: targetIndex };
      const elRect = closestHeader.getBoundingClientRect();
      setDropIndicatorY((insertBefore ? elRect.top : elRect.bottom) - listRect.top);
    },
    [sectionGroups],
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
      dropTargetRef.current = { sectionId: null, index: targetIndex };
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
      } else if (dragState.type === "section") {
        e.dataTransfer.dropEffect = "move";
        computeSectionDropTarget(e.clientY, dragState.id);
      } else if (dragState.type === "project") {
        e.dataTransfer.dropEffect = "move";
        computeProjectDropTarget(e.clientY, dragState.id);
      }
    },
    [dragState, computeWorkspaceDropTarget, computeSectionDropTarget, computeProjectDropTarget],
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
          if (dt.sectionId !== null) {
            await moveWorkspaceToSection(ds.id, dt.sectionId, dt.index);
          } else {
            // Move to unsorted
            if (ds.sourceSectionId !== null) {
              await moveWorkspaceToSection(ds.id, "" as string, null);
            }
            // Reorder within project group, then rebuild flat list
            if (appState) {
              const sections = appState.sections;
              const assignedIds = new Set(sections.flatMap((s) => s.workspace_ids));

              const newUnsortedIds: string[] = [];
              for (const group of projectGroups) {
                const groupIds = group.workspaces
                  .map((w) => w.workspace_id)
                  .filter((id) => !assignedIds.has(id) && id !== ds.id);

                if (ds.sourceProjectPath !== null && group.projectPath === ds.sourceProjectPath) {
                  const idx = Math.min(dt.index, groupIds.length);
                  groupIds.splice(idx, 0, ds.id);
                } else if (ds.sourceProjectPath === null) {
                  // Coming from a section — append to end of whichever group the cursor landed in
                  // (the workspace will be re-grouped by resolveProjectRoot on next render)
                  groupIds.push(ds.id);
                }

                newUnsortedIds.push(...groupIds);
              }

              // If workspace wasn't placed in any group (e.g. new project), append it
              if (!newUnsortedIds.includes(ds.id)) {
                newUnsortedIds.push(ds.id);
              }

              const sectionedIds = appState.workspaces
                .map((w) => w.workspace_id)
                .filter((id) => assignedIds.has(id));
              await reorderWorkspaces([...newUnsortedIds, ...sectionedIds]);
            }
          }
        } else if (ds.type === "project") {
          // Reorder project folders
          if (appState) {
            const sections = appState.sections;
            const assignedIds = new Set(sections.flatMap((s) => s.workspace_ids));

            const currentPaths = projectGroups.map((g) => g.projectPath);
            const dragIdx = currentPaths.indexOf(ds.id);
            if (dragIdx >= 0 && dt.index !== undefined) {
              const newPaths = [...currentPaths];
              newPaths.splice(dragIdx, 1);
              const adjusted = dt.index > dragIdx ? dt.index - 1 : dt.index;
              newPaths.splice(Math.min(adjusted, newPaths.length), 0, ds.id);

              const groupMap = new Map(projectGroups.map((g) => [g.projectPath, g]));
              const newUnsortedIds: string[] = [];
              for (const path of newPaths) {
                const group = groupMap.get(path);
                if (group) {
                  newUnsortedIds.push(...group.workspaces.map((w) => w.workspace_id));
                }
              }

              const sectionedIds = appState.workspaces
                .map((w) => w.workspace_id)
                .filter((id) => assignedIds.has(id));
              await reorderWorkspaces([...newUnsortedIds, ...sectionedIds]);
            }
          }
        } else if (ds.type === "section") {
          const currentOrder = sectionGroups.map((g) => g.section.section_id);
          const dragIdx = currentOrder.indexOf(ds.id);
          if (dragIdx >= 0 && dt.index !== undefined) {
            const newOrder = [...currentOrder];
            newOrder.splice(dragIdx, 1);
            const adjusted = dt.index > dragIdx ? dt.index - 1 : dt.index;
            newOrder.splice(Math.min(adjusted, newOrder.length), 0, ds.id);
            await reorderSections(newOrder);
          }
        }
      } catch (err) {
        console.error("Drop failed:", err);
      }

      clearDrag();
    },
    [dragState, appState, sectionGroups, projectGroups, clearDrag],
  );

  const handleDragEnd = useCallback(() => {
    clearDrag();
  }, [clearDrag]);

  return (
    <SidebarGroup>
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

          {/* Unsorted workspaces grouped by project */}
          {projectGroups.map((group) => (
            <div
              key={group.projectPath}
              data-drop-zone-project={group.projectPath}
              className={dragState?.type === "project" && dragState.id === group.projectPath ? "opacity-40" : ""}
            >
              <SidebarMenu>
                <SidebarProjectGroup
                  projectName={group.projectName}
                  projectPath={group.projectPath}
                  workspaces={group.workspaces}
                  activeWorkspaceId={activeWorkspaceId}
                  onWorkspaceDragStart={handleWorkspaceDragStart}
                  onProjectDragStart={handleProjectDragStart(group.projectPath)}
                  dragStateId={dragState?.id ?? null}
                />
              </SidebarMenu>
            </div>
          ))}

          {/* Section groups */}
          {sectionGroups.map((group) => (
            <div
              key={group.section.section_id}
              data-drop-zone-section={group.section.section_id}
              className={dragState?.type === "section" && dragState.id === group.section.section_id ? "opacity-40" : ""}
            >
              <SidebarMenu>
                <SidebarSectionGroup
                  section={group.section}
                  workspaces={group.workspaces}
                  activeWorkspaceId={activeWorkspaceId}
                  onWorkspaceDragStart={handleWorkspaceDragStart}
                  onSectionDragStart={handleSectionDragStart(group.section.section_id)}
                  dragStateId={dragState?.id ?? null}
                />
              </SidebarMenu>
            </div>
          ))}
        </div>
      </SidebarGroupContent>
      <NewWorkspaceDialog open={showDialog} onOpenChange={setShowDialog} />
    </SidebarGroup>
  );
}
