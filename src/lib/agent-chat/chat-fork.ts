/**
 * Fork a chat into a new, independent thread (`agent_chat_fork_thread`).
 *
 * The fork continues the agent's own session, so the model keeps its full
 * context, and then lives on as an ordinary chat: it opens in a new tab and
 * resumes on its first send. Only providers that fork natively offer it
 * (the same set as `/btw` side chats); for the others the action is hidden.
 *
 * A fork from an earlier assistant reply keeps the transcript up to that
 * turn. It is addressed by the user message that starts the next turn.
 * Codex can only cut where that turn has a checkpoint (the same turns
 * Revert is offered on); Claude cuts at any finished turn.
 */
import { resolveWorktreeBranchName } from "@/lib/agent-chat/materialize";
import { waitForWorkspaceCwd } from "@/lib/agent-chat/wait-for-workspace-cwd";
import { launchAgentChatPane } from "@/lib/agent-chat/launch-pane";
import type { ChatViewItem } from "@/lib/agent-chat/types";
import { activateWorkspaceInteraction } from "@/lib/perf/instrumented-activate";
import { toast } from "@/lib/toast";
import { useAppStore } from "@/stores/app-store";
import {
  agentChatForkCheck,
  agentChatForkThread,
  agentChatGetSession,
  createWorktreeWorkspaceResult,
  type AgentChatTurnCheckpointRecord,
} from "@/tauri/commands";
import type { AgentChatProviderKind, WorkspaceSnapshot } from "@/tauri/types";

/** Where a fork runs: the source chat's folder, or a new worktree. */
export type ForkTarget = "here" | "worktree";

/** A point a chat can be forked from: `null` is the latest message,
 *  otherwise the row of the user message the fork leaves out. */
export type ForkBefore = number | null;

/** Providers that fork their own session (mirrors `supports_native_fork`). */
export function chatForkSupported(
  provider: AgentChatProviderKind | null | undefined,
): boolean {
  return provider === "claude" || provider === "codex";
}

/** A user message that starts a turn: not parked in the queue and not
 *  steered into a turn that was already running. */
function startsTurn(item: ChatViewItem): item is Extract<ChatViewItem, { kind: "user_message" }> {
  return item.kind === "user_message" && !item.queued && !item.inflight;
}

/**
 * The fork point each settled assistant reply offers, keyed by item id:
 * the last reply of every finished turn. The newest turn forks from the
 * latest message; an earlier one cuts before the next turn's user
 * message, which needs a durable row id (and, for Codex, a checkpoint).
 * Nothing is offered while the chat is busy: the backend only forks a
 * chat between turns.
 */
export function computeForkPoints(
  items: readonly ChatViewItem[],
  provider: AgentChatProviderKind | null | undefined,
  checkpointByNonce: ReadonlyMap<string, AgentChatTurnCheckpointRecord> | undefined,
  busy: boolean,
): Map<string, ForkBefore> {
  const points = new Map<string, ForkBefore>();
  if (busy || !chatForkSupported(provider)) return points;
  let lastReply: string | null = null;
  for (const item of items) {
    if (item.kind === "assistant_message" && !item.streaming && item.text) {
      lastReply = item.id;
      continue;
    }
    if (!startsTurn(item) || lastReply === null) continue;
    const row = item.source_event_id;
    const checkpointed =
      provider !== "codex" ||
      (!!item.clientNonce &&
        checkpointByNonce?.get(item.clientNonce)?.transcript_cutoff_id === (row ?? 0) - 1);
    if (row != null && checkpointed) points.set(lastReply, row);
    lastReply = null;
  }
  if (lastReply !== null) points.set(lastReply, null);
  return points;
}

/** A fork into a new worktree branches off the source workspace's own
 *  branch, so the fork starts from the same code. */
async function createForkWorktree(
  workspace: WorkspaceSnapshot,
  title: string,
): Promise<string> {
  const projectPath = workspace.project_root ?? workspace.cwd;
  const branch = await resolveWorktreeBranchName(projectPath, "", title);
  const created = await createWorktreeWorkspaceResult(
    projectPath,
    branch,
    true,
    "empty",
    workspace.git_branch,
    null,
    null,
    null,
    null,
    undefined,
    false,
  );
  const cwd = created.cwd ?? (await waitForWorkspaceCwd(created.workspaceId));
  if (!cwd) throw new Error("The new worktree didn't finish setting up.");
  return created.workspaceId;
}

export function workspaceById(workspaceId: string | null | undefined): WorkspaceSnapshot | null {
  if (!workspaceId) return null;
  return (
    useAppStore
      .getState()
      .appState?.workspaces.find((workspace) => workspace.workspace_id === workspaceId) ?? null
  );
}

/** Whether a fork of a chat in this workspace can get its own worktree. */
export function canForkIntoWorktree(workspace: WorkspaceSnapshot | null): boolean {
  return !!workspace && workspace.is_git !== false && !workspace.host_id;
}

export interface ForkChatInput {
  sourceThreadId: string;
  workspaceId: string;
  before: ForkBefore;
  target: ForkTarget;
}

/**
 * Fork a chat and open the fork in a new tab. Failures surface as a toast;
 * resolves to the new thread id, or `null` when the fork failed.
 */
export async function forkChat(input: ForkChatInput): Promise<string | null> {
  let worktreeId: string | null = null;
  try {
    if (input.target === "worktree") {
      const workspace = workspaceById(input.workspaceId);
      if (!workspace || !canForkIntoWorktree(workspace)) {
        throw new Error("This chat's workspace can't get a new worktree.");
      }
      // Refuse before creating a worktree the fork could never use.
      await agentChatForkCheck(input.sourceThreadId, input.before);
      const source = await agentChatGetSession(input.sourceThreadId).catch(() => null);
      worktreeId = await createForkWorktree(workspace, source?.title ?? "fork");
    }
    const fork = await agentChatForkThread(input.sourceThreadId, input.before, worktreeId);
    // A new worktree's workspace is empty, so the fork fills it (as a first
    // send does); beside the source it gets its own tab.
    await launchAgentChatPane(
      fork.workspace_id,
      fork.provider,
      fork.cwd,
      worktreeId ? null : "new_tab",
      fork.thread_id,
    );
    if (fork.workspace_id !== input.workspaceId) await activateWorkspaceInteraction(fork.workspace_id);
    toast.success("Chat forked", { description: fork.title });
    return fork.thread_id;
  } catch (error) {
    toast.failure(
      worktreeId ? "Created the worktree, but couldn't fork the chat" : "Couldn't fork the chat",
      error,
    );
    return null;
  }
}
