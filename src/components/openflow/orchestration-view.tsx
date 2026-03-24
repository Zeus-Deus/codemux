import type { OpenFlowRunRecord, WorkspaceSnapshot } from "@/tauri/types";
import { useActiveCommLog, useActiveAgentSessions } from "@/stores/openflow-store";
import { useAppStore } from "@/stores/app-store";
import { activateTab } from "@/tauri/commands";
import {
  ResizablePanelGroup,
  ResizablePanel,
  ResizableHandle,
} from "@/components/ui/resizable";
import { OrchestrationHeader } from "./orchestration-header";
import { AgentGraph } from "./agent-graph";
import { CommunicationPanel } from "./communication-panel";
import { useCallback } from "react";

interface OrchestrationViewProps {
  workspace: WorkspaceSnapshot;
  run: OpenFlowRunRecord;
}

export function OrchestrationView({ workspace, run }: OrchestrationViewProps) {
  const commLog = useActiveCommLog();
  const agentSessions = useActiveAgentSessions();
  const appState = useAppStore((s) => s.appState);

  // Find the terminal tab for an agent instance and focus it
  const handleAgentClick = useCallback(
    (instanceId: string) => {
      if (!appState) return;

      // Match agent session by instance ID
      const session = agentSessions.find((s) => {
        const sid =
          s.config.role === "orchestrator"
            ? "orchestrator"
            : `${s.config.role}-${s.config.agent_index}`;
        return sid === instanceId;
      });
      if (!session) return;

      // Find the tab in the workspace that matches this session
      const tab = workspace.tabs.find(
        (t) => t.kind === "terminal" && t.title?.toLowerCase().includes(instanceId),
      );
      if (tab) {
        activateTab(workspace.workspace_id, tab.tab_id).catch(console.error);
      }
    },
    [agentSessions, workspace, appState],
  );

  return (
    <div className="flex h-full flex-col">
      <OrchestrationHeader workspace={workspace} run={run} />
      <ResizablePanelGroup orientation="horizontal" className="flex-1 min-h-0">
        <ResizablePanel defaultSize={55} minSize={30}>
          <AgentGraph
            run={run}
            agentSessions={agentSessions}
            commLog={commLog}
            onAgentClick={handleAgentClick}
          />
        </ResizablePanel>
        <ResizableHandle withHandle />
        <ResizablePanel defaultSize={45} minSize={25}>
          <CommunicationPanel runId={run.run_id} />
        </ResizablePanel>
      </ResizablePanelGroup>
    </div>
  );
}
