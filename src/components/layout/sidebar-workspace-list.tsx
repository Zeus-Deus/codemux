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
  type: "workspace" | "section";
  id: string;
  sourceSectionId: string | null;
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
    (workspaceId: string, sectionId: string | null) =>
      (e: React.DragEvent) => {
        setDragState({ type: "workspace", id: workspaceId, sourceSectionId: sectionId });
        e.dataTransfer.effectAllowed = "move";
        e.dataTransfer.setData("text/plain", workspaceId);
      },
    [],
  );

  const handleSectionDragStart = useCallback(
    (sectionId: string) => (e: React.DragEvent) => {
      setDragState({ type: "section", id: sectionId, sourceSectionId: null });
      e.dataTransfer.effectAllowed = "move";
      e.dataTransfer.setData("text/plain", sectionId);
    },
    [],
  );

  const computeWorkspaceDropTarget = useCallback(
    (clientY: number) => {
      const listEl = listRef.current;
      if (!listEl) return;
      const listRect = listEl.getBoundingClientRect();

      // Find which zone the cursor is in
      let targetZone: { el: HTMLElement; sectionId: string | null } | null = null;

      const unsortedZone = listEl.querySelector<HTMLElement>('[data-drop-zone="unsorted"]');
      if (unsortedZone) {
        const rect = unsortedZone.getBoundingClientRect();
        if (clientY >= rect.top && clientY <= rect.bottom) {
          targetZone = { el: unsortedZone, sectionId: null };
        }
      }

      if (!targetZone) {
        const sectionZones = listEl.querySelectorAll<HTMLElement>("[data-drop-zone-section]");
        for (const zone of sectionZones) {
          const rect = zone.getBoundingClientRect();
          if (clientY >= rect.top && clientY <= rect.bottom) {
            targetZone = { el: zone, sectionId: zone.dataset.dropZoneSection || null };
            break;
          }
        }
      }

      // Fallback: closest zone
      if (!targetZone) {
        const allZones: Array<{ el: HTMLElement; sectionId: string | null }> = [];
        if (unsortedZone) allZones.push({ el: unsortedZone, sectionId: null });
        const sectionZones = listEl.querySelectorAll<HTMLElement>("[data-drop-zone-section]");
        for (const zone of sectionZones) {
          allZones.push({ el: zone, sectionId: zone.dataset.dropZoneSection || null });
        }
        let closestDist = Infinity;
        for (const zone of allZones) {
          const rect = zone.el.getBoundingClientRect();
          const dist = Math.min(Math.abs(clientY - rect.top), Math.abs(clientY - rect.bottom));
          if (dist < closestDist) {
            closestDist = dist;
            targetZone = zone;
          }
        }
      }

      if (!targetZone) return;

      // Find insertion position within zone
      const rows = targetZone.el.querySelectorAll<HTMLElement>("[data-ws-id]");

      if (rows.length === 0) {
        dropTargetRef.current = { sectionId: targetZone.sectionId, index: 0 };
        const zoneRect = targetZone.el.getBoundingClientRect();
        setDropIndicatorY(zoneRect.bottom - listRect.top);
        return;
      }

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

      if (!closestEl) return;

      const indexInZone = parseInt(closestEl.dataset.wsIndex ?? "0", 10);
      const targetIndex = insertBefore ? indexInZone : indexInZone + 1;

      dropTargetRef.current = { sectionId: targetZone.sectionId, index: targetIndex };
      const elRect = closestEl.getBoundingClientRect();
      setDropIndicatorY((insertBefore ? elRect.top : elRect.bottom) - listRect.top);
    },
    [],
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

  const handleDragOver = useCallback(
    (e: React.DragEvent) => {
      if (!dragState || !listRef.current) return;
      e.preventDefault();
      e.dataTransfer.dropEffect = "move";

      if (dragState.type === "workspace") {
        computeWorkspaceDropTarget(e.clientY);
      } else if (dragState.type === "section") {
        computeSectionDropTarget(e.clientY, dragState.id);
      }
    },
    [dragState, computeWorkspaceDropTarget, computeSectionDropTarget],
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
              // moveWorkspaceToSection with null sectionId removes from section
              // Pass sectionId as empty string to signal "remove from section"
              // Actually the Rust side accepts Option, so we pass null via the command
              await moveWorkspaceToSection(ds.id, "" as string, null);
            }
            // Reorder unsorted workspaces
            if (appState) {
              const sections = appState.sections;
              const assignedIds = new Set(sections.flatMap((s) => s.workspace_ids));
              assignedIds.delete(ds.id);
              const unsortedIds = appState.workspaces
                .map((w) => w.workspace_id)
                .filter((id) => !assignedIds.has(id) && id !== ds.id);
              const idx = Math.min(dt.index, unsortedIds.length);
              unsortedIds.splice(idx, 0, ds.id);
              const sectionedIds = appState.workspaces
                .map((w) => w.workspace_id)
                .filter((id) => assignedIds.has(id));
              await reorderWorkspaces([...unsortedIds, ...sectionedIds]);
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
    [dragState, appState, sectionGroups, clearDrag],
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
          <div data-drop-zone="unsorted">
            {projectGroups.map((group) => (
              <SidebarMenu key={group.projectPath}>
                <SidebarProjectGroup
                  projectName={group.projectName}
                  projectPath={group.projectPath}
                  workspaces={group.workspaces}
                  activeWorkspaceId={activeWorkspaceId}
                  onWorkspaceDragStart={handleWorkspaceDragStart}
                  dragStateId={dragState?.id ?? null}
                />
              </SidebarMenu>
            ))}
          </div>

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
