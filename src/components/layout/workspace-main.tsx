import { useCallback, useRef, useEffect } from "react";
import { useActiveWorkspace } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import { dbGetUiState, dbSetUiState } from "@/tauri/commands";
import { TabBar } from "./tab-bar";
import { PresetBar } from "./preset-bar";
import { PaneContainer } from "./pane-container";
import { RightPanel } from "./right-panel";
import { DiffPane } from "@/components/diff/DiffPane";
import { OpenFlowWorkspace } from "@/components/openflow/openflow-workspace";

const RIGHT_PANEL_MIN = 240;
const RIGHT_PANEL_MAX = 500;

function RightPanelResizer() {
  const setRightPanelWidth = useUIStore((s) => s.setRightPanelWidth);
  const handleRef = useRef<HTMLDivElement>(null);
  const rafId = useRef(0);

  const startResize = useCallback(
    (e: React.PointerEvent) => {
      e.preventDefault();
      const handle = handleRef.current;
      if (handle) handle.dataset.dragging = "true";

      // Find the right panel element (next sibling of the handle)
      const panelEl = handle?.nextElementSibling as HTMLElement | null;
      let lastWidth = 0;

      const onMove = (ev: PointerEvent) => {
        const width = Math.max(RIGHT_PANEL_MIN, Math.min(RIGHT_PANEL_MAX, window.innerWidth - ev.clientX));
        lastWidth = width;
        // Update DOM directly — no React re-render during drag
        cancelAnimationFrame(rafId.current);
        rafId.current = requestAnimationFrame(() => {
          if (panelEl) {
            panelEl.style.width = `${width}px`;
          }
        });
      };

      const onUp = () => {
        if (handle) handle.dataset.dragging = "false";
        cancelAnimationFrame(rafId.current);
        window.removeEventListener("pointermove", onMove);
        window.removeEventListener("pointerup", onUp);
        // Commit to React state + persist to SQLite (single re-render)
        if (lastWidth > 0) {
          setRightPanelWidth(lastWidth);
          dbSetUiState("right_panel_width", String(lastWidth)).catch(console.error);
        }
      };

      window.addEventListener("pointermove", onMove);
      window.addEventListener("pointerup", onUp);
    },
    [setRightPanelWidth],
  );

  return (
    <div
      ref={handleRef}
      className="w-1 shrink-0 cursor-col-resize bg-transparent hover:bg-primary/20 data-[dragging=true]:bg-primary/30 transition-colors"
      onPointerDown={startResize}
      role="separator"
      aria-orientation="vertical"
    />
  );
}

export function WorkspaceMain() {
  // Load persisted right panel width from SQLite on mount
  useEffect(() => {
    dbGetUiState("right_panel_width").then((val) => {
      if (val) useUIStore.getState().setRightPanelWidth(Number(val));
    }).catch(() => {});
  }, []);

  const activeWorkspace = useActiveWorkspace();
  const rightPanelTab = useUIStore((s) =>
    activeWorkspace
      ? s.rightPanelTabs[activeWorkspace.workspace_id] ?? null
      : null,
  );
  const rightPanelWidth = useUIStore((s) => s.rightPanelWidth);

  if (!activeWorkspace) return null;

  // OpenFlow workspaces get their own dedicated view
  if (activeWorkspace.workspace_type === "open_flow") {
    return (
      <>
        <TabBar workspace={activeWorkspace} />
        <div className="flex flex-1 min-h-0 min-w-0 overflow-hidden">
          <OpenFlowWorkspace workspace={activeWorkspace} />
        </div>
      </>
    );
  }

  const showRightPanel = rightPanelTab !== null;
  const activeTab = activeWorkspace.tabs.find(
    (t) => t.tab_id === activeWorkspace.active_tab_id,
  );

  return (
    <div className="flex flex-1 min-h-0 min-w-0 overflow-hidden">
      {/* Left: tab bar + preset bar + pane content */}
      <div className="flex-1 min-w-0 min-h-0 flex flex-col overflow-hidden">
        <TabBar workspace={activeWorkspace} />
        <PresetBar workspaceId={activeWorkspace.workspace_id} />
        <div className="flex-1 min-h-0 overflow-hidden">
          {activeTab?.kind === "diff" ? (
            <DiffPane tabId={activeTab.tab_id} workspace={activeWorkspace} />
          ) : (
            <PaneContainer workspace={activeWorkspace} />
          )}
        </div>
      </div>

      {/* Right: panel spans full height (header aligns with tab bar) */}
      {showRightPanel && (
        <>
          <RightPanelResizer />
          <div
            className="shrink-0 h-full overflow-hidden"
            style={{ width: rightPanelWidth }}
          >
            <RightPanel
              workspace={activeWorkspace}
              activeTab={rightPanelTab}
            />
          </div>
        </>
      )}
    </div>
  );
}
