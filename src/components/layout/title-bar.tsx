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
  MessageSquare,
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
import { useUIStore } from "@/stores/ui-store";
import { toast } from "@/lib/toast";
import {
  agentChatCreatePane,
  applyPreset,
  detectEditors,
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
import type { EditorInfo, TerminalPreset, WorkspaceSnapshot } from "@/tauri/types";

// ── IDE Launcher ──

interface IdeLauncherProps {
  /** GUI chrome renders the compressed [icon square][caret] shape — no
   *  editor-name label — to save horizontal room in the h-10 bar. The
   *  legacy h-9 bar keeps the default (labelled) shape unchanged, so this
   *  defaults to `false` and every existing call site stays byte-identical. */
  compact?: boolean;
}

function IdeLauncher({ compact = false }: IdeLauncherProps) {
  const [editors, setEditors] = useState<EditorInfo[]>([]);
  const [isLoading, setIsLoading] = useState(false);
  const persistedEditor = useSyncedSettingsStore(selectDefaultEditor);
  const activeWorkspace = useAppStore(
    (s) =>
      s.appState?.workspaces.find(
        (w) => w.workspace_id === s.appState?.active_workspace_id,
      ),
  );

  useEffect(() => {
    detectEditors().then((eds) => {
      setEditors(eds);
      if (eds.length > 0 && !persistedEditor && !useSyncedSettingsStore.getState().isLoading) {
        useSyncedSettingsStore.getState().updateSetting("editor", "default_ide", eds[0].id);
      }
    });
  }, [persistedEditor]);

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
    <div className="flex items-center">
      <Tooltip>
        <TooltipTrigger asChild>
          <button
            type="button"
            onClick={() => defaultEditor && handleOpen(defaultEditor.id)}
            disabled={isLoading || !defaultEditor}
            className={cn(
              "flex items-center gap-1 border border-r-0 bg-secondary/50 text-xs font-medium",
              // `compact` (GUI-chrome only) is sized to match the Run
              // split button's 28px bordered-chip shape (h-7, rounded-[7px],
              // full-opacity border-border) so the two titlebar chips read
              // as one family, per the mock. Non-compact (legacy bar) stays
              // byte-identical to before.
              compact
                ? "h-7 w-7 justify-center rounded-l-[7px] border-border px-0"
                : "h-6 rounded-l-md border-border/60 px-2",
              "transition-colors duration-150",
              "hover:bg-secondary hover:border-border",
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
              "flex items-center justify-center border bg-secondary/50 text-muted-foreground",
              compact
                ? "h-7 w-6 rounded-r-[7px] border-border"
                : "h-6 w-5 rounded-r-md border-border/60",
              "transition-colors duration-150",
              "hover:bg-secondary hover:border-border hover:text-foreground",
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

// ── Right-panel toggle (rehomed from TabBar for GUI chrome) ──

function RightPanelToggle({ workspaceId }: { workspaceId: string }) {
  const setRightPanelTab = useUIStore((s) => s.setRightPanelTab);
  const rightPanelTab = useUIStore(
    (s) => s.rightPanelTabs[workspaceId] ?? null,
  );

  // True toggle: any open tab closes the panel; closed opens to Files.
  const togglePanel = () => {
    setRightPanelTab(workspaceId, rightPanelTab == null ? "files" : null);
  };

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size="icon-sm"
          className={cn(
            "shrink-0",
            rightPanelTab
              ? "text-foreground"
              : "text-muted-foreground hover:text-foreground",
          )}
          onClick={togglePanel}
          aria-pressed={rightPanelTab != null}
          aria-label={rightPanelTab ? "Close panel" : "Open panel"}
        >
          <PanelRight className="h-3.5 w-3.5" />
        </Button>
      </TooltipTrigger>
      <TooltipContent side="bottom" sideOffset={4}>
        {rightPanelTab ? "Close panel" : "Open panel"}
      </TooltipContent>
    </Tooltip>
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
            "flex h-[27px] w-[27px] shrink-0 items-center justify-center rounded-lg transition-colors",
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
      className="flex min-w-0 items-center gap-1"
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
      className="flex min-w-0 items-center gap-1"
      onPointerDown={(e) => e.stopPropagation()}
    >
      <div
        data-testid="titlebar-draft-tab"
        className="flex h-7 shrink-0 items-center gap-1.5 rounded-lg bg-background pl-2.5 pr-2.5 text-xs font-semibold text-foreground"
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
  const rightPanelWidth = useUIStore((s) => s.rightPanelWidth ?? 320);
  const activeWorkspacePanelOpen = useUIStore((s) =>
    activeWorkspaceId
      ? (s.rightPanelTabs[activeWorkspaceId] ?? null) !== null
      : false,
  );
  // During a lazy draft the backend's "active workspace" is whatever was
  // focused before the draft opened, and the draft surface never renders
  // that workspace's right panel. Reading its `rightPanelTabs` entry would
  // stop the floating band ~328px short of the right edge with nothing
  // below it — same reason the draft branch suppresses the other
  // workspace-scoped controls.
  const rightPanelOpen = guiChrome && activeWorkspacePanelOpen;
  const remoteClient = isRemoteClient();
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

          Desktop only. This layer spans the full app width — over the
          sidebar, the workspace, and the right panel — so on the web remote
          client, where `data-tauri-drag-region` does nothing at all, it
          would be a pure 40px pointer sink: it swallowed clicks on the top
          of the right panel's 45px tab row (making Files/Tasks/… reliably
          unclickable there) and ate wheel events over the transcript. */}
      {!remoteClient && (
        <div
          data-testid="titlebar-drag-layer"
          data-tauri-drag-region
          className="pointer-events-auto absolute inset-0"
        />
      )}

      {/* Sidebar control island. The sidebar surface itself now reaches the
          top edge; its first local row reserves this small collision area. */}
      <div
        data-testid="titlebar-sidebar-cluster"
        className="pointer-events-auto absolute left-1.5 top-1 z-10 flex h-8 items-center"
        onPointerDown={(e) => e.stopPropagation()}
      >
        <SidebarToggleButton open={sidebarOpen} onToggle={onToggleSidebar} />
      </div>

      {/* Workspace band. It spans only the space between the measured left
          sidebar and either the right panel or native window controls. Tabs
          and actions are separate islands with a calm drag region between. */}
      <div
        data-testid="titlebar-floating-band"
        className="absolute top-1 z-10 flex h-8 min-w-0 items-center gap-2"
        style={{
          left: `${sidebarGapWidth + 6}px`,
          right: rightPanelOpen
            ? `${rightPanelWidth + 8}px`
            : remoteClient
              ? "6px"
              : "104px",
        }}
      >
        <div
          ref={workspaceIslandRef}
          data-testid="titlebar-workspace-island"
          className="titlebar-overlap-surface pointer-events-auto flex h-8 min-w-0 max-w-[58%] items-center"
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
          className="titlebar-overlap-surface pointer-events-auto flex h-8 shrink-0 items-center gap-1"
          onPointerDown={(e) => e.stopPropagation()}
        >
          {guiChrome ? (
            <>
              <div
                data-testid="titlebar-primary-actions"
                className="flex items-center gap-1"
              >
                {activeWorkspaceId && (
                  <RunButton workspaceId={activeWorkspaceId} variant="split" />
                )}
                <IdeLauncher compact />
              </div>
              <div
                data-testid="titlebar-utility-actions"
                className="ml-1 flex items-center gap-0.5"
              >
                <ResourceMonitor variant="toolbar" />
                {activeWorkspaceId && (
                  <RightPanelToggle workspaceId={activeWorkspaceId} />
                )}
              </div>
            </>
          ) : (
            <ResourceMonitor />
          )}
          {remoteClient && <RemoteConnectionChip compact />}
        </div>
      </div>

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
