import { useEffect, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ArrowDownNarrowWide, Cpu, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import {
  activateTerminalSession,
  getResourceMetrics,
} from "@/tauri/commands";
import type { ResourceSessionMetrics } from "@/tauri/types";
import {
  selectShowResourceMonitor,
  useSyncedSettingsStore,
} from "@/stores/synced-settings-store";
import { activateWorkspaceInteraction } from "@/lib/perf/instrumented-activate";
import { AppResourceSection } from "./AppResourceSection";
import { MetricBadge } from "./MetricBadge";
import type { SortOption } from "./types";
import { formatCpu, formatMemory, formatPercent } from "./utils/formatters";
import { getTrackedHostMemorySeverity } from "./utils/resource-severity";
import { WorkspaceResourceSection } from "./WorkspaceResourceSection";

const SORT_LABELS: Record<SortOption, string> = {
  memory: "Memory",
  cpu: "CPU",
  name: "Name",
};

function getTrackedMemorySharePercent(
  totalMemory: number,
  hostTotalMemory: number,
): number {
  if (hostTotalMemory <= 0) return 0;
  return (totalMemory / hostTotalMemory) * 100;
}

interface ResourceMonitorProps {
  className?: string;
  variant?: "ghost" | "outline" | "toolbar";
}

/**
 * Title-bar resource monitor: a CPU-chip icon that opens a popover showing
 * how much CPU + memory Codemux and every terminal process tree are using.
 */
export function ResourceMonitor({
  className,
  variant = "ghost",
}: ResourceMonitorProps) {
  const [open, setOpen] = useState(false);
  const [sortOption, setSortOption] = useState<SortOption>("memory");
  const [collapsedProjects, setCollapsedProjects] = useState<Set<string>>(
    new Set(),
  );
  const [collapsedWorkspaces, setCollapsedWorkspaces] = useState<Set<string>>(
    new Set(),
  );

  const enabled = useSyncedSettingsStore(selectShowResourceMonitor);

  const {
    data: snapshot,
    refetch,
    isFetching,
  } = useQuery({
    queryKey: ["resource_metrics"],
    // Closed, only the host share and the severity dot are on screen, so the
    // backend serves a cached summary instead of walking every process.
    queryFn: () => getResourceMetrics(open),
    enabled,
    // Poll fast while the popover is open, slow while it is closed so the
    // tooltip / severity dot stay roughly fresh without wasting work.
    refetchInterval: open ? 2000 : 15000,
  });

  // Opening asks for detail immediately rather than showing the summary's
  // (possibly minute-old) per-process rows until the first 2 s tick.
  useEffect(() => {
    if (open) void refetch();
  }, [open, refetch]);

  const trackedMemorySharePercent = snapshot
    ? getTrackedMemorySharePercent(
        snapshot.total_memory,
        snapshot.host.total_memory,
      )
    : 0;

  const hostShareSeverity = getTrackedHostMemorySeverity(
    trackedMemorySharePercent,
  );
  const shareBarColorClass =
    hostShareSeverity === "high"
      ? "bg-danger/80"
      : hostShareSeverity === "elevated"
        ? "bg-warning/80"
        : "bg-foreground/40";

  const totalUsage = useMemo(
    () => ({
      cpu: snapshot?.total_cpu ?? 0,
      memory: snapshot?.total_memory ?? 0,
    }),
    [snapshot],
  );

  if (!enabled) return null;

  const getSessionName = (session: ResourceSessionMetrics): string =>
    session.title ?? `Terminal ${session.session_id.slice(0, 8)}`;

  const navigateToWorkspace = (workspaceId: string) => {
    activateWorkspaceInteraction(workspaceId).catch(console.error);
    setOpen(false);
  };

  const navigateToSession = (sessionId: string) => {
    void activateTerminalSession(sessionId);
    setOpen(false);
  };

  const toggleProject = (projectId: string) => {
    setCollapsedProjects((prev) => {
      const next = new Set(prev);
      if (next.has(projectId)) next.delete(projectId);
      else next.add(projectId);
      return next;
    });
  };

  const toggleWorkspace = (workspaceId: string) => {
    setCollapsedWorkspaces((prev) => {
      const next = new Set(prev);
      if (next.has(workspaceId)) next.delete(workspaceId);
      else next.add(workspaceId);
      return next;
    });
  };

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <Tooltip delayDuration={150}>
        <TooltipTrigger asChild>
          <PopoverTrigger asChild>
            <Button
              variant={variant}
              size="icon-sm"
              aria-label="Resource monitor"
              className={cn("text-muted-foreground", className)}
            >
              <Cpu />
            </Button>
          </PopoverTrigger>
        </TooltipTrigger>
        <TooltipContent side="bottom" sideOffset={4}>
          {snapshot
            ? `Resources · ${formatMemory(snapshot.total_memory)}`
            : "Resource monitor"}
        </TooltipContent>
      </Tooltip>

      <PopoverContent align="start" className="w-[28rem] p-0 overflow-hidden">
        <div className="px-3.5 pt-3 pb-3 border-b border-border/60">
          <div className="flex items-center justify-between">
            <h4 className="text-[13px] font-medium tracking-tight text-foreground">
              Resources
            </h4>
            <div className="flex items-center gap-0.5">
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <button
                    type="button"
                    className="flex items-center gap-1 h-6 px-1.5 rounded text-[11px] text-muted-foreground hover:text-foreground hover:bg-foreground/[0.06] transition-colors"
                    aria-label="Sort workspaces"
                  >
                    <ArrowDownNarrowWide className="h-3.5 w-3.5" />
                    <span>{SORT_LABELS[sortOption]}</span>
                  </button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end" className="w-40">
                  <DropdownMenuRadioGroup
                    value={sortOption}
                    onValueChange={(value) =>
                      setSortOption(value as SortOption)
                    }
                  >
                    <DropdownMenuRadioItem value="memory">
                      Memory
                    </DropdownMenuRadioItem>
                    <DropdownMenuRadioItem value="cpu">
                      CPU
                    </DropdownMenuRadioItem>
                    <DropdownMenuRadioItem value="name">
                      Name
                    </DropdownMenuRadioItem>
                  </DropdownMenuRadioGroup>
                </DropdownMenuContent>
              </DropdownMenu>
              <button
                type="button"
                onClick={() => refetch()}
                className="h-6 w-6 inline-flex items-center justify-center rounded text-muted-foreground hover:text-foreground hover:bg-foreground/[0.06] transition-colors"
                aria-label="Refresh metrics"
              >
                <RefreshCw
                  className={cn("h-3.5 w-3.5", isFetching && "animate-spin")}
                />
              </button>
            </div>
          </div>

          {snapshot && (
            <>
              <div className="mt-3 grid grid-cols-3 divide-x divide-border/50">
                <MetricBadge
                  label="CPU"
                  value={formatCpu(snapshot.total_cpu)}
                  tooltip="Sum of CPU used by Codemux and monitored terminal process trees. Over 100% means multiple CPU cores are busy."
                />
                <MetricBadge
                  label="Memory"
                  value={formatMemory(snapshot.total_memory)}
                  tooltip="Resident memory used by Codemux and monitored terminal process trees. If this keeps climbing, a workspace process may be retaining memory."
                />
                <MetricBadge
                  label="RAM Share"
                  value={formatPercent(trackedMemorySharePercent)}
                  tooltip="Percent of total system RAM used by monitored Codemux resources only (not all apps)."
                />
              </div>
              <Tooltip delayDuration={150}>
                <TooltipTrigger asChild>
                  <div
                    className="mt-3 h-1 w-full overflow-hidden rounded-full bg-muted/60"
                    role="progressbar"
                    aria-label="System RAM share"
                    aria-valuenow={Math.round(trackedMemorySharePercent)}
                    aria-valuemin={0}
                    aria-valuemax={100}
                  >
                    <div
                      className={cn(
                        "h-full rounded-full transition-[width] duration-300",
                        shareBarColorClass,
                      )}
                      style={{
                        width: `${Math.min(100, Math.max(0, trackedMemorySharePercent))}%`,
                      }}
                    />
                  </div>
                </TooltipTrigger>
                <TooltipContent side="bottom" sideOffset={6}>
                  Codemux uses {formatPercent(trackedMemorySharePercent)} of
                  system RAM
                </TooltipContent>
              </Tooltip>
            </>
          )}
        </div>

        <div className="max-h-[50vh] overflow-y-auto">
          {snapshot && (
            <AppResourceSection app={snapshot.app} totalUsage={totalUsage} />
          )}

          {snapshot && (
            <WorkspaceResourceSection
              workspaces={snapshot.workspaces}
              sortOption={sortOption}
              collapsedProjects={collapsedProjects}
              toggleProject={toggleProject}
              collapsedWorkspaces={collapsedWorkspaces}
              toggleWorkspace={toggleWorkspace}
              navigateToWorkspace={navigateToWorkspace}
              navigateToSession={navigateToSession}
              getSessionName={getSessionName}
            />
          )}

          {snapshot && snapshot.workspaces.length === 0 && (
            <div className="px-3.5 py-6 text-center text-[11px] text-muted-foreground">
              No active terminal sessions
            </div>
          )}

          {!snapshot && (
            <div className="px-3.5 py-6 text-center text-[11px] text-muted-foreground">
              Loading…
            </div>
          )}
        </div>
      </PopoverContent>
    </Popover>
  );
}
