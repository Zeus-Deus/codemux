import { useEffect, useMemo } from "react";
import { MonitorSmartphone } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useAppStore } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import { useHosts } from "@/stores/hosts-store";
import { useHostStatuses } from "@/stores/host-status-store";
import { useWorkspacesSyncStore } from "@/stores/workspaces-sync-store";
import { detectDivergedRows } from "@/lib/workspace-divergence";
import {
  computeDevicesIndicator,
  type DivergedRowLabel,
} from "@/lib/devices-attention";
import { cn } from "@/lib/utils";
import type { HostView, WorkspaceSyncView } from "@/tauri/commands";

/**
 * Reduce the synced rows to one label per diverged branch. Both copies of a
 * forked branch come back flagged; the tooltip should say the branch once
 * and name a configured device that holds a copy, never "this device".
 */
function divergedLabels(
  rows: readonly WorkspaceSyncView[],
  hosts: readonly HostView[],
): DivergedRowLabel[] {
  const diverged = detectDivergedRows(rows);
  if (diverged.size === 0) return [];

  const nameByServerId = new Map<string, string>();
  for (const host of hosts) {
    if (host.server_id) nameByServerId.set(host.server_id, host.name);
  }

  const seen = new Set<string>();
  const labels: DivergedRowLabel[] = [];
  for (const row of rows) {
    const info = diverged.get(row.id);
    if (!info) continue;
    const key = `${row.project_remote}::${row.git_branch}`;
    if (seen.has(key)) continue;
    seen.add(key);

    let hostName: string | null = null;
    for (const serverId of [row.host_server_id, ...info.otherHostServerIds]) {
      const name = serverId ? nameByServerId.get(serverId) : undefined;
      if (name) {
        hostName = name;
        break;
      }
    }
    labels.push({ title: row.git_branch ?? row.title, hostName });
  }
  return labels;
}

/**
 * The Devices destination. Present once the page has something to show: a
 * configured device, or work the account knows about on a device this
 * install hasn't configured (the "Another device" card). A fresh install
 * never has to wonder what it is — adding a device lives in Settings, and
 * the command palette reaches the page regardless. The dot is the app's
 * only ambient signal for cross-device state; see `computeDevicesIndicator`.
 *
 * Divergence reads the sync store's last snapshot plus one load on mount.
 * The Devices page polls while open; the footer deliberately does not.
 */
export function SidebarDevicesButton({
  tooltipSide = "top",
}: {
  tooltipSide?: "top" | "right";
}) {
  const setShowDevices = useUIStore((s) => s.setShowDevices);
  const hosts = useHosts();
  const statuses = useHostStatuses();
  const rows = useWorkspacesSyncStore((s) => s.rows);
  const initSync = useWorkspacesSyncStore((s) => s.init);
  const transferError = useAppStore(
    (s) => s.workspacePushPullError?.title ?? null,
  );

  useEffect(() => {
    void initSync();
  }, [initSync]);

  const { dot, tooltip } = useMemo(
    () =>
      computeDevicesIndicator({
        hosts,
        statuses,
        divergedRows: divergedLabels(rows, hosts),
        transferError,
      }),
    [hosts, statuses, rows, transferError],
  );

  const hasSiblingRows = rows.some((row) => row.workspace_id === null);
  if (hosts.length === 0 && !hasSiblingRows) return null;

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size="icon-xs"
          aria-label="Devices"
          data-testid="sidebar-devices"
          onClick={() => setShowDevices(true)}
          className="relative size-7 rounded-[7px] text-muted-foreground hover:bg-foreground/[0.04] hover:text-foreground"
        >
          <MonitorSmartphone className="size-[15px]" />
          {dot && (
            <span
              data-testid="sidebar-devices-dot"
              data-tone={dot}
              className={cn(
                // Ringed in the footer's own colour so it reads as sitting on
                // the icon rather than clipped by it.
                "absolute right-1 top-1 size-1.5 rounded-full ring-2 ring-sidebar",
                dot === "amber" ? "bg-status-working" : "bg-status-open",
              )}
            />
          )}
        </Button>
      </TooltipTrigger>
      <TooltipContent side={tooltipSide} sideOffset={4} className="text-xs">
        {tooltip}
      </TooltipContent>
    </Tooltip>
  );
}
