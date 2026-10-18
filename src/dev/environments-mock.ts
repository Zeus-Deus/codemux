/**
 * `?mockEnvironments`: seed the account's other machines in the browser
 * preview, so their workspaces can be inspected in the sidebar without a
 * second Codemux and an account. One machine is live, one is offline with its
 * last workspaces, one is waiting for approval.
 */
import { useEnvironmentsStore } from "@/environments/store";
import type { AppStateSnapshot, PaneStatus, WorkspaceSnapshot } from "@/tauri/types";

function workspace(
  id: string,
  title: string,
  project: string,
  extra: Partial<WorkspaceSnapshot> & { provider?: string } = {},
): WorkspaceSnapshot {
  const { provider = "claude", ...fields } = extra;
  return {
    workspace_id: id,
    title,
    workspace_type: "standard",
    cwd: `/home/dev/Projects/${project}`,
    project_root: `/home/dev/Projects/${project}`,
    git_branch: `feat/${title}`,
    git_ahead: 0,
    git_additions: 0,
    git_deletions: 0,
    notification_count: 0,
    surfaces: [
      { surface_id: `${id}-surface`, root: { kind: "agent_chat", pane_id: `${id}-pane`, provider } },
    ],
    ...fields,
  } as unknown as WorkspaceSnapshot;
}

function state(workspaces: WorkspaceSnapshot[], statuses: Record<string, PaneStatus>): AppStateSnapshot {
  return { workspaces, pane_statuses: statuses } as unknown as AppStateSnapshot;
}

export function seedEnvironmentsFromSearch(search: string): void {
  if (!new URLSearchParams(search).has("mockEnvironments")) return;
  const seen = new Date().toISOString();
  const store = useEnvironmentsStore.getState();
  store.setSelf({ id: "env-laptop", name: "laptop", platform: "linux", app_version: "0.24.0", sharing: true });
  store.setCatalog([
    { id: "1", deviceId: "env-ai-node", nodeId: "node-1", name: "ai-node", platform: "linux", lastSeenAt: seen },
    { id: "2", deviceId: "env-studio", nodeId: "node-2", name: "studio-mac", platform: "macos", lastSeenAt: null },
    { id: "3", deviceId: "env-pi", nodeId: "node-3", name: "build-box", platform: "linux", lastSeenAt: seen },
  ]);
  store.update("env-ai-node", {
    status: "connected",
    appState: state(
      [
        workspace("ai-1", "mobile-screenshots", "codemux", { git_ahead: 3, git_additions: 214, git_deletions: 38 }),
        workspace("ai-2", "relay-handshake", "codemux", { provider: "codex", git_additions: 61, git_deletions: 9 }),
        workspace("ai-3", "cuda-variant", "vexis", {
          git_additions: 402,
          git_deletions: 117,
          pr_number: 91,
          pr_state: "open",
          pr_url: "https://github.com/example/vexis/pull/91",
        }),
        workspace("ai-4", "docs-refresh", "codemux-site", { git_branch: "main" }),
      ],
      { "ai-1-pane": "working", "ai-2-pane": "permission", "ai-3-pane": "review" },
    ),
  });
  store.update("env-studio", {
    status: "offline",
    appState: state([workspace("st-1", "launch-video", "marketing", { git_additions: 12 })], {}),
  });
  store.update("env-pi", { status: "pending-approval", appState: null });
}
