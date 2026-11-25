import { activateTab, getAppState } from "@/tauri/commands";

/** Find a tab that is still attached to the workspace after a partial launch. */
export async function focusConfirmedPresetTab(
  workspaceId: string,
  confirmedTabIds: readonly string[],
): Promise<{ tabId: string | null; openTabCount: number }> {
  let liveTabs: string[];
  try {
    const snapshot = await getAppState();
    const workspace = snapshot.workspaces.find((item) => item.workspace_id === workspaceId);
    const terminalTabIds = new Set(
      workspace?.tabs.filter((tab) => tab.kind === "terminal").map((tab) => tab.tab_id) ?? [],
    );
    liveTabs = confirmedTabIds.filter((tabId) => terminalTabIds.has(tabId));
  } catch {
    return { tabId: null, openTabCount: 0 };
  }

  let openTabCount = liveTabs.length;
  for (const tabId of liveTabs) {
    try {
      await activateTab(workspaceId, tabId);
      return { tabId, openTabCount };
    } catch {
      // The tab may have closed after the snapshot; try another confirmed tab.
      openTabCount -= 1;
    }
  }
  return { tabId: null, openTabCount: 0 };
}
