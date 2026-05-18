import { useState, useMemo, useCallback } from "react";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useAppStore } from "@/stores/app-store";
import {
  createBrowserPane,
  activateWorkspace,
  killPort,
} from "@/tauri/commands";
import { Plug, Globe, X, Copy } from "lucide-react";
import type { PortInfoSnapshot, WorkspaceSnapshot } from "@/tauri/types";
import { cn } from "@/lib/utils";

interface PortGroup {
  workspaceId: string | null;
  workspaceName: string;
  ports: PortInfoSnapshot[];
}

function groupPorts(
  ports: PortInfoSnapshot[],
  workspaces: WorkspaceSnapshot[],
): PortGroup[] {
  const map = new Map<string, PortGroup>();
  for (const port of ports) {
    const key = port.workspace_id ?? "__other__";
    if (!map.has(key)) {
      const ws = workspaces.find((w) => w.workspace_id === port.workspace_id);
      map.set(key, {
        workspaceId: port.workspace_id,
        workspaceName: ws?.title ?? "Other",
        ports: [],
      });
    }
    map.get(key)!.ports.push(port);
  }
  return Array.from(map.values());
}

export function SidebarPortsPopover() {
  const [open, setOpen] = useState(false);
  const appState = useAppStore((s) => s.appState);

  const ports = useMemo(
    () => appState?.detected_ports ?? [],
    [appState?.detected_ports],
  );
  const groups = useMemo(
    () => groupPorts(ports, appState?.workspaces ?? []),
    [ports, appState?.workspaces],
  );

  const portCount = ports.length;

  const openInBrowser = useCallback(
    async (port: PortInfoSnapshot) => {
      const state = useAppStore.getState().appState;
      if (!state) return;
      const wsId = port.workspace_id ?? state.active_workspace_id;
      if (!wsId) return;

      try {
        if (wsId !== state.active_workspace_id) {
          await activateWorkspace(wsId);
          await new Promise((r) => setTimeout(r, 100));
        }
        const fresh = useAppStore.getState().appState;
        const ws = fresh?.workspaces.find((w) => w.workspace_id === wsId);
        const surface = ws?.surfaces.find(
          (s) => s.surface_id === ws.active_surface_id,
        );
        if (!surface) return;
        await createBrowserPane(
          surface.active_pane_id,
          `http://localhost:${port.port}`,
        );
        setOpen(false);
      } catch (err) {
        console.error("Failed to open port in browser:", err);
      }
    },
    [],
  );

  const copyUrl = useCallback((port: PortInfoSnapshot) => {
    navigator.clipboard.writeText(`http://localhost:${port.port}`);
  }, []);

  const handleKill = useCallback((port: PortInfoSnapshot) => {
    killPort(port.port).catch(console.error);
  }, []);

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <Tooltip>
        <TooltipTrigger asChild>
          <PopoverTrigger asChild>
            <Button
              variant="ghost"
              size="icon-xs"
              aria-label="Ports"
              className={cn(
                "relative h-7 w-7 text-muted-foreground hover:text-foreground hover:bg-sidebar-accent",
                portCount > 0 && "text-foreground",
              )}
            >
              <Plug className="size-[18px]" />
              {portCount > 0 && (
                <span className="absolute -top-0.5 -right-0.5 min-w-[15px] h-[15px] px-1 rounded-full bg-primary text-[9px] leading-[15px] text-primary-foreground font-semibold tabular-nums">
                  {portCount}
                </span>
              )}
            </Button>
          </PopoverTrigger>
        </TooltipTrigger>
        <TooltipContent side="top" sideOffset={4} className="text-xs">
          {portCount > 0
            ? `${portCount} active port${portCount === 1 ? "" : "s"}`
            : "No active ports"}
        </TooltipContent>
      </Tooltip>
      <PopoverContent
        side="top"
        align="start"
        sideOffset={6}
        className="w-[340px] p-0"
      >
        <Command shouldFilter={false}>
          <CommandInput placeholder="Filter ports…" className="h-9" />
          <CommandList className="max-h-[340px]">
            {portCount === 0 ? (
              <CommandEmpty>
                <div className="flex flex-col items-center justify-center py-6 text-muted-foreground">
                  <Plug className="h-5 w-5 mb-2 opacity-50" />
                  <span className="text-xs">No active ports detected</span>
                </div>
              </CommandEmpty>
            ) : (
              groups.map((group) => (
                <CommandGroup
                  key={group.workspaceId ?? "__other__"}
                  heading={
                    <span className="text-[10px] uppercase tracking-wide text-muted-foreground/70">
                      {group.workspaceName}
                    </span>
                  }
                >
                  {group.ports.map((port) => (
                    <CommandItem
                      key={port.port}
                      value={`${port.port}-${port.process_name}-${port.label ?? ""}`}
                      onSelect={() => openInBrowser(port)}
                      className="group/port flex items-center gap-2 py-1.5"
                    >
                      <span className="font-mono text-xs font-semibold text-foreground tabular-nums shrink-0">
                        {port.port}
                      </span>
                      <span className="truncate text-[11px] text-muted-foreground flex-1 min-w-0">
                        {port.label ?? port.process_name}
                      </span>
                      <span className="text-[10px] text-muted-foreground/50 tabular-nums shrink-0 transition-opacity group-hover/port:opacity-0">
                        PID {port.pid}
                      </span>
                      <div className="absolute right-2 flex items-center gap-0.5 opacity-0 group-hover/port:opacity-100 transition-opacity">
                        <Tooltip>
                          <TooltipTrigger asChild>
                            <button
                              type="button"
                              onClick={(e) => {
                                e.stopPropagation();
                                openInBrowser(port);
                              }}
                              className="flex h-5 w-5 items-center justify-center rounded-sm text-muted-foreground hover:bg-accent hover:text-foreground"
                              aria-label="Open in browser pane"
                            >
                              <Globe className="h-3 w-3" />
                            </button>
                          </TooltipTrigger>
                          <TooltipContent side="top" sideOffset={4} className="text-xs">
                            Open in browser pane
                          </TooltipContent>
                        </Tooltip>
                        <Tooltip>
                          <TooltipTrigger asChild>
                            <button
                              type="button"
                              onClick={(e) => {
                                e.stopPropagation();
                                copyUrl(port);
                              }}
                              className="flex h-5 w-5 items-center justify-center rounded-sm text-muted-foreground hover:bg-accent hover:text-foreground"
                              aria-label="Copy URL"
                            >
                              <Copy className="h-3 w-3" />
                            </button>
                          </TooltipTrigger>
                          <TooltipContent side="top" sideOffset={4} className="text-xs">
                            Copy URL
                          </TooltipContent>
                        </Tooltip>
                        <Tooltip>
                          <TooltipTrigger asChild>
                            <button
                              type="button"
                              onClick={(e) => {
                                e.stopPropagation();
                                handleKill(port);
                              }}
                              className="flex h-5 w-5 items-center justify-center rounded-sm text-muted-foreground hover:bg-danger/15 hover:text-danger"
                              aria-label="Kill process"
                            >
                              <X className="h-3 w-3" />
                            </button>
                          </TooltipTrigger>
                          <TooltipContent side="top" sideOffset={4} className="text-xs">
                            Kill process
                          </TooltipContent>
                        </Tooltip>
                      </div>
                    </CommandItem>
                  ))}
                </CommandGroup>
              ))
            )}
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}
