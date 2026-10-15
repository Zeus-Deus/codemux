import { isRemoteClient } from "@/components/remote/is-remote-client";
import { useAppStore } from "@/stores/app-store";
import { useFeatureFlags } from "@/stores/feature-flags";

/** Workflows run against this machine's local checkouts, so they need Agent
 *  Chat and a local, fully-attached workspace in the desktop client. */
export function useWorkflowsAvailable(workspaceId: string | null | undefined): boolean {
  const enabled = useFeatureFlags((state) => state.enableAgentChat);
  const workspace = useAppStore((state) => workspaceId ? state.appState?.workspaces.find((item) => item.workspace_id === workspaceId) : undefined);
  return enabled && !!workspace && !workspace.host_id && !workspace.attach_only && !isRemoteClient();
}
