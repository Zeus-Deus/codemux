/**
 * `?mockEnvironments`: seed the account's other machines in the browser
 * preview, so the sidebar's machine sections can be inspected without a
 * second Codemux and an account. One machine is live, one is offline with its
 * last workspaces, one is waiting for approval.
 */
import { useEnvironmentsStore } from "@/environments/store";
import type { AppStateSnapshot, PaneStatus, WorkspaceSnapshot } from "@/tauri/types";

function workspace(id: string, title: string, project: string): WorkspaceSnapshot {
  return {
    workspace_id: id,
    title,
    workspace_type: "standard",
    cwd: `/home/dev/Projects/${project}`,
    project_root: `/home/dev/Projects/${project}`,
    git_branch: title,
    surfaces: [{ surface_id: `${id}-surface`, root: { kind: "agent_chat", pane_id: `${id}-pane` } }],
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
        workspace("ai-1", "mobile-screenshots", "codemux"),
        workspace("ai-2", "relay-handshake", "codemux"),
        workspace("ai-3", "cuda-variant", "vexis"),
        workspace("ai-4", "docs-refresh", "codemux-site"),
      ],
      { "ai-1-pane": "working", "ai-2-pane": "permission", "ai-3-pane": "review" },
    ),
  });
  store.update("env-studio", {
    status: "offline",
    appState: state([workspace("st-1", "launch-video", "marketing")], {}),
  });
  store.update("env-pi", { status: "pending-approval", appState: null });
}
