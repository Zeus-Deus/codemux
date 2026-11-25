import { activateTab, getAppState } from "@/tauri/commands";

export interface ConfirmedPresetTabState {
  tabId: string | null;
  /** Null means current tab state could not be loaded. */
  openTabCount: number | null;
}

/** Find a tab that is still attached to the workspace after a partial launch. */
export async function focusConfirmedPresetTab(
  workspaceId: string,
  confirmedTabIds: readonly string[],
): Promise<ConfirmedPresetTabState> {
  let liveTabs: string[];
  try {
    // A failed snapshot is a read failure, not a failed preset launch. Retry
    // only this read; the preset command must never be dispatched again here.
    const snapshot = await getAppState().catch(() => getAppState());
    const workspace = snapshot.workspaces.find((item) => item.workspace_id === workspaceId);
    const terminalTabIds = new Set(
      workspace?.tabs.filter((tab) => tab.kind === "terminal").map((tab) => tab.tab_id) ?? [],
    );
    liveTabs = confirmedTabIds.filter((tabId) => terminalTabIds.has(tabId));
  } catch {
    return { tabId: null, openTabCount: null };
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

export function partialPresetStatusMessage(
  presetName: string,
  partial: { confirmedCommands: number; totalCommands: number },
  tab: ConfirmedPresetTabState,
): string {
  if (tab.tabId) {
    return `${presetName} opened ${tab.openTabCount ?? partial.confirmedCommands} of ${partial.totalCommands} tabs; another tab failed or closed.`;
  }
  if (tab.openTabCount === null) {
    return `${partial.confirmedCommands} of ${partial.totalCommands} preset commands reached terminals; open tabs could not be confirmed.`;
  }
  return `${partial.confirmedCommands} of ${partial.totalCommands} preset commands reached terminals; no agent tab remains open.`;
}
