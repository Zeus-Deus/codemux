import {
  DEFAULT_THREAD_PERMISSION_MODE,
  useAgentChatStore,
  type ChatMode,
} from "@/stores/agent-chat-store";
import { useAppStore } from "@/stores/app-store";
import {
  agentChatCreatePane,
  agentChatStartSession,
} from "@/tauri/commands";

/** Optional session config carried from the surface that triggered the
 *  prestart (Thread Scope deferred-worktree submit) so the new
 *  session launches with the SAME picker values the user was looking
 *  at, rather than resetting to defaults. Every field is optional —
 *  omitted fields keep the pre-existing defaults (model null,
 *  permission mode `DEFAULT_THREAD_PERMISSION_MODE`). */
export interface PrestartSessionConfig {
  model?: string | null;
  permissionMode?: string;
  effort?: string | null;
  contextWindow?: string | null;
  /** Composer mode pill to mirror onto the new slice. Callers that
   *  need `permissionMode: "plan"` for plan/ask compute that
   *  themselves — this field only seeds the slice's pill state. */
  mode?: ChatMode;
}

/** Resolved handles for the pre-started session, so a caller can
 *  route a first turn into it (`agent_chat_send_turn` with
 *  `thread_id`) before activating the workspace. */
export interface PrestartedWorktreeSession {
  paneId: string;
  threadId: string;
}

/**
 * Pre-start the chat session for a freshly-created worktree workspace.
 *
 * Mirrors the `materializeAndSend` pattern: attach a chat pane, mint a
 * thread id, and `await agent_chat_start_session` BEFORE the caller
 * activates the workspace. By the time `AgentChatPane` mounts, the
 * backend has already called `set_agent_chat_thread_id(pane_id,
 * thread_id)` and emitted the state, so `pane.thread_id` is non-null
 * at mount. The mount-effect's first branch (`if (threadId) {
 * ensureThread(threadId); return; }`) runs and no duplicate start is
 * attempted.
 *
 * Without this pre-start, `AgentChatPane`'s mount-effect minted a
 * thread id locally and fired `agent_chat_start_session` async. If
 * the user queued a message before that resolved (or before
 * `pane.cwd` was populated enough to pass the `if (!cwd) return`
 * guard inside the mount-effect), `send_turn` could land with a
 * thread id that wasn't registered in the adapter's session map →
 * `session_not_found`.
 *
 * Returns the created `{ paneId, threadId }` so the Thread Scope
 * deferred-worktree path can send the user's first message into the
 * new session before activating; returns `null` on the
 * workspace-not-in-store fallback below.
 *
 * ## CWD resolution
 *
 * The worktree path is only known to the backend after
 * `git_create_worktree` runs. We read it from the app-store after
 * `create_worktree_workspace` returns — `emit_app_state` fires
 * synchronously inside that Tauri command before the `Ok()` return,
 * so the event is on the frontend's IPC queue ahead of the invoke
 * response. In the degenerate case where the workspace still isn't
 * in the store (e.g. event delivery reordering), we skip the
 * pre-start entirely and fall back to the mount-effect path — not
 * ideal, but strictly no worse than today's buggy behavior.
 */
export async function prestartWorktreeSession(
  workspaceId: string,
  config: PrestartSessionConfig = {},
): Promise<PrestartedWorktreeSession | null> {
  const ws = useAppStore
    .getState()
    .appState?.workspaces.find((w) => w.workspace_id === workspaceId);
  const cwd = ws?.cwd;
  if (!cwd) {
    console.warn(
      "[prestart-worktree-session] workspace not yet in store; deferring " +
        "session start to AgentChatPane mount-effect",
      { workspaceId },
    );
    return null;
  }

  const paneId = await agentChatCreatePane(workspaceId, "claude", cwd);
  const threadId = crypto.randomUUID();
  const permissionMode =
    config.permissionMode ?? DEFAULT_THREAD_PERMISSION_MODE;

  await agentChatStartSession(paneId, "claude", {
    thread_id: threadId,
    cwd,
    model: config.model ?? null,
    resume_cursor: null,
    permission_mode: permissionMode,
    effort: config.effort ?? null,
    context_window: config.contextWindow ?? null,
    additional_directories: [],
    env: null,
  });

  // Seed the slice so the mount-effect's `ensureThread(threadId)`
  // lands on a populated entry and pickers render with sensible
  // defaults instead of waiting on an echo from the live session.
  // Optional config setters run only when the caller provided a
  // value, so legacy callers (no config) keep the original
  // permission-mode-only seed.
  const chat = useAgentChatStore.getState();
  chat.ensureThread(threadId);
  chat.setPermissionMode(threadId, permissionMode);
  chat.setSessionLaunchMode(threadId, permissionMode);
  if (config.model != null) chat.setModel(threadId, config.model);
  if (config.effort != null) chat.setEffort(threadId, config.effort);
  if (config.contextWindow != null) {
    chat.setContextWindow(threadId, config.contextWindow);
  }
  if (config.mode != null && config.mode !== "default") {
    chat.setMode(threadId, config.mode);
  }

  return { paneId, threadId };
}
