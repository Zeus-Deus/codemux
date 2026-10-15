import { randomUUID } from "@/lib/uuid";
import { modelLabel, modelsForProvider } from "@/lib/agent-chat/capability-defaults";
import { buildChatWorkflowLaunch } from "@/lib/workflows/chat-workflow";
import { buildWorkflowRoutes, getWorkflowSettings } from "@/lib/workflows/workflow-settings";
import { useWorkflowUIStore } from "@/stores/workflow-ui-store";
import type { AgentChatProviderKind } from "@/tauri/types";
import { workflowCapabilities, workflowScriptRun, type WorkflowRunSnapshot } from "@/tauri/workflows";

/** The engine's goal limit, in UTF-8 bytes. */
const MAX_GOAL_BYTES = 65_536;

/** The goal every workflow agent sees: the typed goal plus any attached
 *  context (files, issues, PRs, conversation handoffs) the chat resolved. */
export function workflowGoalWithContext(goal: string, context: string | null | undefined): string {
  const full = context?.trim() ? `${goal}\n\n${context}` : goal;
  if (new TextEncoder().encode(full).length > MAX_GOAL_BYTES) {
    throw new Error("The attached context is too large for a workflow (64 KB). Remove an attachment and try again.");
  }
  return full;
}

/**
 * Start the `/workflow <goal>` program for a chat and link the run to the
 * chat so its card appears in the transcript. Throws a user-facing error when
 * no provider route can run it or the attached context is too large.
 */
export async function launchChatWorkflow({
  workspaceId,
  threadId,
  goal,
  context,
  provider,
  model,
  anchorEventId,
}: {
  workspaceId: string;
  threadId: string;
  goal: string;
  /** Resolved attachment block from the chat, if any. */
  context?: string | null;
  provider: AgentChatProviderKind;
  model: string | null;
  anchorEventId: number | null;
}): Promise<WorkflowRunSnapshot> {
  const capabilities = await workflowCapabilities();
  const settings = getWorkflowSettings();
  const plan = buildWorkflowRoutes({
    settings,
    capabilities,
    chat: { provider, model },
    models: (kind) => modelsForProvider(kind),
    modelLabel: (kind, id) => modelLabel(kind, id),
  });
  if (plan.problem) throw new Error(plan.problem);
  const launch = buildChatWorkflowLaunch({ workspaceId, goal: workflowGoalWithContext(goal, context), title: goal, settings, plan });
  const run = await workflowScriptRun(launch.spec, launch.source, launch.args, randomUUID());
  // The card and the Workflows pane load the run themselves; the pane's
  // history refreshes on the host's "created" event.
  const store = useWorkflowUIStore.getState();
  store.linkChatRun(threadId, { runId: run.id, workspaceId, anchorEventId, createdAt: Date.now() });
  store.selectRun(workspaceId, run.id);
  return run;
}
