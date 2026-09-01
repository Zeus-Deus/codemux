import type { HostStatusView, HostView } from "@/tauri/commands";

/**
 * What the sidebar's Devices entry should say about the account's other
 * machines, reduced to one dot and one sentence.
 *
 * The footer is the only place cross-device trouble is visible without
 * opening the Devices page, so the dot doubles as the notification channel:
 * amber means something needs a decision (a transfer that failed, a branch
 * that forked across devices, a device that answers but can't do the job),
 * green means at least one device is up, no dot means nothing is reachable
 * or nothing is known yet. Attention always wins over reachability — a
 * diverged branch on an online device is still a problem.
 */
export type DevicesDot = "green" | "amber" | null;

export interface DevicesIndicator {
  dot: DevicesDot;
  tooltip: string;
}

/** One diverged branch, named the way the tooltip will say it. */
export interface DivergedRowLabel {
  /** Branch name (falls back to the workspace title upstream). */
  title: string;
  /** Configured device that holds a copy; null when only known locally. */
  hostName: string | null;
}

export interface DevicesIndicatorInput {
  hosts: readonly HostView[];
  /** Keyed by `HostView.id`. A host missing here, or present with
   *  `probed: false`, has not been reached yet this session. */
  statuses: Readonly<Record<number, HostStatusView>>;
  divergedRows: readonly DivergedRowLabel[];
  /** Title of the last failed push or pull ("Pull failed: app"), if any. */
  transferError: string | null;
}

export function computeDevicesIndicator({
  hosts,
  statuses,
  divergedRows,
  transferError,
}: DevicesIndicatorInput): DevicesIndicator {
  if (transferError) {
    return { dot: "amber", tooltip: `Devices — ${transferError}` };
  }

  if (divergedRows.length > 0) {
    const [first] = divergedRows;
    const where = first.hostName ? ` on ${first.hostName}` : "";
    const more =
      divergedRows.length > 1 ? ` (+${divergedRows.length - 1} more)` : "";
    return {
      dot: "amber",
      tooltip: `Devices — ${first.title} diverged${where}${more}`,
    };
  }

  let online = 0;
  let offline = 0;
  let unprobed = 0;
  for (const host of hosts) {
    const status = statuses[host.id];
    if (!status || !status.probed) {
      unprobed += 1;
      continue;
    }
    if (!status.reachable) {
      offline += 1;
      continue;
    }
    // Reachable but degraded: SSH answers, yet the host can't take part
    // (no host agent, inventory failed). Worth the amber dot — pushing
    // there would fail, and only setup fixes it.
    if (status.last_error) {
      return { dot: "amber", tooltip: `Devices — ${host.name}: ${status.last_error}` };
    }
    online += 1;
  }

  if (online > 0) {
    return { dot: "green", tooltip: `Devices — ${online} online` };
  }
  if (offline > 0) {
    return {
      dot: null,
      tooltip: unprobed === 0 ? "Devices — all offline" : `Devices — ${offline} offline`,
    };
  }
  // Nothing probed yet (or no devices at all): say nothing rather than
  // claim everything is down.
  return { dot: null, tooltip: "Devices" };
}
