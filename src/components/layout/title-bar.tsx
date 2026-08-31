import {
  useState,
  useEffect,
  useCallback,
  useLayoutEffect,
  useRef,
  useSyncExternalStore,
} from "react";
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
import { RunButton } from "./run-button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { groupEditors } from "@/lib/editor-groups";
import { clampRightPanelWidth } from "@/lib/right-panel-width";
import { panelClusterRight, topRightReserve } from "@/lib/titlebar-geometry";
import { PaneActionButton } from "./right-panel/pane-actions";
import {
  BAND_ACTIVE_FILL,
  BAND_CONTROL_HOVER,
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
  useAppStore,
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
import { toast } from "@/lib/toast";
import {
  agentChatCreatePane,
  applyPreset,
  openInEditor,
} from "@/tauri/commands";
import { cn } from "@/lib/utils";
import {
  getTitlebarContentUnder,
  getTitlebarTranscriptElements,
  getTitlebarTranscriptVersion,
  subscribeTitlebarContentUnder,
  subscribeTitlebarTranscripts,
} from "@/lib/titlebar-content-under";
import { EditorIcon } from "@/components/icons/editor-icon";
import { PresetIcon } from "@/components/icons/preset-icon";
import { useSyncedSettingsStore, selectDefaultEditor } from "@/stores/synced-settings-store";
import type { TerminalPreset, WorkspaceSnapshot } from "@/tauri/types";
import { useDetectedEditors } from "@/stores/editor-discovery-store";

// ── IDE Launcher ──

interface IdeLauncherProps {
  /** GUI chrome renders the compressed [icon square][caret] shape — no
   *  editor-name label — to save horizontal room in the h-10 bar. The
   *  legacy h-9 bar keeps the default (labelled) shape unchanged, so this
   *  defaults to `false` and every existing call site stays byte-identical. */
  compact?: boolean;
}

function IdeLauncher({ compact = false }: IdeLauncherProps) {
  const editors = useDetectedEditors();
  const [isLoading, setIsLoading] = useState(false);
  const persistedEditor = useSyncedSettingsStore(selectDefaultEditor);
  const activeWorkspace = useAppStore(
    (s) =>
      s.appState?.workspaces.find(
        (w) => w.workspace_id === s.appState?.active_workspace_id,
      ),
  );

  useEffect(() => {
    if (
      editors.length > 0 &&
      !persistedEditor &&
      !useSyncedSettingsStore.getState().isLoading
    ) {
      void useSyncedSettingsStore
        .getState()
        .updateSetting("editor", "default_ide", editors[0].id);
    }
  }, [editors, persistedEditor]);

  const workspacePath = activeWorkspace?.cwd;
  const defaultEditorId = persistedEditor || (editors.length > 0 ? editors[0].id : null);

  const handleOpen = useCallback(
    (editorId: string) => {
      if (!workspacePath || isLoading) return;
      setIsLoading(true);
      useSyncedSettingsStore.getState().updateSetting("editor", "default_ide", editorId);
      openInEditor(editorId, workspacePath).finally(() => setIsLoading(false));
    },
    [workspacePath, isLoading],
  );

  const defaultEditor = editors.find((e) => e.id === defaultEditorId);
  const groupedEditors = groupEditors(editors);
  const showGroupLabels = groupedEditors.length > 1;

  if (editors.length === 0 || !workspacePath) return null;

  const mainTooltip = compact
    ? defaultEditor
      ? `Open in editor: ${defaultEditor.name} — click to change`
      : "Open in editor"
    : defaultEditor
      ? `Open in ${defaultEditor.name}`
      : "Open in editor";

  return (
    // Compact keeps the band's 2px rhythm between the icon and its caret;
    // the legacy shape's two halves are one joined chip, so no gap there.
    <div className={cn("flex items-center", compact && "gap-[2px]")}>
      <Tooltip>
        <TooltipTrigger asChild>
          <button
            type="button"
            onClick={() => defaultEditor && handleOpen(defaultEditor.id)}
            disabled={isLoading || !defaultEditor}
            className={cn(
              "flex items-center gap-1",
              // `compact` (GUI-chrome only) joins the band's one control
              // family: 28px tall on the shared radius token, with no
              // border and no resting fill — a hover fill is the whole
              // treatment, same as the panel toggle. Non-compact (legacy
              // bar) keeps its bordered chip and stays byte-identical.
              !compact && "border border-r-0 bg-secondary/50",
              "text-xs font-medium",
              compact
                ? cn("h-7 w-7 justify-center px-0", BAND_CONTROL_RADIUS)
                : "h-6 rounded-l-md border-border/60 px-2",
              compact ? BAND_CONTROL_HOVER : "transition-colors duration-150",
              !compact && "hover:bg-secondary hover:border-border",
              isLoading && "opacity-50 pointer-events-none",
            )}
          >
            {defaultEditor ? (
              <EditorIcon id={defaultEditor.id} className="h-3.5 w-3.5" />
            ) : (
              <ExternalLink className="h-3 w-3 shrink-0" />
            )}
            {!compact && (
              <span className="hidden sm:inline">
                {defaultEditor?.name ?? "Open"}
              </span>
            )}
          </button>
        </TooltipTrigger>
        <TooltipContent side="bottom" sideOffset={4}>
          {mainTooltip}
        </TooltipContent>
      </Tooltip>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            disabled={isLoading}
            className={cn(
              "flex items-center justify-center",
              !compact && "border bg-secondary/50",
              "text-muted-foreground",
              compact
                ? cn("h-7 w-5", BAND_CONTROL_RADIUS)
                : "h-6 w-5 rounded-r-md border-border/60",
              compact ? BAND_CONTROL_HOVER : "transition-colors duration-150",
              !compact && "hover:bg-secondary hover:border-border hover:text-foreground",
              isLoading && "opacity-50 pointer-events-none",
            )}
          >
            <ChevronDown className="h-3 w-3" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-48">
          {groupedEditors.map((group, groupIdx) => (
            // Render each family as a labelled section using
            // DropdownMenuGroup for proper radix aria semantics. We only
            // show a header + separator when more than one group is
            // present — a user with only VS Code installed shouldn't
            // see a lonely "VS Code family" header above a single item.
            <DropdownMenuGroup key={group.id}>
              {groupIdx > 0 && <DropdownMenuSeparator />}
              {showGroupLabels && (
                <DropdownMenuLabel className="text-[10px] font-medium uppercase tracking-wide text-muted-foreground/70">
                  {group.label}
                </DropdownMenuLabel>
              )}
              {group.editors.map((editor) => (
                <DropdownMenuItem
                  key={editor.id}
                  onClick={() => handleOpen(editor.id)}
                >
                  <EditorIcon id={editor.id} className="h-4 w-4" />
                  <span>{editor.name}</span>
                  {editor.id === defaultEditorId && (
                    <span className="ml-auto text-[10px] text-muted-foreground">
                      default
                    </span>
                  )}
                </DropdownMenuItem>
              ))}
            </DropdownMenuGroup>
          ))}
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
          <PanelLeft className="h-3.5 w-3.5" />
        </Button>
      </TooltipTrigger>
      <TooltipContent side="bottom" sideOffset={4}>
        Toggle sidebar (Ctrl+B)
      </TooltipContent>
    </Tooltip>
  );
}

// ── Fixed panel-control cluster ──
//
// Full-expand + the right-panel toggle, pinned to the window's top-right
// corner just inside the native window buttons. This cluster does not move:
// panel open or closed, narrow or wide, it is in the same place. That is the
// whole point of it existing separately from the action island — the island
// stops at the panel's left edge (it belongs to the workspace column), and
// when the toggle rode along inside it, opening the panel teleported the
// button hundreds of pixels left and left a dead 40px strip above the
// panel's own tab row. See `src/lib/titlebar-geometry.ts`.

function RightPanelChromeCluster({ workspaceId }: { workspaceId: string }) {
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
      className="pointer-events-auto absolute top-1 z-10 flex h-8 items-center gap-[2px]"
      style={{ right: `${panelClusterRight(isRemoteClient())}px` }}
      onPointerDown={(e) => e.stopPropagation()}
    >
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
            "flex h-7 w-7 shrink-0 items-center justify-center transition-colors",
            BAND_CONTROL_RADIUS,
            variant === "ember"
              ? "border border-accent-ember/40 bg-accent-ember/14 text-accent-ember hover:bg-accent-ember/20"
              : "border border-border bg-secondary/50 text-muted-foreground hover:bg-secondary hover:text-foreground",
          )}
        >
          <PresetIcon icon={preset.icon} className="h-3.5 w-3.5" />
        </button>
      </TooltipTrigger>
      <TooltipContent side="bottom" sideOffset={4}>
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
    agentChatCreatePane(workspaceId, "claude", null, "new_tab").catch((err) => {
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
        // active pill: same radius token, same 6% selected fill.
        className={cn(
          "flex h-7 shrink-0 items-center gap-1.5 pl-2.5 pr-2.5 text-xs font-semibold text-foreground",
          BAND_CONTROL_RADIUS,
          BAND_ACTIVE_FILL,
        )}
      >
        <MessageSquare className="h-3 w-3" />
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

const CHAT_READING_COLUMN_MAX_WIDTH = 792;

/**
 * Any measured sidebar width at or below this is the collapsed icon rail
 * (3.25rem = 52px — see `SIDEBAR_WIDTH_ICON` in `ui/sidebar.tsx`), never an
 * expanded sidebar: `SidebarRail` drag and every preset keep expanded widths
 * far above it. Used to centre the floating sidebar toggle over the rail's
 * icon column.
 */
const COLLAPSED_RAIL_MAX_WIDTH = 64;

/**
 * Measure the real transcript and control rectangles. This keeps the raised
 * treatment tied to an actual horizontal collision instead of a viewport
 * breakpoint that would style windows where the controls still sit safely
 * outside the centered reading column.
 *
 * `transcriptVersion` is the live-registry counter from
 * `titlebar-content-under.ts`. It MUST stay in the dependency list: the
 * measured node set is a snapshot, and `PaneContainer` renders only the
 * active surface, so every tab / workspace switch destroys the observed
 * transcript and mounts a new one. Without re-keying on the registry the
 * effect would keep observing a detached node and `overlapsChat` could
 * never become true again after the first navigation.
 */
function useTitlebarChatOverlap(enabled: boolean, transcriptVersion: number) {
  const workspaceIslandRef = useRef<HTMLDivElement | null>(null);
  const actionIslandRef = useRef<HTMLDivElement | null>(null);
  const [overlapsChat, setOverlapsChat] = useState(false);

  useLayoutEffect(() => {
    if (!enabled) {
      setOverlapsChat(false);
      return;
    }

    // Registered viewports are the live source of truth; the DOM query is
    // kept as a superset so a transcript rendered by a path that never
    // registers is still measured on the runs that do happen.
    const transcripts = Array.from(
      new Set<HTMLElement>([
        ...getTitlebarTranscriptElements(),
        ...document.querySelectorAll<HTMLElement>(
          '[data-slot="transcript-list"]',
        ),
      ]),
    );
    const islands = [workspaceIslandRef.current, actionIslandRef.current].filter(
      (element): element is HTMLDivElement => element !== null,
    );

    const update = () => {
      const next = transcripts.some((transcript) => {
        const transcriptRect = transcript.getBoundingClientRect();
        if (transcriptRect.width <= 0 || transcriptRect.height <= 0) return false;
        const columnWidth = Math.min(
          CHAT_READING_COLUMN_MAX_WIDTH,
          transcriptRect.width,
        );
        const columnLeft =
          transcriptRect.left + (transcriptRect.width - columnWidth) / 2;
        const columnRight = columnLeft + columnWidth;
        return islands.some((island) => {
          const islandRect = island.getBoundingClientRect();
          return islandRect.left < columnRight && islandRect.right > columnLeft;
        });
      });
      setOverlapsChat((current) => (current === next ? current : next));
    };

    update();
    window.addEventListener("resize", update);
    if (typeof ResizeObserver === "undefined") {
      return () => window.removeEventListener("resize", update);
    }
    const observer = new ResizeObserver(update);
    transcripts.forEach((transcript) => observer.observe(transcript));
    islands.forEach((island) => observer.observe(island));
    return () => {
      window.removeEventListener("resize", update);
      observer.disconnect();
    };
  }, [enabled, transcriptVersion]);

  return { actionIslandRef, overlapsChat, workspaceIslandRef };
}

export function TitleBar({ sidebarOpen, onToggleSidebar }: TitleBarProps) {
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
  const rightPanelWidth = useUIStore((s) => {
    const stored = s.rightPanelWidth ?? 320;
    const row = s.rightPanelRowWidth ?? 0;
    return row > 0 ? clampRightPanelWidth(stored, row) : stored;
  });
  const activeWorkspacePanelOpen = useUIStore((s) =>
    activeWorkspaceId
      ? (s.rightPanelTabs[activeWorkspaceId] ?? null) !== null
      : false,
  );
  const rightPanelMaximized = useUIStore((s) => s.rightPanelMaximized);
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
  // How much of the band the panel owns, measured from the window's right
  // edge. Everything the *workspace* puts in the band has to stop here.
  const panelBandWidth = panelMaximized
    ? rightPanelRowWidth > 0
      ? rightPanelRowWidth
      : rightPanelWidth
    : rightPanelWidth;
  const contentUnder = useSyncExternalStore(
    subscribeTitlebarContentUnder,
    () => getTitlebarContentUnder(activeWorkspaceId),
    () => false,
  );
  const transcriptVersion = useSyncExternalStore(
    subscribeTitlebarTranscripts,
    getTitlebarTranscriptVersion,
    () => 0,
  );
  const { actionIslandRef, overlapsChat, workspaceIslandRef } =
    useTitlebarChatOverlap(guiChrome, transcriptVersion);

  // The band's right inset, measured from the window's right edge.
  //
  // Panel closed, the workspace branch has to stop before the fixed panel
  // cluster: `topRightReserve` clears that cluster with 6px of breath, and
  // the extra 8px turns it into the single 14px gap the mock puts between
  // the action run and the corner cluster. That is the one deliberate
  // break in the band — everything inside a run is on a 2px rhythm, and
  // the 14px is what tells the eye the corner cluster is a separate,
  // fixed thing rather than the tail of the workspace's own controls.
  // (Panel open, the band tracks the panel's edge; that offset is left
  // exactly as it was.) A draft has no panel cluster to clear, only the
  // native window buttons.
  const bandRightInset = rightPanelOpen
    ? panelBandWidth + 8
    : guiChrome
      ? topRightReserve(remoteClient, false) + 8
      : remoteClient
        ? 6
        : 104;

  // How far past its own right edge the action island's overlap wash
  // reaches. Panel closed that is the whole remaining strip, so the panel
  // toggle and the native window buttons are lit by the same wash as the
  // actions. Panel open, the panel owns that strip and paints its own
  // background, so the wash stays inside the workspace column.
  const actionWashRightExtend = rightPanelOpen ? 28 : bandRightInset;

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
      data-chat-overlap={contentUnder && overlapsChat ? "true" : undefined}
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
          style={{ right: rightPanelOpen ? `${panelBandWidth}px` : 0 }}
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
          panelMaximized && "hidden",
        )}
        style={{
          left: `${sidebarGapWidth + 6}px`,
          right: `${bandRightInset}px`,
        }}
      >
        <div
          ref={workspaceIslandRef}
          data-testid="titlebar-workspace-island"
          className="titlebar-overlap-wash titlebar-overlap-wash-start pointer-events-auto flex h-8 min-w-0 max-w-[58%] items-center"
          onPointerDown={(e) => e.stopPropagation()}
        >
          {guiChrome ? <TitleBarWorkspaceSlots /> : <TitleBarDraftSlots />}
        </div>

        {/* Calm drag gap between the two islands. Same web-client rule as
            the full-width layer above: keep the flex spacer for layout, but
            stay transparent to pointer events where there is no window to
            drag. */}
        <div
          data-testid="titlebar-drag-gap"
          data-tauri-drag-region={remoteClient ? undefined : true}
          className={cn(
            "min-w-4 flex-1 self-stretch",
            remoteClient ? "pointer-events-none" : "pointer-events-auto",
          )}
        />

        <div
          ref={actionIslandRef}
          data-testid="titlebar-action-island"
          className="titlebar-overlap-wash titlebar-overlap-wash-end pointer-events-auto flex h-8 shrink-0 items-center gap-[2px]"
          style={
            {
              "--wash-right-extend": `${actionWashRightExtend}px`,
            } as React.CSSProperties
          }
          onPointerDown={(e) => e.stopPropagation()}
        >
          {guiChrome ? (
            <>
              <div
                data-testid="titlebar-primary-actions"
                className="flex items-center gap-[2px]"
              >
                {activeWorkspaceId && (
                  <RunButton workspaceId={activeWorkspaceId} variant="split" />
                )}
                <IdeLauncher compact />
              </div>
              {/* The right-panel toggle used to sit here. It now lives in
                  the fixed top-right cluster below, because this island
                  tracks the panel's left edge and the toggle must not. */}
              <div
                data-testid="titlebar-utility-actions"
                // Same 2px as the primary run and no leading margin: the
                // mock has one uninterrupted action run, with the only
                // real gap being the 14px before the fixed corner cluster.
                className="flex items-center gap-[2px]"
              >
                {/* Ghost, not the bordered toolbar chip: the band is one
                    frameless family, and this glyph sits mid-run between
                    the equally frameless IDE launcher and panel toggle. */}
                <ResourceMonitor variant="ghost" />
              </div>
            </>
          ) : (
            <ResourceMonitor />
          )}
          {remoteClient && <RemoteConnectionChip compact />}
        </div>
      </div>

      {/* Panel controls. Fixed to the window's top-right corner, just inside
          the native buttons — the one cluster in this band whose position
          never depends on the panel. Workspace-only: a draft renders no
          right panel, so there is nothing for it to control. */}
      {guiChrome && activeWorkspaceId && (
        <RightPanelChromeCluster workspaceId={activeWorkspaceId} />
      )}

      {/* Native window controls stay attached to the physical window edge,
          outside the workspace action island. */}
      {!remoteClient && (
        <div className="pointer-events-auto absolute right-1 top-1 z-10 overflow-hidden rounded-md">
          <WindowControls />
        </div>
      )}
    </div>
  );
}
