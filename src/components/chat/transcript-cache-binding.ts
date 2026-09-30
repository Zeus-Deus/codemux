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

/** Terminal-kind tabs host chat surfaces too. Splits remain uncached: portal
 * ancestry bypasses the pane capture events that activate split panes. */
function bindingForTab(workspace: WorkspaceSnapshot, tab: WorkspaceSnapshot["tabs"][number]): TranscriptBinding | null {
  if (tab.kind !== "terminal") return null;
  const surface = workspace.surfaces.find((candidate) => candidate.surface_id === tab.surface_id);
  if (!surface) return null;
  const pane = surface.root;
  if (pane.kind !== "agent_chat" || pane.pane_id !== surface.active_pane_id || !pane.thread_id) return null;
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

export function transcriptCacheBinding(workspace: WorkspaceSnapshot | null): TranscriptBinding | null {
  if (!workspace) return null;
  const tab = workspace.tabs.find((candidate) => candidate.tab_id === workspace.active_tab_id);
  if (!tab || tab.surface_id !== workspace.active_surface_id) return null;
  return bindingForTab(workspace, tab);
}

/** Valid identities include parked tabs, independently of the selected route. */
export function transcriptCacheBindings(workspace: WorkspaceSnapshot | null): TranscriptBinding[] {
  return workspace ? workspace.tabs.flatMap((tab) => {
    const binding = bindingForTab(workspace, tab);
    return binding ? [binding] : [];
  }) : [];
}
