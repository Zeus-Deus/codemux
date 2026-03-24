import {
  Command,
  CommandDialog,
  CommandInput,
  CommandList,
  CommandEmpty,
  CommandGroup,
  CommandItem,
  CommandShortcut,
} from "@/components/ui/command";
import { useAppStore } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import { useSidebar } from "@/components/ui/sidebar";
import {
  activateWorkspace,
  splitPane,
  closePane,
  createTab,
  closeTab,
  cyclePane,
  createBrowserPane,
} from "@/tauri/commands";

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function CommandPalette({ open, onOpenChange }: Props) {
  const appState = useAppStore((s) => s.appState);
  const toggleRightPanel = useUIStore((s) => s.toggleRightPanel);
  const setShowNewWorkspaceDialog = useUIStore((s) => s.setShowNewWorkspaceDialog);
  const setShowSettings = useUIStore((s) => s.setShowSettings);
  const { toggleSidebar } = useSidebar();

  const run = (fn: () => void) => {
    onOpenChange(false);
    fn();
  };

  const ws = appState?.workspaces.find(
    (w) => w.workspace_id === appState.active_workspace_id,
  );
  const surface = ws?.surfaces.find(
    (s) => s.surface_id === ws.active_surface_id,
  );
  const activePaneId = surface?.active_pane_id;

  return (
    <CommandDialog open={open} onOpenChange={onOpenChange}>
      <Command>
      <CommandInput placeholder="Type a command..." />
      <CommandList>
        <CommandEmpty>No results found.</CommandEmpty>

        <CommandGroup heading="Workspaces">
          {appState?.workspaces.map((w) => (
            <CommandItem
              key={w.workspace_id}
              onSelect={() =>
                run(() =>
                  activateWorkspace(w.workspace_id).catch(console.error),
                )
              }
            >
              <span className="truncate">{w.title}</span>
              {w.git_branch && (
                <span className="ml-2 text-muted-foreground truncate">
                  {w.git_branch}
                </span>
              )}
            </CommandItem>
          ))}
          <CommandItem
            onSelect={() => run(() => setShowNewWorkspaceDialog(true))}
          >
            Create New Workspace
          </CommandItem>
        </CommandGroup>

        <CommandGroup heading="Panes">
          <CommandItem
            onSelect={() =>
              run(() =>
                activePaneId &&
                splitPane(activePaneId, "horizontal").catch(console.error),
              )
            }
          >
            Split Pane Right
            <CommandShortcut>Ctrl+Shift+D</CommandShortcut>
          </CommandItem>
          <CommandItem
            onSelect={() =>
              run(() =>
                activePaneId &&
                splitPane(activePaneId, "vertical").catch(console.error),
              )
            }
          >
            Split Pane Down
          </CommandItem>
          <CommandItem
            onSelect={() =>
              run(() =>
                activePaneId &&
                closePane(activePaneId).catch(console.error),
              )
            }
          >
            Close Pane
            <CommandShortcut>Ctrl+Shift+W</CommandShortcut>
          </CommandItem>
        </CommandGroup>

        <CommandGroup heading="Tabs">
          <CommandItem
            onSelect={() =>
              run(() =>
                ws &&
                createTab(ws.workspace_id, "terminal").catch(console.error),
              )
            }
          >
            New Terminal Tab
            <CommandShortcut>Ctrl+T</CommandShortcut>
          </CommandItem>
          <CommandItem
            onSelect={() =>
              run(() =>
                ws &&
                ws.tabs.length > 1 &&
                closeTab(ws.workspace_id, ws.active_tab_id).catch(
                  console.error,
                ),
              )
            }
          >
            Close Tab
            <CommandShortcut>Ctrl+W</CommandShortcut>
          </CommandItem>
          <CommandItem
            onSelect={() =>
              run(() =>
                activePaneId &&
                createBrowserPane(activePaneId).catch(console.error),
              )
            }
          >
            Open Browser
          </CommandItem>
          <CommandItem
            onSelect={() =>
              run(() =>
                ws &&
                createTab(ws.workspace_id, "diff").catch(console.error),
              )
            }
          >
            Open Diff Viewer
          </CommandItem>
        </CommandGroup>

        <CommandGroup heading="View">
          <CommandItem
            onSelect={() =>
              run(() =>
                ws && toggleRightPanel(ws.workspace_id, "changes"),
              )
            }
          >
            Toggle Right Panel
          </CommandItem>
          <CommandItem onSelect={() => run(toggleSidebar)}>
            Toggle Sidebar
            <CommandShortcut>Ctrl+B</CommandShortcut>
          </CommandItem>
          <CommandItem onSelect={() => run(() => setShowSettings(true))}>
            Open Settings
          </CommandItem>
        </CommandGroup>

        <CommandGroup heading="Navigation">
          <CommandItem
            onSelect={() =>
              run(() => cyclePane(1).catch(console.error))
            }
          >
            Focus Next Pane
          </CommandItem>
          <CommandItem
            onSelect={() =>
              run(() => cyclePane(-1).catch(console.error))
            }
          >
            Focus Previous Pane
          </CommandItem>
        </CommandGroup>
      </CommandList>
      </Command>
    </CommandDialog>
  );
}
