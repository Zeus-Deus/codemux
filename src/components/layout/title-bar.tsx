import { useState, useEffect, useCallback } from "react";
import {
  PanelLeft,
  ChevronDown,
  ExternalLink,
} from "lucide-react";
import { WindowControls } from "./window-chrome";
import { ResourceMonitor } from "./resource-monitor";
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
import { useAppStore } from "@/stores/app-store";
import { detectEditors, openInEditor } from "@/tauri/commands";
import { cn } from "@/lib/utils";
import { EditorIcon } from "@/components/icons/editor-icon";
import { useSyncedSettingsStore, selectDefaultEditor } from "@/stores/synced-settings-store";
import type { EditorInfo } from "@/tauri/types";

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
  const groupedEditors = groupEditors(editors);
  const showGroupLabels = groupedEditors.length > 1;

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
        <Separator orientation="vertical" className="!h-4 !self-auto bg-border/50" />
        <WindowControls />
      </div>
    </div>
  );
}
