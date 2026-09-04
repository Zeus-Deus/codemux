import { useEffect } from "react";
import { useHosts } from "@/stores/hosts-store";
import { useWorkspacesSyncStore } from "@/stores/workspaces-sync-store";
import { useFeatureFlags } from "@/stores/feature-flags";

export function useFooterAvailability() {
  const agentChatEnabled = useFeatureFlags((s) => s.enableAgentChat);
  const hosts = useHosts();
  const hasSiblingRows = useWorkspacesSyncStore((s) =>
    s.rows.some((row) => row.workspace_id === null),
  );
  const init = useWorkspacesSyncStore((s) => s.init);
  useEffect(() => {
    void init();
  }, [init]);
  return { agentChatEnabled, hasDevices: hosts.length > 0 || hasSiblingRows };
}
