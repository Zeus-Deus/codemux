import { useCallback, useRef, useEffect } from "react";
import { useActiveWorkspace, useAppStore } from "@/stores/app-store";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { useUIStore } from "@/stores/ui-store";
import { dbGetUiState, dbSetUiState } from "@/tauri/commands";
import { TabBar } from "./tab-bar";
import { PresetBar } from "./preset-bar";
import { PaneContainer } from "./pane-container";
import { RightPanel } from "./right-panel";
import { DiffPane } from "@/components/diff/DiffPane";
import { DraftChatSurface } from "@/components/chat/DraftChatSurface";
import { EditorPane } from "@/components/editor/EditorPane";
import { OpenFlowWorkspace } from "@/components/openflow/openflow-workspace";
import { ProjectOnboarding } from "@/components/overlays/project-onboarding";

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
      className="w-1 shrink-0 cursor-col-resize bg-transparent hover:bg-foreground/20 data-[dragging=true]:bg-foreground/30 transition-colors"
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
  const onboardingProjectDir = useUIStore((s) => s.onboardingProjectDir);
  const setOnboardingProjectDir = useUIStore((s) => s.setOnboardingProjectDir);
  const rightPanelTab = useUIStore((s) =>
    activeWorkspace
      ? s.rightPanelTabs[activeWorkspace.workspace_id] ?? null
      : null,
  );
  const rightPanelWidth = useUIStore((s) => s.rightPanelWidth);

  // Auto-dismiss onboarding when a workspace is created through any path ("+", CLI, etc.).
  //
  // Subscribe to the COUNT (a primitive number), not the workspaces array.
  // The Rust backend rebuilds the snapshot on every emit, so the array ref
  // is fresh on every backend tick — agent tokens, git polls, hook events.
  // Subscribing to the array would re-run this entire `WorkspaceMain`
  // tree on every tick. Counting inside the selector returns a primitive
  // that compares with === and is stable across ticks unless workspaces
  // were actually added/removed.
  const onboardingMatchCount = useAppStore((s) => {
    if (!onboardingProjectDir) return 0;
    const ws = s.appState?.workspaces;
    if (!ws) return 0;
    let count = 0;
    for (const w of ws) {
      if (w.project_root === onboardingProjectDir || w.cwd === onboardingProjectDir) {
        count += 1;
      }
    }
    return count;
  });
  useEffect(() => {
    if (!onboardingProjectDir) return;
    if (onboardingMatchCount > 1) {
      setOnboardingProjectDir(null);
    }
  }, [onboardingProjectDir, onboardingMatchCount, setOnboardingProjectDir]);

  // Lazy-workspace-creation: when the flag is on and a client-side
  // chat draft is active, render the draft surface in place of the
  // workspace pane tree. PresetBar sits ABOVE the branch so it's
  // present on both drafts and real workspaces — a preset click on a
  // draft materialises the workspace via `materializeWithPreset`.
  const lazyEnabled = useFeatureFlags((s) => s.enableLazyWorkspaceCreation);
  // GUI chrome (Agent Chat Beta): the title bar absorbs the tab strip +
  // preset launcher, so those stacked rows drop here. Off ⇒ legacy chrome
  // renders byte-identical.
  const enableAgentChat = useFeatureFlags((s) => s.enableAgentChat);
  const activeDraftId = useChatDraftStore((s) => s.activeDraftId);
  const activeDraft = useChatDraftStore((s) =>
    s.activeDraftId ? s.draftsById[s.activeDraftId] ?? null : null,
  );
  if (lazyEnabled && activeDraftId && activeDraft) {
    return (
      <div className="flex-1 min-w-0 min-h-0 flex flex-col overflow-hidden">
        <PresetBar
          workspaceId={null}
          draftId={activeDraft.draftId}
          disabled={activeDraft.promoting}
        />
        <div className="flex-1 min-h-0 overflow-hidden">
          <DraftChatSurface />
        </div>
      </div>
    );
  }

  if (!activeWorkspace) return null;

  // Onboarding wizard — renders in the content area for first-time project setup
  const isOnboarding =
    onboardingProjectDir &&
    (activeWorkspace.project_root === onboardingProjectDir ||
      activeWorkspace.cwd === onboardingProjectDir);

  if (isOnboarding) {
    return (
      <ProjectOnboarding
        projectDir={onboardingProjectDir}
        tempWorkspaceId={activeWorkspace.workspace_id}
        onComplete={() => setOnboardingProjectDir(null)}
        onCancel={() => setOnboardingProjectDir(null)}
      />
    );
  }

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
      {/* Left: tab bar + preset bar + pane content. In GUI chrome the
          title bar hosts the tabs + launcher, so both rows are dropped. */}
      <div className="flex-1 min-w-0 min-h-0 flex flex-col overflow-hidden">
        {!enableAgentChat && <TabBar workspace={activeWorkspace} />}
        {!enableAgentChat && (
          <PresetBar workspaceId={activeWorkspace.workspace_id} />
        )}
        <div className="flex-1 min-h-0 overflow-hidden">
          {activeTab?.kind === "diff" ? (
            <DiffPane tabId={activeTab.tab_id} workspace={activeWorkspace} />
          ) : activeTab?.kind === "editor" ? (
            <EditorPane tabId={activeTab.tab_id} />
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
