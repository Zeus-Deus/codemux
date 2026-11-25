import { getAppState } from "@/tauri/commands";

/** The first terminal belongs to the workspace just created, even if focus
 * moves to another tab before the preset command is dispatched. */
export async function firstTerminalTabId(workspaceId: string): Promise<string> {
  const workspace = (await getAppState()).workspaces.find(
    (candidate) => candidate.workspace_id === workspaceId,
  );
  const tab = workspace?.tabs.find((candidate) => candidate.kind === "terminal");
  if (!tab) throw new Error(`Workspace ${workspaceId} has no terminal tab`);
  return tab.tab_id;
}
