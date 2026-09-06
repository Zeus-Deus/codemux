import { useEffect } from "react";
import { useHosts } from "@/stores/hosts-store";
import { useWorkspacesSyncStore } from "@/stores/workspaces-sync-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import type { HostView, WorkspaceSyncView } from "@/tauri/commands";

/**
 * The Devices destination only earns a footer slot once there is cross-device
 * state to show: a configured device, or synced work on a device this install
 * hasn't configured. Shared by the footer bar, the customize dialog and the
 * Devices button itself so the rule cannot drift between them.
 */
export function hasDevicesToShow(
  hosts: readonly HostView[],
  rows: readonly WorkspaceSyncView[],
): boolean {
  return hosts.length > 0 || rows.some((row) => row.workspace_id === null);
}

export function useFooterAvailability() {
  const agentChatEnabled = useFeatureFlags((s) => s.enableAgentChat);
  const hosts = useHosts();
  const hasDevices = useWorkspacesSyncStore((s) =>
    hasDevicesToShow(hosts, s.rows),
  );
  const init = useWorkspacesSyncStore((s) => s.init);
  useEffect(() => {
    void init();
  }, [init]);
  return { agentChatEnabled, hasDevices };
}
