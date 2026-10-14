import chatWorkflowScript from "./chat-workflow.script.js?raw";
import type { WorkflowRunSpec } from "@/tauri/workflows";
import { WORKFLOW_SIZES, workflowLimits, workflowTitleFromGoal, type WorkflowRoutePlan, type WorkflowSettings } from "./workflow-settings";

/**
 * The program behind `/workflow <goal>`. A planner agent reads the workspace
 * and splits the goal into independent subtasks (choosing a model route for
 * each), the subtasks run in parallel, and a final agent writes the report.
 * The host validates every structured result against its schema, so the plan
 * the program fans out is data, never agent prose.
 *
 * Task ids are part of the contract with the chat card: `plan`, `work-N` and
 * `report` map to its three phases.
 */
export const CHAT_WORKFLOW_SCRIPT = chatWorkflowScript;

export interface ChatWorkflowLaunch {
  spec: WorkflowRunSpec;
  source: string;
  args: { max_workers: number; route_labels: Record<string, string> };
}

export function buildChatWorkflowLaunch({
  workspaceId,
  goal,
  title,
  settings,
  plan,
}: {
  workspaceId: string;
  goal: string;
  /** Text to name the run from; defaults to the goal. */
  title?: string;
  settings: WorkflowSettings;
  plan: WorkflowRoutePlan;
}): ChatWorkflowLaunch {
  return {
    source: CHAT_WORKFLOW_SCRIPT,
    args: { max_workers: WORKFLOW_SIZES[settings.size].workers, route_labels: plan.labels },
    spec: {
      workspace_id: workspaceId,
      title: workflowTitleFromGoal(title ?? goal),
      goal,
      mode: "live",
      allow_writes: settings.allowWrites,
      routes: plan.routes,
      limits: workflowLimits(settings),
      tasks: [],
    },
  };
}

export type ChatWorkflowPhase = "plan" | "work" | "report";

/** Which card phase a task belongs to, or null for tasks a custom script made. */
export function chatWorkflowPhase(taskId: string): ChatWorkflowPhase | null {
  if (taskId === "plan") return "plan";
  if (taskId === "report") return "report";
  return /^work-\d+$/.test(taskId) ? "work" : null;
}
