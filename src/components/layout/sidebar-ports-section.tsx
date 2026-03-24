import { useState, useMemo, useCallback } from "react";
import {
  ContextMenu,
  ContextMenuTrigger,
  ContextMenuContent,
  ContextMenuItem,
} from "@/components/ui/context-menu";
import {
  Tooltip,
  TooltipTrigger,
  TooltipContent,
  TooltipProvider,
} from "@/components/ui/tooltip";
import { useAppStore } from "@/stores/app-store";
import {
  createBrowserPane,
  activateWorkspace,
  killPort,
} from "@/tauri/commands";
import { Plug, Globe, X, ChevronRight } from "lucide-react";
import type { PortInfoSnapshot, WorkspaceSnapshot } from "@/tauri/types";

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

function PortPill({
  port,
  workspaceId,
}: {
  port: PortInfoSnapshot;
  workspaceId: string | null;
}) {
  const [confirmingKill, setConfirmingKill] = useState(false);

  const openInBrowser = useCallback(async () => {
    // Read fresh state at call time — never from stale closure
    const state = useAppStore.getState().appState;
    if (!state) return;
    const wsId = workspaceId ?? state.active_workspace_id;
    if (!wsId) return;

    try {
      // Ensure the target workspace is active
      if (wsId !== state.active_workspace_id) {
        await activateWorkspace(wsId);
        await new Promise((r) => setTimeout(r, 100));
      }

      // Find the workspace's active surface and pane (read fresh)
      const freshState = useAppStore.getState().appState;
      const ws = freshState?.workspaces.find((w) => w.workspace_id === wsId);
      const surface = ws?.surfaces.find(
        (s) => s.surface_id === ws.active_surface_id,
      );
      if (!surface) return;

      // Create browser pane with URL set from the start
      await createBrowserPane(
        surface.active_pane_id,
        `http://localhost:${port.port}`,
      );
    } catch (err) {
      console.error("Failed to open port in browser:", err);
    }
  }, [port.port, workspaceId]);

  const handleKill = useCallback(() => {
    if (confirmingKill) {
      killPort(port.port).catch(console.error);
      setConfirmingKill(false);
    } else {
      setConfirmingKill(true);
      setTimeout(() => setConfirmingKill(false), 3000);
    }
  }, [port.port, confirmingKill]);

  const copyUrl = useCallback(() => {
    navigator.clipboard.writeText(`http://localhost:${port.port}`);
  }, [port.port]);

  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <TooltipProvider>
          <Tooltip>
            <TooltipTrigger asChild>
              <button
                className="group/pill inline-flex items-center gap-1 rounded-md bg-accent/10 px-2 py-0.5 transition-colors hover:bg-accent/20"
                onClick={openInBrowser}
              >
                {port.label && (
                  <span className="text-[10px] text-muted-foreground">
                    {port.label}
                  </span>
                )}
                <span className="font-mono text-xs font-semibold text-foreground">
                  {port.port}
                </span>
                <span className="inline-flex items-center gap-px opacity-0 transition-opacity group-hover/pill:opacity-100">
                  <span
                    className="flex h-4 w-4 items-center justify-center rounded-sm text-muted-foreground hover:bg-accent/20 hover:text-accent-foreground"
                    onClick={(e) => {
                      e.stopPropagation();
                      openInBrowser();
                    }}
                  >
                    <Globe className="h-2.5 w-2.5" />
                  </span>
                  <span
                    className={`flex h-4 w-4 items-center justify-center rounded-sm transition-colors ${
                      confirmingKill
                        ? "bg-danger/15 text-danger"
                        : "text-muted-foreground hover:bg-danger/15 hover:text-danger"
                    }`}
                    onClick={(e) => {
                      e.stopPropagation();
                      handleKill();
                    }}
                  >
                    {confirmingKill ? (
                      <span className="text-[7px] font-bold">kill</span>
                    ) : (
                      <X className="h-2.5 w-2.5" />
                    )}
                  </span>
                </span>
              </button>
            </TooltipTrigger>
            <TooltipContent side="right" className="text-xs">
              {port.process_name} (PID {port.pid})
            </TooltipContent>
          </Tooltip>
        </TooltipProvider>
      </ContextMenuTrigger>
      <ContextMenuContent>
        <ContextMenuItem onClick={openInBrowser}>
          Open in browser pane
        </ContextMenuItem>
        <ContextMenuItem onClick={copyUrl}>Copy URL</ContextMenuItem>
        <ContextMenuItem
          onClick={handleKill}
          className="text-danger focus:text-danger"
        >
          Kill process
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  );
}

export function SidebarPortsSection() {
  const appState = useAppStore((s) => s.appState);
  const [expanded, setExpanded] = useState(false);

  const ports = useMemo(
    () => appState?.detected_ports ?? [],
    [appState?.detected_ports],
  );

  const groups = useMemo(
    () => groupPorts(ports, appState?.workspaces ?? []),
    [ports, appState?.workspaces],
  );

  const multiGroup = groups.length > 1;

  return (
    <div className="shrink-0 border-t border-sidebar-border">
      <button
        className="flex w-full items-center gap-1.5 px-3 py-1.5 text-left text-[11px] font-semibold uppercase tracking-wider text-sidebar-foreground/70 hover:text-sidebar-foreground transition-colors"
        onClick={() => setExpanded(!expanded)}
      >
        <ChevronRight
          className={`h-3 w-3 shrink-0 transition-transform ${expanded ? "rotate-90" : ""}`}
        />
        <span>Ports</span>
        {ports.length > 0 && (
          <span className="ml-auto text-[10px] font-semibold tabular-nums text-muted-foreground">
            {ports.length}
          </span>
        )}
      </button>

      {expanded && (
        <div className="px-3 pb-2">
          {ports.length === 0 ? (
            <div className="flex items-center gap-1.5 py-1 text-muted-foreground">
              <Plug className="h-3.5 w-3.5" />
              <span className="text-xs">No active ports</span>
            </div>
          ) : (
            <div className="space-y-2">
              {groups.map((group) => (
                <div key={group.workspaceId ?? "__other__"}>
                  {multiGroup && (
                    <p className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground pb-1">
                      {group.workspaceName}
                    </p>
                  )}
                  <div className="flex flex-wrap gap-1">
                    {group.ports.map((port) => (
                      <PortPill
                        key={port.port}
                        port={port}
                        workspaceId={group.workspaceId}
                      />
                    ))}
                  </div>
                </div>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
