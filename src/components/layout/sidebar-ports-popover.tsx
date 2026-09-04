import type { LucideIcon } from "lucide-react";
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
  killPort,
} from "@/tauri/commands";
import { Plug, Globe, X, Copy } from "lucide-react";
import type { PortInfoSnapshot, WorkspaceSnapshot } from "@/tauri/types";
import { cn } from "@/lib/utils";
import { activateWorkspaceInteraction } from "@/lib/perf/instrumented-activate";

type PortGroupKind = "workspace" | "docker" | "other";

interface PortGroup {
  key: string;
  kind: PortGroupKind;
  workspaceId: string | null;
  workspaceName: string;
  ports: PortInfoSnapshot[];
}

const GROUP_RANK: Record<PortGroupKind, number> = {
  workspace: 0,
  docker: 1,
  other: 2,
};

const DOCKER_KEY = "__docker__";
const OTHER_KEY = "__other__";

export function groupPorts(
  ports: PortInfoSnapshot[],
  workspaces: WorkspaceSnapshot[],
): PortGroup[] {
  const map = new Map<string, PortGroup>();
  for (const port of ports) {
    // Docker container ports collapse into one "Docker" group regardless of
    // which worktree they belong to; everything else groups by workspace.
    let key: string;
    let kind: PortGroupKind;
    let workspaceId: string | null;
    let workspaceName: string;
    if (port.source === "docker") {
      key = DOCKER_KEY;
      kind = "docker";
      workspaceId = null;
      workspaceName = "Docker";
    } else if (port.workspace_id) {
      key = port.workspace_id;
      kind = "workspace";
      workspaceId = port.workspace_id;
      const ws = workspaces.find((w) => w.workspace_id === port.workspace_id);
      workspaceName = ws?.title ?? "Other";
    } else {
      key = OTHER_KEY;
      kind = "other";
      workspaceId = null;
      workspaceName = "Other";
    }
    if (!map.has(key)) {
      map.set(key, { key, kind, workspaceId, workspaceName, ports: [] });
    }
    map.get(key)!.ports.push(port);
  }
  // Workspaces first, then Docker, then Other. Array.sort is stable, so the
  // first-seen order is preserved within each rank.
  return Array.from(map.values()).sort(
    (a, b) => GROUP_RANK[a.kind] - GROUP_RANK[b.kind],
  );
}

export function SidebarPortsPopover({ icon: Icon = Plug, labeled = false, tooltipSide = "top" }: { icon?: LucideIcon; labeled?: boolean; tooltipSide?: "top" | "right" }) {
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
          await activateWorkspaceInteraction(wsId);
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
                "relative h-7 text-muted-foreground hover:text-foreground hover:bg-sidebar-accent",
                portCount > 0 && "text-foreground",
                labeled ? "w-full justify-start gap-2 px-2 text-xs" : "w-7",
              )}
            >
              <Icon className="size-[18px]" />
              {labeled && "Ports"}
              {portCount > 0 && (
                <span className="absolute -top-0.5 -right-0.5 min-w-[15px] h-[15px] px-1 rounded-full bg-primary text-[9px] leading-[15px] text-primary-foreground font-semibold tabular-nums">
                  {portCount}
                </span>
              )}
            </Button>
          </PopoverTrigger>
        </TooltipTrigger>
        <TooltipContent side={tooltipSide} sideOffset={4} className="text-xs">
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
                  key={group.key}
                  heading={
                    <span className="text-[10px] uppercase tracking-wide text-muted-foreground/70">
                      {group.workspaceName}
                    </span>
                  }
                >
                  {group.ports.map((port) => (
                    <CommandItem
                      key={`${port.source ?? "os"}-${port.port}`}
                      value={`${port.port}-${port.process_name}-${port.label ?? ""}`}
                      onSelect={() => openInBrowser(port)}
                      className="group/port flex items-center gap-2 py-1.5"
                    >
                      <span className="font-mono text-xs font-semibold text-foreground tabular-nums shrink-0">
                        {port.port}
                      </span>
                      <span
                        className="truncate text-[11px] text-muted-foreground flex-1 min-w-0"
                        title={port.label ?? port.process_name}
                      >
                        {port.label ?? port.process_name}
                      </span>
                      <span className="text-[10px] text-muted-foreground/50 tabular-nums shrink-0 transition-opacity group-hover/port:opacity-0">
                        {port.source === "docker"
                          ? port.process_name
                          : `PID ${port.pid}`}
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
                        {port.source !== "docker" && (
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
                        )}
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
