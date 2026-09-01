import { useMemo } from "react";

import { useAppStore } from "@/stores/app-store";
import { useHosts } from "@/stores/hosts-store";
import { useHostStatuses } from "@/stores/host-status-store";
import { useWorkspacesSync } from "@/stores/workspaces-sync-store";
import { useCoarseClock } from "@/lib/use-coarse-clock";
import { relativeTime } from "@/components/settings/remote-access-utils";
import {
  detectDivergedRows,
  type DivergenceInfo,
} from "@/lib/workspace-divergence";
import type {
  HostStatusView,
  HostView,
  WorkspaceSyncView,
} from "@/tauri/commands";

/**
 * Composition for the Devices page: one card per configured device, each
 * carrying that device's synced workspace rows grouped by project, with
 * every label the view needs already resolved. Everything below
 * `useDeviceCards` is pure so the grouping and labelling rules stay
 * unit-testable without React.
 */

/** How a card header reads: colour and default expansion follow this. */
export type DeviceTone = "online" | "attention" | "offline" | "checking";

/** A synced row plus the facts the row view needs beyond the row itself. */
export interface DeviceRow {
  sync: WorkspaceSyncView;
  /** Where else this branch lives at a different commit — "zeus",
   *  "this device", "zeus + 1 more" — or null when it hasn't diverged. */
  divergedLabel: string | null;
}

/** A project cluster inside a device card. */
export interface DeviceProject {
  /** Stable key for React and for the per-project collapse state. */
  key: string;
  name: string;
  /** Present when every row was stamped with the same deterministic
   *  project id — required for the one-click "Pull project" action. */
  projectUid: string | null;
  rows: DeviceRow[];
}

export interface DeviceCard {
  /** React key. Configured hosts key by their local id so a host that has
   *  not synced yet (no server id) still gets a stable card. */
  key: string;
  /** Null for an orphan card — rows whose host is not configured here. */
  host: HostView | null;
  name: string;
  serverId: string | null;
  tone: DeviceTone;
  /** "online", "needs attention", "unreachable · last seen 2d ago", … */
  statusLabel: string;
  /** The poller's note behind a degraded or unreachable label. */
  statusDetail: string | null;
  remoteControlServing: boolean;
  diskBytes: number | null;
  projects: DeviceProject[];
}

/** Rows across every project of a card. */
export function cardRowCount(card: DeviceCard): number {
  return card.projects.reduce((n, p) => n + p.rows.length, 0);
}

/**
 * Project name for a synced row. Prefers the `project_path` basename; falls
 * back to the workspace title when the originating host never recorded a
 * project root (a plain root checkout created via the MCP tool only stamps
 * `project_root` for worktrees). `title` is always present, so a row never
 * renders under a bare "—".
 */
export function remoteProjectName(row: WorkspaceSyncView): string | null {
  const fromPath = row.project_path
    ? row.project_path.split("/").filter(Boolean).slice(-1)[0] ?? null
    : null;
  return fromPath ?? (row.title.trim() || null);
}

/**
 * Cluster a device's rows by project. Identity is the deterministic
 * `project_uid` when stamped, else the project path/name, so a repo root and
 * its worktrees (different paths, same uid) land together while two unrelated
 * repos sharing a basename do not. The root checkout floats to the top of its
 * cluster; clusters sort by name.
 */
export function groupRowsByProject(rows: readonly DeviceRow[]): DeviceProject[] {
  const byKey = new Map<string, { name: string; rows: DeviceRow[] }>();
  for (const row of rows) {
    const name = remoteProjectName(row.sync) ?? "untitled";
    const key = (row.sync.project_uid ?? row.sync.project_path ?? name).toLowerCase();
    const group = byKey.get(key);
    if (group) group.rows.push(row);
    else byKey.set(key, { name, rows: [row] });
  }
  const projects: DeviceProject[] = [];
  for (const [key, group] of byKey) {
    group.rows.sort(
      (a, b) =>
        Number(b.sync.workspace_kind === "main") -
        Number(a.sync.workspace_kind === "main"),
    );
    projects.push({
      key,
      name: group.name,
      projectUid: group.rows.map((r) => r.sync.project_uid).find(Boolean) ?? null,
      rows: group.rows,
    });
  }
  projects.sort((a, b) =>
    a.name.localeCompare(b.name, undefined, { sensitivity: "base" }),
  );
  return projects;
}

export interface DeviceStatusSummary {
  tone: DeviceTone;
  label: string;
  detail: string | null;
}

/**
 * One-line reachability summary for a card header. A host the poller has
 * not reached yet this session is "checking", not offline — the app just
 * started, and a stale "unreachable" is a false alarm. A host that answers
 * SSH but reports a problem (no host agent, inventory failed) is online in
 * the network sense and useless in every other, so it asks for attention.
 */
export function describeStatus(
  status: HostStatusView | null,
  now: number,
): DeviceStatusSummary {
  if (!status || !status.probed) {
    return { tone: "checking", label: "checking…", detail: null };
  }
  if (status.reachable) {
    return status.last_error
      ? { tone: "attention", label: "needs attention", detail: status.last_error }
      : { tone: "online", label: "online", detail: null };
  }
  return {
    tone: "offline",
    label: status.last_seen_at
      ? `unreachable · last seen ${relativeTime(status.last_seen_at, now)}`
      : "never reached",
    detail: status.last_error,
  };
}

/**
 * Where else a diverged branch lives, for the chip tooltip: "zeus",
 * "this device", or "zeus + 1 more".
 */
function divergedLabel(
  info: DivergenceInfo,
  nameByServerId: ReadonlyMap<string, string>,
): string {
  const labels = Array.from(
    new Set(
      info.otherHostServerIds.map((id) =>
        id === null ? "this device" : nameByServerId.get(id) ?? "another device",
      ),
    ),
  );
  if (labels.length === 0) return "another device";
  if (labels.length === 1) return labels[0];
  return `${labels[0]} + ${labels.length - 1} more`;
}

/** A local attach-in-place workspace: a window onto a host workspace. */
export interface AttachedInPlace {
  hostId: number;
  remoteCwd: string;
}

export interface BuildDeviceCardsInput {
  hosts: readonly HostView[];
  statuses: Readonly<Record<number, HostStatusView>>;
  syncRows: readonly WorkspaceSyncView[];
  attachedInPlace: readonly AttachedInPlace[];
  now: number;
}

/**
 * Build the card list. Configured hosts keep their configured order; rows
 * whose host is unknown here trail as "another device" cards so nothing the
 * account knows about disappears silently.
 */
export function buildDeviceCards({
  hosts,
  statuses,
  syncRows,
  attachedInPlace,
  now,
}: BuildDeviceCardsInput): DeviceCard[] {
  const serverIdByHostId = new Map<number, string>();
  const nameByServerId = new Map<string, string>();
  for (const h of hosts) {
    if (!h.server_id) continue;
    serverIdByHostId.set(h.id, h.server_id);
    nameByServerId.set(h.server_id, h.name);
  }

  // An attach-in-place workspace is a local window onto a host workspace that
  // ALSO has a synced row. Showing both would offer "Open on host" for
  // something already open here, so the row is hidden by `<host>::<path>`.
  const openInPlace = new Set<string>();
  for (const att of attachedInPlace) {
    const serverId = serverIdByHostId.get(att.hostId);
    if (serverId) openInPlace.add(`${serverId}::${att.remoteCwd}`);
  }

  // Divergence is judged over every row the account knows about — a branch
  // forked against this laptop's own copy is flagged too.
  const divergence = detectDivergedRows(syncRows);
  const toRow = (sync: WorkspaceSyncView): DeviceRow => {
    const info = divergence.get(sync.id);
    return {
      sync,
      divergedLabel: info ? divergedLabel(info, nameByServerId) : null,
    };
  };

  // Rows that still map to a local workspace are this device's own — or a
  // just-closed workspace whose soft-delete hasn't synced yet. Either way
  // they are not something to pull or open remotely.
  const candidates = syncRows.filter(
    (row) =>
      row.workspace_id === null &&
      !(
        row.host_server_id &&
        row.origin_path &&
        openInPlace.has(`${row.host_server_id}::${row.origin_path}`)
      ),
  );

  const cards: DeviceCard[] = [];
  const claimed = new Set<number>();
  for (const host of hosts) {
    const rows = host.server_id
      ? candidates.filter((row) => row.host_server_id === host.server_id)
      : [];
    for (const row of rows) claimed.add(row.id);
    const status = statuses[host.id] ?? null;
    const summary = describeStatus(status, now);
    cards.push({
      key: `host:${host.id}`,
      host,
      name: host.name,
      serverId: host.server_id,
      tone: summary.tone,
      statusLabel: summary.label,
      statusDetail: summary.detail,
      remoteControlServing: status?.remote_control_serving === true,
      diskBytes: status?.disk_bytes ?? null,
      projects: groupRowsByProject(rows.map(toRow)),
    });
  }

  // Orphans: known to the account but not configured on this device. One
  // card per unknown host id; rows from a host-less device share one card.
  const orphans = new Map<string | null, WorkspaceSyncView[]>();
  for (const row of candidates) {
    if (claimed.has(row.id)) continue;
    const list = orphans.get(row.host_server_id) ?? [];
    list.push(row);
    orphans.set(row.host_server_id, list);
  }
  for (const [serverId, rows] of orphans) {
    cards.push({
      key: `orphan:${serverId ?? "none"}`,
      host: null,
      name: "Another device",
      serverId,
      tone: "offline",
      statusLabel: "not configured on this device",
      statusDetail: null,
      remoteControlServing: false,
      diskBytes: null,
      projects: groupRowsByProject(rows.map(toRow)),
    });
  }
  return cards;
}

/**
 * Attach-in-place workspaces as one string, so the store subscription
 * only re-renders the page when that set changes rather than on every
 * app-state emit (git counters, agent state, …).
 */
function selectAttachedKey(state: { appState: { workspaces: readonly { attach_only?: boolean; host_id?: number | null; remote_cwd?: string | null }[] } | null }): string {
  const parts: string[] = [];
  for (const ws of state.appState?.workspaces ?? []) {
    if (ws.attach_only && ws.host_id != null && ws.remote_cwd) {
      parts.push(`${ws.host_id}\t${ws.remote_cwd}`);
    }
  }
  return parts.join("\n");
}

export function useDeviceCards(): DeviceCard[] {
  const hosts = useHosts();
  const statuses = useHostStatuses();
  const syncRows = useWorkspacesSync();
  const attachedKey = useAppStore(selectAttachedKey);
  // The coarse clock keeps "last seen 2d ago" honest while the page sits
  // open, without a per-card timer.
  const now = useCoarseClock(true);

  const attachedInPlace = useMemo<AttachedInPlace[]>(
    () =>
      attachedKey
        ? attachedKey.split("\n").map((line) => {
            const [hostId, remoteCwd] = line.split("\t");
            return { hostId: Number(hostId), remoteCwd };
          })
        : [],
    [attachedKey],
  );

  return useMemo(
    () => buildDeviceCards({ hosts, statuses, syncRows, attachedInPlace, now }),
    [hosts, statuses, syncRows, attachedInPlace, now],
  );
}
