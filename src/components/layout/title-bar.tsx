import { TitlebarActions, useTitlebarActionsWidth } from "@/contrib/react/titlebar-actions";
import {
  PanelLeft,
  PanelRight,
  ChevronDown,
  ExternalLink,
  Maximize2,
  MessageSquare,
  Minimize2,
} from "lucide-react";
import { WindowControls } from "./window-chrome";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { RemoteConnectionChip } from "@/components/remote/remote-connection-indicator";
import { ResourceMonitor } from "./resource-monitor";
import { TitleBarTabs } from "./title-bar-tabs";
import { AgentLauncher, DraftAgentLauncher } from "./agent-launcher";
import {
  WorkspaceDetailsHost,
  WorkspaceDetailsToggle,
} from "./workspace-details/workspace-details-panel";
import { EditorMenuItems } from "./editor-menu-items";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { resolveRightPanelWidth } from "@/lib/right-panel-width";
import {
  PANEL_CLUSTER_GAP,
  RIGHT_PANEL_RESIZER_REACH,
  WINDOW_CONTROLS_INSET,
  panelClusterRight,
  topRightReserve,
} from "@/lib/titlebar-geometry";
import { PaneActionButton } from "./right-panel/pane-actions";
import {
  BAND_ACTIVE_FILL,
  BAND_CONTROL_RADIUS,
} from "./titlebar-control-style";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import {
  useActiveWorkspace,
  useActiveWorkspaceId,
} from "@/stores/app-store";
import { useDraftGuiChrome, useGuiChrome } from "@/hooks/use-gui-chrome";
import {
  selectActiveDraft,
  useChatDraftStore,
} from "@/stores/chat-draft-store";
import { usePresetStore } from "@/hooks/use-preset-store";
import { useSidebarGapWidth } from "@/hooks/use-sidebar-gap-width";
import { useTitlebarPinsStore } from "@/stores/titlebar-pins-store";
import { RIGHT_PANEL_EMPTY, useUIStore } from "@/stores/ui-store";
import { selectUtilityPage } from "@/lib/utility-pages";
import { toast } from "@/lib/toast";
import { applyPreset } from "@/tauri/commands";
import { launchAgentChatPane } from "@/lib/agent-chat/launch-pane";
import { cn } from "@/lib/utils";
import { EditorIcon } from "@/components/icons/editor-icon";
import { PresetIcon } from "@/components/icons/preset-icon";
import type { TerminalPreset, WorkspaceSnapshot } from "@/tauri/types";
import { useEditorLauncher } from "@/hooks/use-editor-launcher";
import { useRootFontPx } from "@/hooks/use-root-font-px";
import {
  selectShowResourceMonitor,
  useSyncedSettingsStore,
} from "@/stores/synced-settings-store";

// ── IDE Launcher (legacy bar) ──
//
// The GUI chrome moved this into the Workspace details card's
// "Open in <editor>" row; the legacy h-9 bar keeps its labelled chip.

function IdeLauncher() {
  const { editors, defaultEditor, defaultEditorId, isLoading, open, unavailable } =
    useEditorLauncher();

  if (unavailable) return null;

  return (
    <div className="flex items-center">
      <Tooltip>
        <TooltipTrigger asChild>
          <button
            type="button"
            onClick={() => defaultEditor && open(defaultEditor.id)}
            disabled={isLoading || !defaultEditor}
            className={cn(
              "flex h-6 items-center gap-1 rounded-l-md border border-r-0 border-border/60 bg-secondary/50 px-2",
              "text-label font-medium transition-colors duration-150",
              "hover:bg-secondary hover:border-border",
              isLoading && "opacity-50 pointer-events-none",
            )}
          >
            {defaultEditor ? (
              <EditorIcon id={defaultEditor.id} className="size-3.5" />
            ) : (
              <ExternalLink className="size-3 shrink-0" />
            )}
            <span className="hidden sm:inline">
              {defaultEditor?.name ?? "Open"}
            </span>
          </button>
        </TooltipTrigger>
        <TooltipContent side="bottom">
          {defaultEditor ? `Open in ${defaultEditor.name}` : "Open in editor"}
        </TooltipContent>
      </Tooltip>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            disabled={isLoading}
            aria-label="Choose editor"
            className={cn(
              "flex h-6 w-5 items-center justify-center rounded-r-md border border-border/60 bg-secondary/50",
              "text-muted-foreground transition-colors duration-150",
              "hover:bg-secondary hover:border-border hover:text-foreground",
              isLoading && "opacity-50 pointer-events-none",
            )}
          >
            <ChevronDown className="size-3" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-48">
          <EditorMenuItems
            editors={editors}
            defaultEditorId={defaultEditorId}
            onOpen={open}
          />
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}

// ── Sidebar Toggle ──

function SidebarToggleButton({
  open,
  onToggle,
}: {
  open: boolean;
  onToggle: () => void;
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="Toggle sidebar"
          aria-pressed={open}
          onClick={onToggle}
          className="text-muted-foreground"
        >
          <PanelLeft className="size-3.5" />
        </Button>
      </TooltipTrigger>
      <TooltipContent side="bottom" shortcut="toggleSidebar">
        Toggle sidebar
      </TooltipContent>
    </Tooltip>
  );
}

// ── Fixed panel-control cluster ──
//
// The resource monitor, Workspace details, full-expand and the right-panel
// toggle, pinned to the window's top-right corner just inside the native
// window buttons, on one evenly spaced rhythm. This cluster does not move:
// panel open or closed, narrow or wide, it is in the same place. That is the
// whole point of it existing separately from the action island — the island
// stops at the panel's left edge (it belongs to the workspace column), and
// when the toggle rode along inside it, opening the panel teleported the
// button hundreds of pixels left and left a dead 40px strip above the
// panel's own tab row. See `src/lib/titlebar-geometry.ts`.

function RightPanelChromeCluster({
  workspaceId,
  remPx,
  pluginWidth,
}: {
  workspaceId: string;
  remPx: number;
  pluginWidth: number;
}) {
  const setRightPanelTab = useUIStore((s) => s.setRightPanelTab);
  const collapseRightPanel = useUIStore((s) => s.collapseRightPanel);
  const toggleMaximized = useUIStore((s) => s.toggleRightPanelMaximized);
  const maximized = useUIStore((s) => s.rightPanelMaximized);
  const rightPanelTab = useUIStore(
    (s) => s.rightPanelTabs[workspaceId] ?? null,
  );
  const open = rightPanelTab !== null;

  // True toggle. Opening lands on the picker sentinel, which resolves to
  // whatever panes the deck already has — and to the surface picker when it
  // has none, instead of force-opening Files over the user's last choice.
  // Collapsing goes through the store's one collapse path, which owns the
  // "a collapsed panel is not a surface" rule (it undocks the agent browser).
  const togglePanel = () => {
    if (open) collapseRightPanel(workspaceId);
    else setRightPanelTab(workspaceId, RIGHT_PANEL_EMPTY);
  };

  return (
    <div
      data-testid="titlebar-panel-cluster"
      className="pointer-events-auto absolute top-1 z-10 flex h-8 items-center gap-1"
      style={{ right: `${panelClusterRight(isRemoteClient(), remPx) + pluginWidth}px` }}
      onPointerDown={(e) => e.stopPropagation()}
    >
      {/* Leads the run so it keeps its place when the panel opens, rather
          than riding the workspace band to the panel's edge. */}
      <ResourceMonitor variant="ghost" />
      {/* Full-expand leaves no workspace column for the card to sit in. */}
      <WorkspaceDetailsToggle disabled={open && maximized} />
      {open && (
        <PaneActionButton
          label={maximized ? "Restore panel width" : "Expand panel"}
          icon={maximized ? Minimize2 : Maximize2}
          active={maximized}
          testId="right-panel-maximize"
          size="titlebar"
          onClick={() => toggleMaximized(workspaceId)}
        />
      )}
      <PaneActionButton
        label={open ? "Close panel" : "Open panel"}
        icon={PanelRight}
        active={open}
        testId="right-panel-toggle"
        size="titlebar"
        shortcut="toggleRightPanel"
        onClick={togglePanel}
      />
    </div>
  );
}

// ── Pinned preset tiles (GUI chrome) ──
//
// One 27px icon tile per preset the user has opted into via the title-bar
// pin toggle in the `+` launcher (`useTitlebarPinsStore` —
// src/stores/titlebar-pins-store.ts), NOT `preset.pinned` (that flag means
// "show in the legacy PresetBar" and is unrelated — nearly every built-in
// preset ships `pinned: true` for that bar, which is what used to flood
// this row with tiles by default). Default is an empty store: no tiles,
// no divider. chat_agent presets get the ember-tinted tile, cli presets
// get the neutral tile. Click launches exactly like the launcher row for
// that preset kind; Shift-click still splits for CLI presets. `+` remains
// the only way to reach everything else (and the only way to pin/unpin).

function errorMessage(err: unknown): string {
  return typeof err === "string"
    ? err
    : err instanceof Error
      ? err.message
      : String(err);
}

function PinnedPresetTile({
  preset,
  variant,
  onClick,
  testId,
}: {
  preset: TerminalPreset;
  variant: "ember" | "neutral";
  onClick: (e: React.MouseEvent) => void;
  testId: string;
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          onClick={onClick}
          data-testid={testId}
          aria-label={preset.name}
          className={cn(
            // 28px on the band's shared radius like every other control.
            // The tinted treatments stay: these tiles are shortcuts to
            // *launch* something, so unlike Run and the editor launcher
            // they keep a fill that says which agent family they belong to.
            "flex size-7 shrink-0 items-center justify-center transition-colors duration-150",
            BAND_CONTROL_RADIUS,
            variant === "ember"
              ? "border border-accent-ember/40 bg-accent-ember/14 text-accent-ember hover:bg-accent-ember/20"
              : "border border-border bg-secondary/50 text-muted-foreground hover:bg-secondary hover:text-foreground",
          )}
        >
          <PresetIcon icon={preset.icon} className="size-3.5" />
        </button>
      </TooltipTrigger>
      <TooltipContent side="bottom">
        {preset.name}
      </TooltipContent>
    </Tooltip>
  );
}

function PinnedPresetTiles({ workspace }: { workspace: WorkspaceSnapshot }) {
  const presetStore = usePresetStore();
  const pinnedIds = useTitlebarPinsStore((s) => s.pinnedIds);
  const presets = presetStore?.presets ?? [];
  const chatPresets = presets.filter(
    (p) => p.kind === "chat_agent" && pinnedIds.includes(p.id),
  );
  const pinnedCliPresets = presets.filter(
    (p) => p.kind === "cli" && pinnedIds.includes(p.id),
  );
  const workspaceId = workspace.workspace_id;

  // Same semantics as today's inline chat favorite: always a new tab,
  // Shift has no effect (chat_agent launches never split from here). The
  // preset arg is unused today (error text stays "Chat Agent" for every
  // chat_agent preset, matching the prior implementation) but kept in the
  // signature so each mapped tile still gets its own bound handler.
  const launchChat = (_preset: TerminalPreset) => () => {
    launchAgentChatPane(workspaceId, "claude", null, "new_tab").catch((err) => {
      toast.error(`Chat Agent: ${errorMessage(err)}`);
      console.error("[title-bar] chat favorite launch failed:", err);
    });
  };

  // Mirrors AgentLauncher's `launchCli`: plain click opens a new tab,
  // Shift-click splits the active surface.
  const launchCli = (preset: TerminalPreset) => (e: React.MouseEvent) => {
    const mode = e.shiftKey ? "split_pane" : "new_tab";
    const modelSelection = preset.launch_config?.model_selection ?? null;
    applyPreset(workspaceId, preset.id, mode, null, modelSelection).catch(
      (err) => {
        toast.error(`${preset.name}: ${errorMessage(err)}`);
        console.error("[title-bar] pinned preset launch failed:", err);
      },
    );
  };

  if (chatPresets.length === 0 && pinnedCliPresets.length === 0) return null;

  return (
    <>
      <div className="h-4 w-px shrink-0 bg-border/70" aria-hidden />
      {chatPresets.map((preset) => (
        <PinnedPresetTile
          key={preset.id}
          preset={preset}
          variant="ember"
          onClick={launchChat(preset)}
          testId={`titlebar-favorite-${preset.id}`}
        />
      ))}
      {pinnedCliPresets.map((preset) => (
        <PinnedPresetTile
          key={preset.id}
          preset={preset}
          variant="neutral"
          onClick={launchCli(preset)}
          testId={`titlebar-pin-${preset.id}`}
        />
      ))}
    </>
  );
}

// ── Workspace slots (GUI chrome) ──
//
// Self-subscribes to the active workspace so its per-tick re-render
// (fresh snapshot ref every backend emit) stays isolated to the tab
// strip + launcher + pinned tiles, never churning the window controls /
// resource monitor sitting in the sibling right cluster.
function TitleBarWorkspaceSlots() {
  const workspace = useActiveWorkspace();
  if (!workspace) return null;
  return (
    <div
      className="flex min-w-0 items-center gap-[2px]"
      onPointerDown={(e) => e.stopPropagation()}
    >
      <TitleBarTabs workspace={workspace} />
      <AgentLauncher workspace={workspace} />
      <PinnedPresetTiles workspace={workspace} />
    </div>
  );
}

// ── Draft slots (GUI chrome, lazy-creation draft) ──
//
// The draft counterpart of `TitleBarWorkspaceSlots`: while a chat draft
// is the active surface there is no workspace (so no backend tabs to
// pill-ify), just the one draft being composed. Render a single static
// "Agent Chat" pill in the active-tab style plus the draft `+` launcher
// (materialise-with-preset — the GUI replacement for the legacy draft
// PresetBar row). Keeps the titlebar silhouette identical to the
// post-materialise workspace chrome, so sending the first prompt no
// longer swaps the whole top bar.
function TitleBarDraftSlots() {
  const draft = useChatDraftStore(selectActiveDraft);
  if (!draft) return null;
  return (
    <div
      className="flex min-w-0 items-center gap-[2px]"
      onPointerDown={(e) => e.stopPropagation()}
    >
      <div
        data-testid="titlebar-draft-tab"
        // The draft's stand-in for the active tab, so it wears exactly the
        // active pill: same radius token, same selected fill.
        className={cn(
          "flex h-7 shrink-0 items-center gap-1.5 pl-2.5 pr-2.5 text-label font-semibold text-foreground",
          BAND_CONTROL_RADIUS,
          BAND_ACTIVE_FILL,
        )}
      >
        <MessageSquare className="size-3" />
        <span>Agent Chat</span>
      </div>
      <DraftAgentLauncher draft={draft} />
    </div>
  );
}

// ── Title Bar ──

interface TitleBarProps {
  sidebarOpen: boolean;
  onToggleSidebar: () => void;
}

/**
 * Any measured sidebar width at or below this is the collapsed icon rail
 * (3.25rem = 52px — see `SIDEBAR_WIDTH_ICON` in `ui/sidebar.tsx`), never an
 * expanded sidebar: `SidebarRail` drag and every preset keep expanded widths
 * far above it. Used to centre the floating sidebar toggle over the rail's
 * icon column.
 */
const COLLAPSED_RAIL_MAX_WIDTH = 64;

export function TitleBar({ sidebarOpen, onToggleSidebar }: TitleBarProps) {
  const pluginWidth = useTitlebarActionsWidth();
  const activeWorkspaceId = useActiveWorkspaceId();
  // Called unconditionally (Rules of Hooks) even though only the GUI-chrome
  // branch below renders the sidebar-matched left cluster that consumes it.
  const sidebarGapWidth = useSidebarGapWidth();

  // GUI chrome renders for a real workspace when the Agent
  // Chat Beta is on; a live lazy-creation draft renders the same h-10
  // shell with draft slots instead (mutually exclusive predicates — see
  // the hook doc comments). Shared with other GUI-mode-only surfaces via
  // `useGuiChrome`.
  const guiChrome = useGuiChrome();
  const draftGuiChrome = useDraftGuiChrome();
  // The *rendered* width, not the stored one: the panel's stored width can
  // exceed what fits in the current window (see `right-panel-width.ts`), and
  // the band has to stop at the panel's real edge.
  const rightPanelWidth = useUIStore((s) =>
    resolveRightPanelWidth(s.rightPanelWidth, s.rightPanelRowWidth ?? 0),
  );
  const activeWorkspacePanelOpen = useUIStore((s) =>
    activeWorkspaceId
      ? (s.rightPanelTabs[activeWorkspaceId] ?? null) !== null
      : false,
  );
  const rightPanelMaximized = useUIStore((s) => s.rightPanelMaximized);
  // A utility page (Pull requests, Devices, …) covers the workspace and
  // draws its own header in this band, so the workspace's tabs, actions
  // and panel controls step aside while it is open.
  const pageOpen = useUIStore((s) => selectUtilityPage(s) !== null);
  // The measured content row — the only honest width for "the panel covers
  // everything" while full-expand is on, since the panel then has no inline
  // width of its own.
  const rightPanelRowWidth = useUIStore((s) => s.rightPanelRowWidth);
  // During a lazy draft the backend's "active workspace" is whatever was
  // focused before the draft opened, and the draft surface never renders
  // that workspace's right panel. Reading its `rightPanelTabs` entry would
  // stop the floating band ~328px short of the right edge with nothing
  // below it — same reason the draft branch suppresses the other
  // workspace-scoped controls.
  const rightPanelOpen = guiChrome && activeWorkspacePanelOpen;
  const panelMaximized = rightPanelOpen && rightPanelMaximized;
  const remoteClient = isRemoteClient();
  // The corner's controls are rem-sized; its geometry scales with them.
  const remPx = useRootFontPx();
  const resourceMonitorShown = useSyncedSettingsStore(selectShowResourceMonitor);
  // How much of the band the panel owns, measured from the window's right
  // edge. Everything the *workspace* puts in the band has to stop here.
  const panelBandWidth = panelMaximized
    ? rightPanelRowWidth > 0
      ? rightPanelRowWidth
      : rightPanelWidth
    : rightPanelWidth;
  // The band's right inset, measured from the window's right edge.
  //
  // Panel closed, the workspace band stops one rhythm gap short of the
  // fixed panel cluster (`topRightReserve`), so its last control — the
  // resource monitor — continues the corner's evenly spaced run instead of
  // floating apart from it (see `TITLEBAR_RHYTHM_GAP`). Panel open, the
  // band tracks the panel's edge. A draft has no panel cluster to clear,
  // only the native window buttons.
  const normalBandRightInset = rightPanelOpen
    ? panelBandWidth + 8
    : guiChrome
      ? topRightReserve(remoteClient, {
          panelOpen: false,
          monitor: resourceMonitorShown,
          remPx,
        })
      : remoteClient
        ? 6
        : 104;
  const bandRightInset = pluginWidth > 0
    ? Math.max(normalBandRightInset, topRightReserve(remoteClient, { panelOpen: rightPanelOpen, monitor: resourceMonitorShown, remPx }) + pluginWidth)
    : normalBandRightInset;

  if (!guiChrome && !draftGuiChrome) {
    return (
      <div
        data-tauri-drag-region
        className="relative flex h-9 w-full shrink-0 items-center justify-between border-b border-border bg-card"
      >
        {/* Left */}
        <div className="flex items-center gap-1 pl-2">
          <SidebarToggleButton open={sidebarOpen} onToggle={onToggleSidebar} />
        </div>

        {/* Center — left intentionally empty so the title bar reads as
            a calm drag region. Command palette is reachable from the
            sidebar footer + its keyboard shortcut. */}

        {/* Right */}
        <div className="flex items-center gap-1.5 pr-0.5">
          <TitlebarActions />
          {/* Resource monitor — CPU/memory usage of Codemux + every
              terminal process tree. Self-gates on the
              appearance.show_resource_monitor setting and renders null
              when disabled. */}
          <ResourceMonitor />
          <IdeLauncher />
          {/* The separator only divides the content cluster from the native
              window controls — on the web remote client those controls render
              nothing, so the separator would dangle at the right edge. Hide it
              there. */}
          {!isRemoteClient() && (
            <Separator orientation="vertical" className="!h-4 !self-auto bg-border/50" />
          )}
          <WindowControls />
          {/* Web remote client only: the connection chip takes the slot the
              hidden window controls free up. Quiet while connected; the loud
              reconnecting/offline states live in the app-wide banner. */}
          {isRemoteClient() && <RemoteConnectionChip />}
        </div>
      </div>
    );
  }

  return (
    <div
      data-testid="floating-titlebar"
      className="pointer-events-none absolute inset-x-0 top-0 z-30 h-10"
    >
      {/* The unoccupied top edge remains a native drag target. The floating
          islands below sit above it and opt back into pointer events.

          Desktop only, and it stops at the right panel's left edge. The
          panel's tab row now lives *in* this band rather than 40px below
          it, so a full-width layer would sit on top of every tab, the `+`
          and the pane actions. The panel supplies its own drag surface in
          the gap after its tabs (see `pane-tab-strip.tsx`). On the web
          remote client the layer is dropped entirely: there
          `data-tauri-drag-region` does nothing at all, so it would be a
          pure 40px pointer sink that also ate wheel events over the
          transcript. */}
      {!remoteClient && (
        <div
          data-testid="titlebar-drag-layer"
          data-tauri-drag-region
          className="pointer-events-auto absolute inset-y-0 left-0"
          // Maximized, the panel owns everything right of the sidebar, so
          // the layer is sized from the left instead of trusting a row
          // measurement that may not have landed yet. An open utility page
          // gets the same: its header is its own drag surface, and a layer
          // over it would swallow clicks on its controls.
          style={
            panelMaximized || pageOpen
              ? { width: `${sidebarGapWidth}px` }
              : {
                  right: rightPanelOpen
                    ? `${panelBandWidth + RIGHT_PANEL_RESIZER_REACH}px`
                    : 0,
                }
          }
        />
      )}

      {/* Sidebar control island. The sidebar surface itself now reaches the
          top edge; its first local row reserves this small collision area.

          The collapsed sidebar is a 52px icon rail (`--sidebar-width-icon`)
          that centres its 28px controls, so the toggle joins that vertical
          axis instead of hugging the window edge 6px to its left — the
          floating button is the top of the rail's icon column and must
          read as part of it. Beside an expanded sidebar there is no icon
          axis to join and the toggle keeps its corner inset. */}
      <div
        data-testid="titlebar-sidebar-cluster"
        className="pointer-events-auto absolute top-1 z-10 flex h-8 items-center"
        style={{
          left: `${
            sidebarGapWidth <= COLLAPSED_RAIL_MAX_WIDTH
              ? Math.max(6, (sidebarGapWidth - 28) / 2)
              : 6
          }px`,
        }}
        onPointerDown={(e) => e.stopPropagation()}
      >
        <SidebarToggleButton open={sidebarOpen} onToggle={onToggleSidebar} />
      </div>

      {/* Workspace band. It spans only the space between the measured left
          sidebar and either the right panel or the fixed top-right cluster.
          Tabs and actions are separate islands with a calm drag region
          between.

          Full-expand hides it outright: the panel then owns the whole
          content row, so the workspace tabs would be labels for a column
          that is zero pixels wide, drawn on top of the panel's own tab row.
          Restoring brings them straight back. */}
      <div
        data-testid="titlebar-floating-band"
        className={cn(
          "absolute top-1 z-10 flex h-8 min-w-0 items-center gap-2",
          (panelMaximized || pageOpen) && "hidden",
        )}
        style={{
          left: `${sidebarGapWidth + 6}px`,
          right: `${bandRightInset}px`,
        }}
      >
        <div
          data-testid="titlebar-workspace-island"
          className="pointer-events-auto flex h-8 min-w-0 items-center"
          onPointerDown={(e) => e.stopPropagation()}
        >
          {guiChrome ? <TitleBarWorkspaceSlots /> : <TitleBarDraftSlots />}
        </div>

        {/* Calm drag gap between the two islands. Same web-client rule as
            the full-width layer above: keep the flex spacer for layout, but
            stay transparent to pointer events where there is no window to
            drag. Its floor is what keeps the band draggable once the tabs
            fill it; the tab island takes everything else and scrolls past
            that. */}
        <div
          data-testid="titlebar-drag-gap"
          data-tauri-drag-region={remoteClient ? undefined : true}
          className={cn(
            "min-w-16 flex-1 self-stretch",
            remoteClient ? "pointer-events-none" : "pointer-events-auto",
          )}
        />

        <div
          data-testid="titlebar-action-island"
          className={cn(
            "pointer-events-auto flex h-8 shrink-0 items-center",
            // GUI chrome: the same rhythm as the corner cluster it leads into.
            guiChrome ? "gap-1" : "gap-[2px]",
          )}
          onPointerDown={(e) => e.stopPropagation()}
        >
          {/* GUI chrome: Run and "Open in editor" moved into the Workspace
              details card, and the resource monitor into the fixed corner
              cluster, so only plugin actions are left here. */}
          {!guiChrome && <ResourceMonitor />}
          {remoteClient && <RemoteConnectionChip compact />}
        </div>
      </div>

      <TitlebarActions floating className="fixed top-1 z-40 h-8 rounded-md bg-card" style={{ right: panelClusterRight(remoteClient, remPx) - 4 }} />

      {/* Panel controls. Fixed to the window's top-right corner, just inside
          the native buttons — the one cluster in this band whose position
          never depends on the panel. Workspace-only: a draft renders no
          right panel, so there is nothing for it to control. */}
      {guiChrome && activeWorkspaceId && !pageOpen && (
        <RightPanelChromeCluster workspaceId={activeWorkspaceId} remPx={remPx} pluginWidth={pluginWidth} />
      )}

      {/* The Workspace details card, pinned to the workspace column's
          top-right corner (or, when the column is too narrow for it and a
          readable chat lane, a popover in that same corner). Hidden with
          the workspace column: under a utility page or a fully expanded
          panel there is nothing for it to sit beside. */}
      {guiChrome && activeWorkspaceId && !pageOpen && !panelMaximized && (
        <WorkspaceDetailsHost
          workspaceId={activeWorkspaceId}
          rightInset={rightPanelOpen ? panelBandWidth : 0}
          // Its right edge continues the line of the controls above it:
          // the window buttons' edge, the web client's cluster inset, or
          // the band's own 8px stop before an open panel.
          edgeGap={
            ((rightPanelOpen ? 8 : remoteClient ? PANEL_CLUSTER_GAP : WINDOW_CONTROLS_INSET) *
              remPx) /
            16
          }
          remPx={remPx}
          columnWidth={
            (rightPanelRowWidth > 0
              ? rightPanelRowWidth
              : (typeof window === "undefined" ? 0 : window.innerWidth) - sidebarGapWidth) -
            (rightPanelOpen ? panelBandWidth : 0)
          }
        />
      )}

      {/* Native window controls stay attached to the physical window edge,
          outside the workspace action island. */}
      {!remoteClient && (
        // The buttons are 28px; centring them in the band's 32px row keeps
        // them on the same axis as the panel controls beside them.
        <div className="pointer-events-auto absolute right-1 top-1 z-10 flex h-8 items-center">
          <div className="overflow-hidden rounded-md">
            <WindowControls />
          </div>
        </div>
      )}
    </div>
  );
}
