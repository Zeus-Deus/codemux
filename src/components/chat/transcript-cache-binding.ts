import { createContext } from "react";
import type { AgentChatProviderKind, WorkspaceSnapshot } from "@/tauri/types";

export interface TranscriptBinding {
  key: string;
  workspaceId: string;
  threadKey: string;
  provider: AgentChatProviderKind;
  cwd: string | null;
}

export const TranscriptBindingContext = createContext<TranscriptBinding | null>(null);

/** Deliberately conservative: terminal-kind tabs host chat surfaces too.
 * Extra tabs/surfaces, splits, drafts and unbound startup chats are not cached. */
export function transcriptCacheBinding(workspace: WorkspaceSnapshot | null): TranscriptBinding | null {
  if (!workspace || workspace.tabs.length !== 1 || workspace.surfaces.length !== 1) return null;
  const tab = workspace.tabs[0];
  const surface = workspace.surfaces[0];
  const pane = surface.root;
  if (tab.kind !== "terminal" || tab.tab_id !== workspace.active_tab_id ||
      tab.surface_id !== surface.surface_id || surface.surface_id !== workspace.active_surface_id ||
      pane.kind !== "agent_chat" || pane.pane_id !== surface.active_pane_id || !pane.thread_id) return null;
  const provider = pane.provider ?? "claude";
  const cwd = pane.cwd ?? workspace.cwd;
  return {
    key: JSON.stringify([workspace.workspace_id, tab.tab_id, surface.surface_id, pane.pane_id,
      pane.thread_id, pane.provider, pane.cwd, workspace.cwd]),
    workspaceId: workspace.workspace_id,
    threadKey: pane.thread_id,
    provider,
    cwd,
  };
}
