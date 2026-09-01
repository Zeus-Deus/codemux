import type {
  HostStatusView,
  HostView,
  WorkspaceSyncView,
} from "@/tauri/commands";
import type { WorkspaceSnapshot } from "@/tauri/types";

/**
 * Builders for the device-related tests. One copy so a field added to a
 * view lands in every suite at once.
 */

export function host(
  id: number,
  name: string,
  serverId: string | null = `srv-${name}`,
): HostView {
  return {
    id,
    server_id: serverId,
    name,
    ssh_target: `deus@${name}`,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    dirty: false,
  };
}

/** A probed-and-offline status unless overridden. */
export function status(
  hostId: number,
  partial: Partial<HostStatusView> = {},
): HostStatusView {
  return {
    host_id: hostId,
    probed: true,
    reachable: false,
    last_seen_at: null,
    last_error: null,
    disk_bytes: null,
    remote_control_serving: false,
    ...partial,
  };
}

let nextRowId = 1;

/** A sibling-device row on zeus unless overridden. */
export function syncRow(partial: Partial<WorkspaceSyncView> = {}): WorkspaceSyncView {
  const id = partial.id ?? nextRowId++;
  return {
    id,
    server_id: `ws-${id}`,
    workspace_id: null,
    title: `ws-${id}`,
    host_server_id: "srv-zeus",
    project_path: "/home/deus/projects/passpage",
    project_remote: "github.com/deus/passpage",
    git_branch: "main",
    git_head_sha: null,
    project_uid: "uid-passpage",
    workspace_kind: "worktree",
    default_branch: "main",
    origin_path: null,
    created_at: "2026-08-01T00:00:00Z",
    updated_at: "2026-08-01T00:00:00Z",
    dirty: false,
    ...partial,
  };
}

export function workspace(
  partial: Partial<WorkspaceSnapshot> & { workspace_id: string },
): WorkspaceSnapshot {
  return {
    title: partial.workspace_id,
    workspace_type: "standard",
    cwd: "/home/deus/projects/x",
    git_branch: null,
    git_ahead: 0,
    git_behind: 0,
    git_additions: 0,
    git_deletions: 0,
    git_changed_files: 0,
    notification_count: 0,
    latest_agent_state: null,
    worktree_path: null,
    project_root: null,
    pr_number: null,
    pr_state: null,
    pr_url: null,
    linked_issue: null,
    notifications_muted: false,
    tabs: [],
    active_tab_id: "",
    active_surface_id: "",
    surfaces: [],
    ...partial,
  } as WorkspaceSnapshot;
}
