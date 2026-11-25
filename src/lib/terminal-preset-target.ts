import type { WorkspaceCreateResult } from "@/tauri/commands";

/** Use the tab captured before workspace creation was exposed to the UI. */
export function initialTerminalTabId(created: WorkspaceCreateResult): string {
  if (!created.initialTabId) {
    throw new Error(`Workspace ${created.workspaceId} has no initial terminal tab`);
  }
  return created.initialTabId;
}
