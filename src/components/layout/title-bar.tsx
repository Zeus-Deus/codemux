import { useState, useEffect, useCallback } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useResolvedKeybinds } from "@/hooks/use-resolved-keybinds";
import {
  PanelLeft,
  Search,
  Settings,
  Keyboard,
  BookOpen,
  Bug,
  Info,
  LogOut,
  Minus,
  Square,
  Copy,
  X,
  ChevronDown,
  ExternalLink,
} from "lucide-react";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import { useUIStore } from "@/stores/ui-store";
import { useAppStore } from "@/stores/app-store";
import { detectEditors, openInEditor } from "@/tauri/commands";
import { cn } from "@/lib/utils";
import { EditorIcon } from "@/components/icons/editor-icon";
import { useSyncedSettingsStore, selectDefaultEditor } from "@/stores/synced-settings-store";
import type { EditorInfo } from "@/tauri/types";

// ── Window Controls ──

function WindowControls() {
  const [isMaximized, setIsMaximized] = useState(false);
  const appWindow = getCurrentWindow();

  useEffect(() => {
    appWindow.isMaximized().then(setIsMaximized);
    const unlisten = appWindow.onResized(() => {
      appWindow.isMaximized().then(setIsMaximized);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [appWindow]);

  return (
    <div className="flex items-center">
      <button
        type="button"
        aria-label="Minimize"
        className="flex h-7 w-8 items-center justify-center text-muted-foreground transition-colors duration-150 hover:bg-accent hover:text-foreground"
        onClick={() => appWindow.minimize()}
      >
        <Minus className="h-3 w-3" />
      </button>
      <button
        type="button"
        aria-label={isMaximized ? "Restore" : "Maximize"}
        className="flex h-7 w-8 items-center justify-center text-muted-foreground transition-colors duration-150 hover:bg-accent hover:text-foreground"
        onClick={() => appWindow.toggleMaximize()}
      >
        {isMaximized ? (
          <Copy className="h-3 w-3" />
        ) : (
          <Square className="h-2.5 w-2.5" />
        )}
      </button>
      <button
        type="button"
        aria-label="Close"
        className="flex h-7 w-8 items-center justify-center text-muted-foreground transition-colors duration-150 hover:bg-destructive hover:text-destructive-foreground"
        onClick={() => appWindow.close()}
      >
        <X className="h-3.5 w-3.5" />
      </button>
    </div>
  );
}

// ── Search Trigger ──

function SearchTrigger() {
  const { getKeysForAction } = useResolvedKeybinds();
  const toggleCombo = getKeysForAction("commandPalette");

  const handleClick = useCallback(() => {
    useUIStore.getState().toggleCommandPalette();
  }, []);

  return (
    <div className="absolute inset-0 flex items-center justify-center pointer-events-none">
      <button
        type="button"
        onClick={handleClick}
        className="pointer-events-auto flex items-center gap-2 h-6 px-2.5 rounded-md border border-border/50 bg-muted/50 text-muted-foreground text-xs transition-colors duration-150 hover:bg-muted hover:text-foreground cursor-pointer min-w-[120px] max-w-[260px]"
      >
        <Search className="h-3 w-3 shrink-0" />
        <span className="truncate">Search...</span>
        {toggleCombo && (
          <kbd className="ml-auto shrink-0 text-[10px] text-muted-foreground/60 border border-border/40 rounded px-1 py-px">
            {toggleCombo}
          </kbd>
        )}
      </button>
    </div>
  );
}

function SettingsShortcutHint() {
  const { getKeysForAction } = useResolvedKeybinds();
  const keys = getKeysForAction("openSettings");
  if (!keys) return null;
  return <kbd className="ml-auto text-[10px] text-muted-foreground">{keys}</kbd>;
}

// ── App Menu ──

function AppMenu() {
  const setShowSettings = useUIStore((s) => s.setShowSettings);
  const [version, setVersion] = useState<string | null>(null);

  useEffect(() => {
    getVersion().then(setVersion);
  }, []);

  return (
    <DropdownMenu>
      <Tooltip>
        <TooltipTrigger asChild>
          <DropdownMenuTrigger asChild>
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label="App menu"
            >
              <Settings className="h-3.5 w-3.5" />
            </Button>
          </DropdownMenuTrigger>
        </TooltipTrigger>
        <TooltipContent side="bottom" sideOffset={4}>
          Menu
        </TooltipContent>
      </Tooltip>
      <DropdownMenuContent align="end" className="w-52">
        <DropdownMenuItem
          onClick={() => setShowSettings(true)}
        >
          <Settings className="h-4 w-4" />
          <span>Settings</span>
          <SettingsShortcutHint />
        </DropdownMenuItem>
        <DropdownMenuItem
          onClick={() => setShowSettings(true, "shortcuts")}
        >
          <Keyboard className="h-4 w-4" />
          <span>Keyboard Shortcuts</span>
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem
          onClick={() => openUrl("https://docs.codemux.org/installation")}
        >
          <BookOpen className="h-4 w-4" />
          <span>Documentation</span>
          <ExternalLink className="ml-auto h-3 w-3 text-muted-foreground" />
        </DropdownMenuItem>
        <DropdownMenuItem
          onClick={() => openUrl("https://github.com/Zeus-Deus/codemux/issues/new")}
        >
          <Bug className="h-4 w-4" />
          <span>Report Issue</span>
          <ExternalLink className="ml-auto h-3 w-3 text-muted-foreground" />
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem disabled>
          <Info className="h-4 w-4" />
          <span>Codemux {version ? `v${version}` : ""}</span>
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem
          onClick={() => {
            import("@/stores/auth-store").then(({ useAuthStore }) =>
              useAuthStore.getState().signOut(),
            );
          }}
        >
          <LogOut className="h-4 w-4" />
          <span>Sign Out</span>
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

// ── IDE Launcher ──

function IdeLauncher() {
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

  if (editors.length === 0 || !workspacePath) return null;

  return (
    <div className="flex items-center">
      <Tooltip>
        <TooltipTrigger asChild>
          <button
            type="button"
            onClick={() => defaultEditor && handleOpen(defaultEditor.id)}
            disabled={isLoading || !defaultEditor}
            className={cn(
              "flex items-center gap-1 h-6 px-2 rounded-l-md border border-r-0 border-border/60 bg-secondary/50 text-xs font-medium",
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
            <span className="hidden sm:inline">
              {defaultEditor?.name ?? "Open"}
            </span>
          </button>
        </TooltipTrigger>
        <TooltipContent side="bottom" sideOffset={4}>
          {defaultEditor ? `Open in ${defaultEditor.name}` : "Open in editor"}
        </TooltipContent>
      </Tooltip>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            disabled={isLoading}
            className={cn(
              "flex items-center justify-center h-6 w-5 rounded-r-md border border-border/60 bg-secondary/50 text-muted-foreground",
              "transition-colors duration-150",
              "hover:bg-secondary hover:border-border hover:text-foreground",
              isLoading && "opacity-50 pointer-events-none",
            )}
          >
            <ChevronDown className="h-3 w-3" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-44">
          {editors.map((editor) => (
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
          onClick={onToggle}
          className={cn(open && "text-foreground")}
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

// ── Title Bar ──

interface TitleBarProps {
  sidebarOpen: boolean;
  onToggleSidebar: () => void;
}

export function TitleBar({ sidebarOpen, onToggleSidebar }: TitleBarProps) {
  return (
    <div
      data-tauri-drag-region
      className="relative flex h-9 w-full shrink-0 items-center justify-between border-b border-border bg-card"
    >
      {/* Left */}
      <div className="flex items-center gap-1 pl-2">
        <SidebarToggleButton open={sidebarOpen} onToggle={onToggleSidebar} />
      </div>

      {/* Center — search trigger absolutely positioned */}
      <SearchTrigger />

      {/* Right */}
      <div className="flex items-center gap-1.5 pr-0.5">
        <IdeLauncher />
        <AppMenu />
        <Separator orientation="vertical" className="!h-4 !self-auto bg-border/50" />
        <WindowControls />
      </div>
    </div>
  );
}
