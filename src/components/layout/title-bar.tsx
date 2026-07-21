import { useState, useEffect, useCallback } from "react";
import {
  PanelLeft,
  ChevronDown,
  ExternalLink,
  FileDiff,
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
              ? "bg-card text-foreground"
              : "text-muted-foreground hover:text-foreground",
          )}
          onClick={togglePanel}
          aria-pressed={rightPanelTab != null}
          aria-label={rightPanelTab ? "Close panel" : "Open panel"}
        >
          <FileDiff className="h-3.5 w-3.5" />
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

export function TitleBar({ sidebarOpen, onToggleSidebar }: TitleBarProps) {
  const activeWorkspaceId = useActiveWorkspaceId();
  // Called unconditionally (Rules of Hooks) even though only the GUI-chrome
  // branch below renders the sidebar-matched left cluster that consumes it.
  const sidebarGapWidth = useSidebarGapWidth();

  // GUI chrome renders for a real, non-OpenFlow workspace when the Agent
  // Chat Beta is on; a live lazy-creation draft renders the same h-10
  // shell with draft slots instead (mutually exclusive predicates — see
  // the hook doc comments). OpenFlow keeps its dedicated chrome
  // untouched. Shared with other GUI-mode-only surfaces via
  // `useGuiChrome`.
  const guiChrome = useGuiChrome();
  const draftGuiChrome = useDraftGuiChrome();

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
      data-tauri-drag-region
      className="relative flex h-10 w-full shrink-0 items-center border-b border-border bg-card"
    >
      {/* Left — sidebar toggle ONLY, sized to match the app sidebar so
          tabs begin exactly where the content pane begins (not floating
          above the sidebar). `TitleBar` renders as a sibling of
          `SidebarProvider` in AppShell, not a descendant, so it can't read
          `--sidebar-width` via CSS inheritance or `useSidebar()` — see
          `useSidebarGapWidth` for how the live width is mirrored instead. */}
      <div
        data-testid="titlebar-sidebar-cluster"
        className="flex h-full shrink-0 items-center justify-start border-r border-border px-2.5"
        style={{ width: `${sidebarGapWidth}px` }}
        onPointerDown={(e) => e.stopPropagation()}
      >
        <SidebarToggleButton open={sidebarOpen} onToggle={onToggleSidebar} />
      </div>

      {/* Workspace slots — tabs (scrollable) + launcher + pinned tiles.
          Draft chrome renders its single "Agent Chat" pill + draft
          launcher instead (no workspace, no backend tabs yet). */}
      {guiChrome ? <TitleBarWorkspaceSlots /> : <TitleBarDraftSlots />}

      {/* Draggable spacer — the calm middle stays a window drag region.
          Tauri v2's injected mousedown handler only checks the direct
          target for `data-tauri-drag-region` (no ancestor walk), so this
          covering spacer must carry the attribute itself even though the
          root does — otherwise the middle isn't draggable on Windows/macOS.
          Discrete slots (tabs / launcher / pinned tiles / spacer / right
          cluster) leave room for a future inline workflow-status pill. */}
      <div data-tauri-drag-region className="flex-1 self-stretch" />

      {/* Right cluster — Run split button, then the standard monitor /
          IDE controls, with the right-panel toggle docked beside the
          window controls (layout toggles live next to window chrome).
          The workspace-scoped controls (panel toggle / Run / IDE) are
          suppressed in draft chrome: the backend's "active workspace"
          is whatever was focused before the draft opened, which is NOT
          what's on screen — acting on it here would be misleading. */}
      <div
        className="flex shrink-0 items-center gap-1.5 pr-0.5"
        onPointerDown={(e) => e.stopPropagation()}
      >
        {guiChrome && activeWorkspaceId && (
          <RunButton workspaceId={activeWorkspaceId} variant="split" />
        )}
        <ResourceMonitor />
        {guiChrome && <IdeLauncher compact />}
        {/* The separator divides the content cluster from the layout-toggle
            + native window controls group — on the web remote client those
            controls render nothing, so the separator would dangle next to a
            lone toggle at the right edge. Hide it there. */}
        {!isRemoteClient() && (
          <Separator orientation="vertical" className="!h-4 !self-auto bg-border/50" />
        )}
        {guiChrome && activeWorkspaceId && (
          <RightPanelToggle workspaceId={activeWorkspaceId} />
        )}
        <WindowControls />
        {/* Web remote client only: the connection chip takes the slot the
            hidden window controls free up. Quiet while connected; the loud
            reconnecting/offline states live in the app-wide banner. */}
        {isRemoteClient() && <RemoteConnectionChip compact />}
      </div>
    </div>
  );
}
