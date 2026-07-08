import { useMemo } from "react";
import { useShallow } from "zustand/react/shallow";

import { activeWorkflowRun, latestWorkflowRun } from "@/lib/agent-chat/workflows";
import type { WorkflowRunItem } from "@/lib/agent-chat/types";
import { selectThread, useAgentChatStore } from "@/stores/agent-chat-store";
import type { PaneNodeSnapshot, WorkspaceSnapshot } from "@/tauri/types";

function collectAgentChatThreadIds(node: PaneNodeSnapshot): string[] {
  if (node.kind === "split") return node.children.flatMap(collectAgentChatThreadIds);
  if (node.kind === "agent_chat" && node.thread_id) return [node.thread_id];
  return [];
}

export interface WorkspaceWorkflow {
  run: WorkflowRunItem | null;
  threadId: string | null;
}

/**
 * Resolves the most relevant `Workflow` tool run for a workspace, so the
 * right-panel Orchestration tab (and its stale-tab guard) can decide
 * whether to render without every caller re-deriving the thread → run
 * lookup.
 *
 * Enumerates every agent-chat pane's `thread_id` across the workspace's
 * surfaces (same source `TitleBarTabs` uses — see
 * `collectPaneIds`/`selectThread` in title-bar-tabs.tsx), then prefers an
 * active (running / pending_approval) run from ANY of those threads over
 * a terminal one, falling back to the most recently started terminal run.
 *
 * Workflow items are only ever produced by the Claude provider's event
 * stream (see `workflows.ts` / `WorkflowRunItem` doc) — no codex/opencode
 * thread will ever carry a `workflow_run` item — so this hook doesn't
 * gate on `pane.provider`. Scanning every agent-chat pane's thread is
 * sufficient today and stays correct if another provider grows the
 * feature later.
 */
export function useWorkspaceWorkflow(
  workspace: WorkspaceSnapshot | null,
): WorkspaceWorkflow {
  const threadIds = useMemo(() => {
    if (!workspace) return [];
    const ids: string[] = [];
    for (const surface of workspace.surfaces) {
      ids.push(...collectAgentChatThreadIds(surface.root));
    }
    return ids;
  }, [workspace]);

  // `useShallow` is required: the selector below synthesises a fresh
  // `{ run, threadId }` object literal on every call. Default `Object.is`
  // equality would treat every unrelated store update (e.g. another
  // thread's streaming tokens) as a change and re-render this hook's
  // consumers on every tick. `useShallow` does a per-key comparison and
  // returns the previous reference when `run`/`threadId` are unchanged.
  return useAgentChatStore(
    useShallow((state) => {
      let bestRun: WorkflowRunItem | null = null;
      let bestThreadId: string | null = null;
      let fallbackRun: WorkflowRunItem | null = null;
      let fallbackThreadId: string | null = null;

      for (const threadId of threadIds) {
        const slice = selectThread(threadId)(state);
        if (!slice) continue;

        const active = activeWorkflowRun(slice.messages);
        if (active) {
          if (!bestRun || active.startedAt > bestRun.startedAt) {
            bestRun = active;
            bestThreadId = threadId;
          }
          continue;
        }

        const latest = latestWorkflowRun(slice.messages);
        if (latest && (!fallbackRun || latest.startedAt > fallbackRun.startedAt)) {
          fallbackRun = latest;
          fallbackThreadId = threadId;
        }
      }

      if (bestRun) return { run: bestRun, threadId: bestThreadId };
      return { run: fallbackRun, threadId: fallbackThreadId };
    }),
  );
}
